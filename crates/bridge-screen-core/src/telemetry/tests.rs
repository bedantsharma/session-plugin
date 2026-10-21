use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::store::{EventDraft, Store, append_in_at};

fn t0() -> Timestamp {
    "2026-10-14T09:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> String {
    (t0() + SignedDuration::from_secs(secs)).to_string()
}

fn script(s: &mut Store, events: &[(&str, i64, &str, Value)]) {
    s.write(|tx| {
        for (session, secs, kind, data) in events {
            let draft = EventDraft::new("r", *session, *kind, data.clone());
            append_in_at(tx, draft, t0() + SignedDuration::from_secs(*secs))?;
        }
        Ok(())
    })
    .unwrap();
}

/// A reply at `secs` with `output` tokens (and 10 input, 100 cache read).
fn reply(id: &str, secs: i64, output: u64) -> Value {
    json!({"message_id": id, "model": "claude-opus-5-5", "ts": at(secs), "input": 10,
           "output": output, "cache_read": 100, "cache_write_5m": 0, "cache_write_1h": 0})
}

#[allow(clippy::needless_pass_by_value)] // reads better at the call sites
fn usage(path: &str, offset: u64, replies: Vec<Value>) -> Value {
    json!({"path": path, "offset_after": offset, "replies": replies})
}

/// s1: unit 1 (0–60) on task 1.1; unit 2 (100–200) on 1.1 with detour D1 open; unit 3
/// (300–) on 1.2, still open.
fn session(s: &mut Store) {
    script(
        s,
        &[
            (
                "s1",
                0,
                "session_started",
                json!({"cwd": "/a", "source": "startup"}),
            ),
            (
                "s1",
                0,
                "task_claimed",
                json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
            ),
            ("s1", 1, "prompt", json!({"chars": 3})),
            ("s1", 60, "unit_stopped", json!({"unit_no": 1})),
            ("s1", 100, "prompt", json!({"chars": 3})),
            (
                "s1",
                110,
                "detour_opened",
                json!({"detour_id": "D1", "reason": "broken build", "blocks": "1.1",
                       "resume_cue": "rerun"}),
            ),
            ("s1", 200, "unit_stopped", json!({"unit_no": 2})),
            (
                "s1",
                250,
                "detour_closed",
                json!({"detour_id": "D1", "outcome": "done"}),
            ),
            (
                "s1",
                260,
                "task_claimed",
                json!({"task_id": "1.2", "mode": "auto", "confidence": 0.9}),
            ),
            ("s1", 300, "prompt", json!({"chars": 3})),
        ],
    );
}

fn row(s: &Store, id: &str) -> (Option<i64>, Option<String>, Option<String>, i64) {
    s.conn()
        .query_row(
            "SELECT unit_no, task_id, detour_id, output FROM usage_replies WHERE message_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
}

fn cursor(s: &Store, path: &str) -> i64 {
    s.conn()
        .query_row(
            "SELECT offset FROM transcript_cursors WHERE path = ?1",
            [path],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn replies_are_attributed_by_timestamp() {
    let mut s = Store::open_in_memory().unwrap();
    session(&mut s);
    script(
        &mut s,
        &[(
            "s1",
            400,
            "usage",
            usage(
                "/t/s1.jsonl",
                900,
                vec![
                    reply("before", -5, 1),
                    reply("u1", 30, 2),
                    reply("between", 80, 3),
                    reply("u2", 150, 4),
                    reply("u3", 350, 5),
                ],
            ),
        )],
    );
    assert_eq!(row(&s, "before"), (None, None, None, 1));
    assert_eq!(row(&s, "u1"), (Some(1), Some("1.1".into()), None, 2));
    assert_eq!(row(&s, "between"), (None, None, None, 3));
    assert_eq!(
        row(&s, "u2"),
        (Some(2), Some("1.1".into()), Some("D1".into()), 4)
    );
    assert_eq!(row(&s, "u3"), (Some(3), Some("1.2".into()), None, 5));
}

#[test]
fn a_reply_seen_again_is_ignored() {
    let mut s = Store::open_in_memory().unwrap();
    session(&mut s);
    script(
        &mut s,
        &[
            (
                "s1",
                400,
                "usage",
                usage("/t", 10, vec![reply("m1", 30, 7)]),
            ),
            (
                "s1",
                401,
                "usage",
                usage("/t", 20, vec![reply("m1", 150, 99), reply("m2", 30, 1)]),
            ),
        ],
    );
    assert_eq!(row(&s, "m1"), (Some(1), Some("1.1".into()), None, 7));
    let t = totals(s.conn(), &Scope::Session("s1".into())).unwrap();
    assert_eq!((t.replies, t.sum.output), (2, 8));
}

#[test]
fn cursors_move_forward_unless_the_read_restarted() {
    let mut s = Store::open_in_memory().unwrap();
    session(&mut s);
    let restarted = json!({"path": "/t", "offset_after": 40, "restarted": true, "replies": []});
    script(
        &mut s,
        &[
            ("s1", 400, "usage", usage("/t", 100, vec![])),
            ("s1", 401, "usage", usage("/t", 70, vec![])),
        ],
    );
    // An older read finishing late never moves the cursor back.
    assert_eq!(cursor(&s, "/t"), 100);
    script(&mut s, &[("s1", 402, "usage", restarted)]);
    assert_eq!(cursor(&s, "/t"), 40);
    script(&mut s, &[("s1", 403, "usage", usage("/u", 5, vec![]))]);
    assert_eq!((cursor(&s, "/t"), cursor(&s, "/u")), (40, 5));
}

#[test]
fn totals_per_scope() {
    let mut s = Store::open_in_memory().unwrap();
    session(&mut s);
    let mut haiku = reply("h1", 150, 50);
    haiku["model"] = json!("claude-haiku-5-5");
    script(
        &mut s,
        &[
            (
                "s1",
                400,
                "usage",
                usage(
                    "/t",
                    10,
                    vec![reply("a", 30, 1), reply("b", 150, 2), reply("c", 350, 4)],
                ),
            ),
            (
                "s1",
                401,
                "usage",
                json!({"path": "/sub", "offset_after": 5, "agent_id": "ag1",
                       "agent_type": "Explore", "replies": [haiku]}),
            ),
        ],
    );
    let out = |scope: Scope| totals(s.conn(), &scope).unwrap().sum.output;
    // The task total includes its detour's replies.
    assert_eq!(
        out(Scope::Task {
            repo: "r".into(),
            id: "1.1".into()
        }),
        1 + 2 + 50
    );
    assert_eq!(out(Scope::Detour("D1".into())), 2 + 50);
    assert_eq!(out(Scope::Session("s1".into())), 57);
    assert_eq!(out(Scope::Repo("r".into())), 57);
    assert_eq!(
        out(Scope::Agent {
            session: "s1".into(),
            agent_id: "ag1".into()
        }),
        50
    );
    let t = totals(s.conn(), &Scope::Repo("r".into())).unwrap();
    assert_eq!(t.by_model["claude-haiku-5-5"].output, 50);
    assert_eq!(t.by_model["claude-opus-5-5"].cache_read, 300);
    assert_eq!(t.sum.total(), 4 * 10 + 57 + 4 * 100);
    let agents = task_by_agent(s.conn(), "r", "1.1").unwrap();
    assert_eq!(agents[&Some("Explore".to_string())].output, 50);
    assert_eq!(agents[&None].output, 3);
    assert_eq!(
        main_model(s.conn(), "r", "1.1").unwrap().as_deref(),
        Some("claude-haiku-5-5")
    );
    let per_unit = session_by_unit(s.conn(), "s1").unwrap();
    assert_eq!(per_unit[&1], 10 + 1 + 100);
}

#[test]
fn readable_numbers() {
    assert_eq!(tokens_text(950), "950");
    assert_eq!(tokens_text(1_290), "1.2k");
    assert_eq!(tokens_text(12_400), "12k");
    assert_eq!(tokens_text(1_290_000), "1.2M");
    assert_eq!(tokens_text(3_000_000), "3M");
    assert_eq!(usd_text(4.1), "$4.10");
    assert_eq!(usd_text(0.004), "<$0.01");
    assert_eq!(usd_text(0.0), "$0.00");
    assert_eq!(usd_text(1240.4), "$1,240");
    assert_eq!(model_text("claude-opus-5-5"), "Opus 5.5");
    assert_eq!(model_text("claude-haiku-4-5-20251001"), "Haiku 4.5");
    assert_eq!(model_text("gpt-x"), "gpt-x");
}

/// A transcript line for reply `id` (the same reply may be written on several lines).
fn line(id: &str, ts: &str, output: u64) -> String {
    format!(
        r#"{{"parentUuid":"p","isSidechain":false,"type":"assistant","requestId":"req_{id}","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-opus-5-5","role":"assistant","content":[{{"type":"text","text":"SECRET reply text"}}],"usage":{{"input_tokens":3,"cache_creation_input_tokens":30,"cache_read_input_tokens":200,"output_tokens":{output},"output_tokens_details":{{"thinking_tokens":0}},"service_tier":"standard","cache_creation":{{"ephemeral_1h_input_tokens":20,"ephemeral_5m_input_tokens":10}}}}}}}}"#
    ) + "\n"
}

fn write(path: &std::path::Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

fn append(path: &std::path::Path, text: &str) {
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[test]
fn a_reply_written_on_several_lines_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let ts = at(5);
    write(
        &path,
        &[
            line("m1", &ts, 7),
            line("m1", &ts, 7),
            line("m1", &ts, 7),
            line("m2", &ts, 1),
        ]
        .concat(),
    );
    let out = read_new(&path, 0, MAX_HOOK_BYTES).unwrap();
    let ids: Vec<&str> = out.replies.iter().map(|r| r.message_id.as_str()).collect();
    assert_eq!(ids, ["m1", "m2"]);
    let r = &out.replies[0];
    assert_eq!(
        (
            r.input,
            r.output,
            r.cache_read,
            r.cache_write_5m,
            r.cache_write_1h
        ),
        (3, 7, 200, 10, 20)
    );
    assert_eq!(
        (r.model.as_str(), r.ts.as_str()),
        ("claude-opus-5-5", ts.as_str())
    );
    assert!(!out.more);
}

#[test]
fn only_whole_lines_are_read_and_a_shrunk_file_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let full = line("m1", &at(5), 7);
    let (head, tail) = full.split_at(40);
    write(&path, head);
    let out = read_new(&path, 0, MAX_HOOK_BYTES).unwrap();
    assert_eq!(
        (out.replies.len(), out.offset_after, out.more),
        (0, 0, true)
    );
    append(&path, tail);
    let out = read_new(&path, 0, MAX_HOOK_BYTES).unwrap();
    assert_eq!(out.replies.len(), 1);
    assert_eq!(out.offset_after, full.len() as u64);
    // The file was replaced by a shorter one: read again from the start.
    write(&path, &line("m9", &at(6), 1));
    let out = read_new(&path, full.len() as u64 + 500, MAX_HOOK_BYTES).unwrap();
    assert!(out.restarted);
    assert_eq!(out.replies[0].message_id, "m9");
}

#[test]
fn reads_are_bounded_and_long_lines_never_stall() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let one = line("m1", &at(5), 7);
    let text: String = (0..10).map(|i| line(&format!("m{i}"), &at(5), 1)).collect();
    write(&path, &text);
    let max = one.len() as u64 * 3 + 5;
    let out = read_new(&path, 0, max).unwrap();
    assert_eq!(out.replies.len(), 3);
    assert!(out.more);
    let rest = read_to_end(&path, out.offset_after).unwrap();
    assert_eq!(rest.replies.len(), 7);
    assert!(!rest.more);
    // A line longer than the slice: extended to its end (within 2 × max).
    let long = format!("{{\"type\":\"user\",\"pad\":\"{}\"}}\n", "x".repeat(150));
    write(&path, &(long.clone() + &one));
    let out = read_new(&path, 0, 100).unwrap();
    assert_eq!(out.offset_after, long.len() as u64);
    // Longer than 2 × max: skipped, and the cursor still moves.
    let out = read_new(&path, 0, 50).unwrap();
    assert!(out.skipped_long_line);
    assert_eq!(out.offset_after, 100);
    let next = read_new(&path, out.offset_after, 1000).unwrap();
    assert_eq!(next.replies.len(), 1);
}

#[test]
fn other_lines_are_ignored_and_a_missing_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let text = [
        r#"{"type":"user","message":{"role":"user","content":"say \"usage\" and \"assistant\""}}"#,
        r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","content":"no usage"}}"#,
        r#"{"type":"assistant","message":{"id":"m0","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
        r#"not json at all "usage" "assistant""#,
        r#"{"type":"assistant","timestamp":"2026-10-14T09:00:01Z","message":{"id":"m2","usage":{"output_tokens":4,"cache_creation_input_tokens":9}}}"#,
    ]
    .join("\n")
        + "\n";
    write(&path, &text);
    let out = read_new(&path, 0, MAX_HOOK_BYTES).unwrap();
    assert_eq!(out.replies.len(), 1);
    let r = &out.replies[0];
    // Without the 5m/1h split, a cache write is the default 5-minute one.
    assert_eq!(
        (r.model.as_str(), r.output, r.cache_write_5m),
        ("unknown", 4, 9)
    );
    let missing = read_new(&dir.path().join("nope.jsonl"), 7, MAX_HOOK_BYTES).unwrap();
    assert_eq!(
        missing,
        ReadOut {
            offset_after: 7,
            ..ReadOut::default()
        }
    );
}

#[test]
fn pending_skips_replies_already_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let mut s = Store::open_in_memory().unwrap();
    session(&mut s);
    write(
        &path,
        &[line("m1", &at(30), 7), line("m1", &at(30), 7)].concat(),
    );
    let first = pending(s.conn(), &path, Some(MAX_HOOK_BYTES)).unwrap();
    let usage = first.usage.unwrap();
    assert_eq!(usage.replies.len(), 1);
    script(
        &mut s,
        &[("s1", 400, "usage", serde_json::to_value(&usage).unwrap())],
    );
    // The reply's other lines arrive in the next read: nothing new.
    append(
        &path,
        &[line("m1", &at(30), 7), line("m2", &at(30), 1)].concat(),
    );
    let next = pending(s.conn(), &path, Some(MAX_HOOK_BYTES))
        .unwrap()
        .usage
        .unwrap();
    assert_eq!(next.replies.len(), 1);
    assert_eq!(next.replies[0].message_id, "m2");
    // Nothing new and the cursor at the end: nothing to record.
    script(
        &mut s,
        &[("s1", 401, "usage", serde_json::to_value(&next).unwrap())],
    );
    assert_eq!(
        pending(s.conn(), &path, Some(MAX_HOOK_BYTES))
            .unwrap()
            .usage,
        None
    );
    let t = totals(s.conn(), &Scope::Session("s1".into())).unwrap();
    assert_eq!((t.replies, t.sum.output), (2, 8));
}

fn price(input: f64, output: f64) -> crate::config::Price {
    crate::config::Price {
        input,
        output,
        cache_read: input / 10.0,
        cache_write_5m: input * 1.25,
        cache_write_1h: input * 2.0,
    }
}

fn tokens(rows: &[(&str, Counts)]) -> Tokens {
    let mut t = Tokens::default();
    for (model, c) in rows {
        t.replies += 1;
        t.sum.add(c);
        t.by_model.insert((*model).to_string(), *c);
    }
    t
}

#[test]
fn unknown_model_cost_is_unknown() {
    let prices =
        std::collections::BTreeMap::from([("claude-opus-5".to_string(), price(5.0, 25.0))]);
    let million = Counts {
        output: 1_000_000,
        ..Counts::default()
    };
    // Only an unpriced model: the cost is unknown, never $0.
    let c = cost(&tokens(&[("gpt-like-9", million)]), &prices);
    assert_eq!(c.usd, None);
    assert_eq!(c.unpriced_models, ["gpt-like-9"]);
    assert_eq!(cost_text(&c), "cost unknown");
    // Priced and unpriced: a lower bound, said so.
    let c = cost(
        &tokens(&[("claude-opus-5-1", million), ("gpt-like-9", million)]),
        &prices,
    );
    assert_eq!(c.usd, Some(25.0));
    assert!(!c.complete());
    assert_eq!(cost_text(&c), "≥ $25.00 (some models unpriced)");
    // Known models still sum.
    let c = cost(
        &tokens(&[("claude-opus-5", million), ("claude-opus-5-1", million)]),
        &prices,
    );
    assert_eq!((c.usd, c.complete()), (Some(50.0), true));
    assert_eq!(cost_text(&c), "$50.00");
    // Nothing spent at all: $0, which is true.
    assert_eq!(cost(&Tokens::default(), &prices).usd, Some(0.0));
}

#[test]
fn cache_writes_are_priced_by_duration() {
    let p = price(4.0, 20.0);
    let w5 = Counts {
        cache_write_5m: 1_000_000,
        ..Counts::default()
    };
    let w1 = Counts {
        cache_write_1h: 1_000_000,
        ..Counts::default()
    };
    let read = Counts {
        cache_read: 2_000_000,
        input: 500_000,
        ..Counts::default()
    };
    assert!((priced(&w5, &p) - 5.0).abs() < 1e-9);
    assert!((priced(&w1, &p) - 8.0).abs() < 1e-9);
    assert!((priced(&read, &p) - (0.8 + 2.0)).abs() < 1e-9);
}

/// Lines Claude Code may write: a lone surrogate escape (text cut mid-emoji), a byte
/// that isn't UTF-8 inside the message text, and a CRLF ending. Each reply still counts.
#[test]
fn odd_lines_still_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let good = line("m1", &at(5), 7);
    let surrogate = good.replace("SECRET reply text", "cut \\ud83d");
    let mut bad = good.replace("m1", "m2").into_bytes();
    let pos = bad.windows(6).position(|w| w == b"SECRET").unwrap();
    bad[pos] = 0xff;
    let mut all = surrogate.replace("m1", "m3").into_bytes();
    all.extend(bad);
    all.extend(good.replace('\n', "\r\n").replace("m1", "m4").into_bytes());
    std::fs::write(&path, all).unwrap();
    let out = read_new(&path, 0, MAX_HOOK_BYTES).unwrap();
    let ids: Vec<&str> = out.replies.iter().map(|r| r.message_id.as_str()).collect();
    assert_eq!(ids, ["m3", "m2", "m4"]);
}
