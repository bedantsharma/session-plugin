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
    // `identity` is the file's device, inode and creation time: its contents are never
    // read for it.
    assert_eq!(keys, ["identity", "offset_after", "path", "replies"]);
    assert!(
        u.data["identity"]
            .as_str()
            .is_some_and(|i| i.starts_with("dev:") && !i.contains("PRIVATE")),
        "{}",
        u.data
    );
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

/// Task 1.1 "Login form" with Opus and Sonnet replies; an Explore subagent on an unpriced
/// model outside any task.
fn seeded_usage(env: &Env) {
    use bridge_screen_core::plan::{EditContext, NodeKind, PlanOp, plan_edit};
    use bridge_screen_core::store::{Actor, EventDraft};
    let repo = bridge_screen_core::repo::identify(env.repo.path()).unwrap();
    let mut store = Store::open(&env.db_path()).unwrap();
    store.register_repo(&repo).unwrap();
    let add = |parent: Option<&str>, kind, title: &str| PlanOp::AddNode {
        parent: parent.map(str::to_string),
        kind,
        title: title.into(),
        intent: Some("why".into()),
        estimate_min: Some(30),
        expected_files: vec![],
    };
    plan_edit(
        &mut store,
        &EditContext {
            repo_id: repo.id.clone(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: None,
            replan: false,
        },
        &[
            add(None, NodeKind::Goal, "Ship"),
            add(Some("G"), NodeKind::Milestone, "Auth"),
            add(Some("1"), NodeKind::Task, "Login form"),
        ],
    )
    .unwrap();
    let t0: jiff::Timestamp = "2026-10-14T09:00:00Z".parse().unwrap();
    let at = |s: i64| (t0 + jiff::SignedDuration::from_secs(s)).to_string();
    let reply = |id: &str, model: &str, secs: i64, input: u64, output: u64| {
        json!({"message_id": id, "model": model, "ts": at(secs), "input": input,
               "output": output, "cache_read": 0, "cache_write_5m": 0, "cache_write_1h": 0})
    };
    let sid = "s1abcdef";
    let events = vec![
        (
            "session_started",
            json!({"cwd": env.repo_path(), "source": "startup", "branch": "feat/login"}),
            Actor::default(),
        ),
        (
            "task_claimed",
            json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
            Actor::default(),
        ),
        ("prompt", json!({"chars": 3}), Actor::default()),
        (
            "usage",
            json!({"path": "/t/main.jsonl", "offset_after": 10, "replies": [
                reply("a", "claude-opus-5-5", 10, 1_000_000, 100_000),
                reply("b", "claude-sonnet-5-5", 20, 500_000, 0)]}),
            Actor::default(),
        ),
        ("unit_stopped", json!({"unit_no": 1}), Actor::default()),
        (
            "usage",
            json!({"path": "/t/sub.jsonl", "offset_after": 10, "agent_id": "ag",
                   "agent_type": "Explore",
                   "replies": [reply("c", "mystery-model", 500, 0, 1_000)]}),
            Actor {
                agent_id: Some("ag".into()),
                agent_type: Some("Explore".into()),
                model: None,
            },
        ),
    ];
    store
        .write(|tx| {
            for (i, (kind, data, actor)) in events.into_iter().enumerate() {
                let draft = EventDraft::new(&repo.id, sid, kind, data).actor(actor);
                let secs = i64::try_from(i).unwrap();
                bridge_screen_core::store::append_in_at(
                    tx,
                    draft,
                    t0 + jiff::SignedDuration::from_secs(secs),
                )?;
            }
            Ok(())
        })
        .unwrap();
}

fn usage_text(env: &Env, args: &[&str]) -> String {
    let mut all = vec!["usage"];
    all.extend_from_slice(args);
    let out = env.run(&all, "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn usage_cli_splits_by_task_model_session_and_agent() {
    let env = Env::new();
    seeded_usage(&env);
    let name = env
        .repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let total = format!("Total for {name}: 1.6M tokens · ≥ $7.00 (some models unpriced)");
    let note = "No price for mystery-model: add one under [prices] in config.toml.";

    let by_task = usage_text(&env, &[]);
    assert!(
        by_task
            .contains("Login form (1.1): 1.6M tokens · $7.00 (Opus 5.5 $6.00, Sonnet 5.5 $1.00)"),
        "{by_task}"
    );
    assert!(
        by_task.contains("No task: 1k tokens · cost unknown"),
        "{by_task}"
    );
    assert!(by_task.contains(&total), "{by_task}");
    assert!(by_task.contains(note), "{by_task}");

    let by_model = usage_text(&env, &["--by", "model"]);
    assert!(
        by_model.contains("Opus 5.5: 1.1M tokens · $6.00"),
        "{by_model}"
    );
    assert!(
        by_model.contains("Sonnet 5.5: 500k tokens · $1.00"),
        "{by_model}"
    );
    assert!(
        by_model.contains("mystery-model: 1k tokens · cost unknown"),
        "{by_model}"
    );

    let by_session = usage_text(&env, &["--by", "session"]);
    assert!(
        by_session.contains("s1abcd (feat/login): 1.6M tokens · ≥ $7.00 (some models unpriced)"),
        "{by_session}"
    );

    let by_agent = usage_text(&env, &["--by", "agent"]);
    assert!(
        by_agent.contains("Main agent: 1.6M tokens · $7.00"),
        "{by_agent}"
    );
    assert!(
        by_agent.contains("Explore: 1k tokens · cost unknown"),
        "{by_agent}"
    );

    let one_task = usage_text(&env, &["--task", "1.1", "--by", "model"]);
    assert!(!one_task.contains("mystery-model"), "{one_task}");
    assert!(
        one_task.contains("Total for Login form (1.1): 1.6M tokens · $7.00"),
        "{one_task}"
    );
    let one_session = usage_text(&env, &["--session", "s1a"]);
    assert!(
        one_session.contains("Total for session s1abcd"),
        "{one_session}"
    );
    let unknown = env.run(&["usage", "--session", "zz"], "");
    assert!(!unknown.status.success());
}

#[test]
fn config_shows_the_price_table_and_its_sources() {
    let env = Env::new();
    std::fs::write(
        env.home.path().join("config.toml"),
        "[prices.claude-opus-5-5]\ninput = 1.0\noutput = 2.0\ncache_read = 0.1\n\
         cache_write_5m = 1.25\ncache_write_1h = 2.0\n",
    )
    .unwrap();
    let out = env.run(&["config"], "");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("#   claude-opus-5-5: 1 / 2 / 0.1 / 1.25 / 2 (yours)"),
        "{text}"
    );
    assert!(
        text.contains("#   claude-sonnet-5-5: 2 / 10 / 0.2 / 2.5 / 4 (default)"),
        "{text}"
    );
    assert!(!text.contains("[prices"), "{text}");
}

/// A subagent stops once, but its transcript can be bigger than one hook's slice: the
/// session's later Stops read the rest, still with the subagent's id and type.
#[test]
fn a_big_subagent_transcript_is_finished_by_later_stops() {
    let env = Env::new();
    let t = Transcript::new("s1");
    start(&env, "s1", &t);
    let path = telemetry::subagent_path(&t.path, "s1", "a1");
    let mut text = String::new();
    let mut n = 0;
    while text.len() < 1300 * 1024 {
        text.push_str(&line_at(
            &format!("sub{n}"),
            1,
            &jiff::Timestamp::now().to_string(),
            true,
        ));
        text.push_str(&user_line());
        n += 1;
    }
    append(&path, &text);
    hook(&env, "subagent-stop", "s1", &t, |p| {
        p["agent_id"] = json!("a1");
        p["agent_type"] = json!("Explore");
        p["agent_transcript_path"] = json!(path.to_string_lossy());
    });
    for _ in 0..3 {
        hook(&env, "user-prompt-submit", "s1", &t, |_| {});
        hook(&env, "stop", "s1", &t, |_| {});
    }
    let store = Store::open(&env.db_path()).unwrap();
    let agent = telemetry::totals(
        store.conn(),
        &Scope::Agent {
            session: "s1".into(),
            agent_id: "a1".into(),
        },
    )
    .unwrap();
    assert_eq!(agent.replies, n);
    let untyped: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM usage_replies WHERE agent_type IS NOT 'Explore'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(untyped, 0);
    assert_eq!(env.log(), "");
}
