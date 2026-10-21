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
