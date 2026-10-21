//! Telemetry through the hooks (spec §9.2, §11.1): Stop, SubagentStop and SessionEnd read
//! the new lines of the transcripts and record each model reply once. Transcripts here
//! are synthetic files in temp folders.

mod common;

use std::io::Write as _;
use std::path::{Path, PathBuf};

use bridge_screen_core::store::{Event, Store, read_all};
use bridge_screen_core::telemetry::{self, Scope};
use common::{Env, fixture_json};
use serde_json::{Value, json};

/// A transcript line for reply `id`; the same reply may be written on several lines.
fn line(id: &str, output: u64) -> String {
    line_at(id, output, &jiff::Timestamp::now().to_string(), false)
}

fn line_at(id: &str, output: u64, ts: &str, sidechain: bool) -> String {
    format!(
        r#"{{"parentUuid":"p","isSidechain":{sidechain},"type":"assistant","requestId":"req_{id}","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-opus-5-5","role":"assistant","content":[{{"type":"tool_use","name":"Bash","input":{{"command":"echo PRIVATE-TOOL-INPUT"}}}}],"usage":{{"input_tokens":3,"cache_creation_input_tokens":30,"cache_read_input_tokens":200,"output_tokens":{output},"service_tier":"standard","cache_creation":{{"ephemeral_1h_input_tokens":20,"ephemeral_5m_input_tokens":10}}}}}}}}"#
    ) + "\n"
}

/// A user line that mentions the words the pre-filter looks for.
fn user_line() -> String {
    r#"{"type":"user","message":{"role":"user","content":"PRIVATE prompt: what \"usage\" did the \"assistant\" have?"}}"#
        .to_string()
        + "\n"
}

fn append(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

/// A session whose transcript lives in a temp folder.
struct Transcript {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Transcript {
    fn new(session: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project").join(format!("{session}.jsonl"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        Self { _dir: dir, path }
    }

    fn text(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

fn hook(env: &Env, event: &str, session: &str, t: &Transcript, edit: impl FnOnce(&mut Value)) {
    let mut p = fixture_json(event, session, &env.repo_path());
    p["transcript_path"] = json!(t.text());
    edit(&mut p);
    let out = env.hook_raw(event, &p.to_string());
    if event == "stop" || event == "subagent-stop" {
        assert_eq!(out, "", "{event}");
    }
}

/// session-start then user-prompt-submit for `session`.
fn start(env: &Env, session: &str, t: &Transcript) {
    hook(env, "session-start", session, t, |_| {});
    hook(env, "user-prompt-submit", session, t, |_| {});
}

fn events(env: &Env) -> Vec<Event> {
    let store = Store::open(&env.db_path()).unwrap();
    read_all(store.conn(), None).unwrap()
}

fn usage_events(env: &Env) -> Vec<Event> {
    events(env)
        .into_iter()
        .filter(|e| e.kind == "usage")
        .collect()
}

fn session_tokens(env: &Env, session: &str) -> telemetry::Tokens {
    let store = Store::open(&env.db_path()).unwrap();
    telemetry::totals(store.conn(), &Scope::Session(session.into())).unwrap()
}

#[test]
fn usage_counts_each_reply_once() {
    let env = Env::new();
    let t = Transcript::new("s1");
    start(&env, "s1", &t);
    // Three lines for one reply, and a second reply whose lines span two reads.
    append(
        &t.path,
        &[line("m1", 7), line("m1", 7), line("m1", 7), line("m2", 5)].concat(),
    );
    hook(&env, "stop", "s1", &t, |_| {});
    append(&t.path, &[line("m2", 5), line("m3", 1)].concat());
    hook(&env, "user-prompt-submit", "s1", &t, |_| {});
    hook(&env, "stop", "s1", &t, |_| {});
    let tokens = session_tokens(&env, "s1");
    assert_eq!((tokens.replies, tokens.sum.output), (3, 13));
    // The file is rewritten shorter (all lines again): read from 0, nothing doubles.
    std::fs::write(&t.path, [line("m1", 7), line("m2", 5)].concat()).unwrap();
    hook(&env, "stop", "s1", &t, |_| {});
    let tokens = session_tokens(&env, "s1");
    assert_eq!((tokens.replies, tokens.sum.output), (3, 13));
    let last = usage_events(&env).pop().unwrap();
    assert_eq!(last.data["restarted"], true);
    assert_eq!(last.data["replies"], json!([]));
    assert_eq!(env.log(), "");
}

#[test]
fn replies_go_to_the_unit_and_task_and_nothing_private_is_kept() {
    let env = Env::new();
    let t = Transcript::new("s1");
    start(&env, "s1", &t);
    let repo = bridge_screen_core::repo::identify(env.repo.path()).unwrap();
    append(&t.path, &[user_line(), line("m1", 7)].concat());
    hook(&env, "stop", "s1", &t, |_| {});
    let store = Store::open(&env.db_path()).unwrap();
    let (unit, repo_id): (Option<i64>, String) = store
        .conn()
        .query_row(
            "SELECT unit_no, repo_id FROM usage_replies WHERE message_id = 'm1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((unit, repo_id), (Some(1), repo.id));
    // Only ids, model names, token counts and timestamps are kept.
    let all: String = events(&env).iter().map(|e| e.data.to_string()).collect();
    assert!(!all.contains("PRIVATE"), "{all}");
    let u = &usage_events(&env)[0];
    let keys: Vec<&str> = u
        .data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["offset_after", "path", "replies"]);
    let reply_keys: Vec<&str> = u.data["replies"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        reply_keys,
        [
            "cache_read",
            "cache_write_1h",
            "cache_write_5m",
            "input",
            "message_id",
            "model",
            "output",
            "sidechain",
            "ts"
        ]
    );
}

#[test]
fn backlog_is_read_in_bounded_slices() {
    // ~3 MB of transcript before the first Stop (a session older than the upgrade).
    let env = Env::new();
    let t = Transcript::new("s1");
    start(&env, "s1", &t);
    let mut text = String::new();
    let mut n = 0;
    while text.len() < 3 * 1024 * 1024 {
        text.push_str(&line(&format!("m{n}"), n % 50));
        text.push_str(&line(&format!("m{n}"), n % 50));
        text.push_str(&user_line());
        n += 1;
    }
    append(&t.path, &text);
    let size = text.len() as u64;
    let mut stops = 0;
    loop {
        hook(&env, "stop", "s1", &t, |_| {});
        stops += 1;
        let last = usage_events(&env).pop().unwrap();
        if last.data["offset_after"].as_u64().unwrap() >= size || stops > 20 {
            break;
        }
    }
    // Each Stop read at most 512 KiB.
    let mut before = 0;
    for e in usage_events(&env) {
        let after = e.data["offset_after"].as_u64().unwrap();
        assert!(
            after - before <= telemetry::MAX_HOOK_BYTES,
            "{after} - {before}"
        );
        before = after;
    }
    assert_eq!(stops, size.div_ceil(telemetry::MAX_HOOK_BYTES));
    let sliced = session_tokens(&env, "s1");

    // The same transcript read in one go by the owner's catch-up.
    let other = Env::new();
    let t2 = Transcript::new("s1");
    start(&other, "s1", &t2);
    append(&t2.path, &text);
    let out = other.run(&["usage", "--catch-up"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8(out.stdout).unwrap();
    assert!(printed.contains(&format!("{n} replies")), "{printed}");
    let whole = session_tokens(&other, "s1");
    assert_eq!(sliced, whole);
    assert_eq!(whole.replies, n);
    // Caught up: a second run adds nothing.
    let again = String::from_utf8(other.run(&["usage", "--catch-up"], "").stdout).unwrap();
    assert!(again.contains("Nothing new"), "{again}");
}

#[test]
fn partial_lines_and_shrunk_files() {
    let env = Env::new();
    let t = Transcript::new("s1");
    start(&env, "s1", &t);
    let full = line("m1", 7);
    let (head, tail) = full.split_at(full.len() / 2);
    append(&t.path, &(line("m0", 1) + &user_line() + head));
    hook(&env, "stop", "s1", &t, |_| {});
    assert_eq!(session_tokens(&env, "s1").replies, 1);
    // The half-written line is finished later: counted once, then.
    append(&t.path, tail);
    hook(&env, "stop", "s1", &t, |_| {});
    hook(&env, "stop", "s1", &t, |_| {});
    let tokens = session_tokens(&env, "s1");
    assert_eq!((tokens.replies, tokens.sum.output), (2, 8));
    // Replaced by a shorter file with one old and one new reply.
    std::fs::write(&t.path, [line("m1", 7), line("m5", 2)].concat()).unwrap();
    hook(&env, "stop", "s1", &t, |_| {});
    let tokens = session_tokens(&env, "s1");
    assert_eq!((tokens.replies, tokens.sum.output), (3, 10));
}

#[test]
fn subagent_usage_goes_to_the_parent_task_with_its_type() {
    let env = Env::new();
    let t = Transcript::new("s1");
    start(&env, "s1", &t);
    let store = Store::open(&env.db_path()).unwrap();
    store
        .conn()
        .execute(
            "UPDATE units SET task_id = '1.1' WHERE session_id = 's1'",
            [],
        )
        .unwrap();
    drop(store);
    let explicit = telemetry::subagent_path(&t.path, "s1", "a1");
    append(
        &explicit,
        &line_at("sub1", 30, &jiff::Timestamp::now().to_string(), true),
    );
    hook(&env, "subagent-stop", "s1", &t, |p| {
        p["agent_id"] = json!("a1");
        p["agent_transcript_path"] = json!(explicit.to_string_lossy());
    });
    // Without agent_transcript_path, the path is derived from the session's transcript.
    let derived = telemetry::subagent_path(&t.path, "s1", "a2");
    append(&derived, &line("sub2", 4));
    hook(&env, "subagent-stop", "s1", &t, |p| {
        p["agent_id"] = json!("a2");
        p["agent_type"] = json!("Plan");
        p.as_object_mut().unwrap().remove("agent_transcript_path");
    });
    let store = Store::open(&env.db_path()).unwrap();
    let rows: Vec<(String, String, String, Option<String>)> = store
        .conn()
        .prepare(
            "SELECT message_id, agent_id, agent_type, task_id FROM usage_replies ORDER BY message_id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [
            (
                "sub1".into(),
                "a1".into(),
                "Explore".into(),
                Some("1.1".into())
            ),
            (
                "sub2".into(),
                "a2".into(),
                "Plan".into(),
                Some("1.1".into())
            ),
        ]
    );
    let by_agent = telemetry::totals(
        store.conn(),
        &Scope::Agent {
            session: "s1".into(),
            agent_id: "a1".into(),
        },
    )
    .unwrap();
    assert_eq!(by_agent.sum.output, 30);
    assert_eq!(env.log(), "");
}

#[test]
fn a_missing_transcript_records_nothing() {
    let env = Env::new();
    let t = Transcript::new("s1");
    start(&env, "s1", &t);
    std::fs::remove_file(&t.path).unwrap();
    hook(&env, "stop", "s1", &t, |_| {});
    hook(&env, "subagent-stop", "s1", &t, |_| {});
    hook(&env, "session-end", "s1", &t, |_| {});
    assert_eq!(usage_events(&env).len(), 0);
    assert_eq!(env.log(), "");
    let kinds: Vec<String> = events(&env).into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"session_ended".to_string()), "{kinds:?}");
}

#[test]
fn session_end_reads_the_last_lines() {
    let env = Env::new();
    let t = Transcript::new("s1");
    start(&env, "s1", &t);
    hook(&env, "stop", "s1", &t, |_| {});
    append(&t.path, &line("late", 9));
    hook(&env, "session-end", "s1", &t, |_| {});
    assert_eq!(session_tokens(&env, "s1").sum.output, 9);
}
