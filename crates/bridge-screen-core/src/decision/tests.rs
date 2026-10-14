use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::store::{EventDraft, Store, append_in_at};

fn t0() -> Timestamp {
    "2026-10-14T09:00:00Z".parse().unwrap()
}

fn script(s: &mut Store, events: &[(&str, &str, i64, &str, Value)]) {
    s.write(|tx| {
        for (repo, session, secs, kind, data) in events {
            let draft = EventDraft::new(*repo, *session, *kind, data.clone());
            append_in_at(tx, draft, t0() + SignedDuration::from_secs(*secs))?;
        }
        Ok(())
    })
    .unwrap();
}

fn started(s: &mut Store, session: &str) {
    script(
        s,
        &[(
            "r",
            session,
            0,
            "session_started",
            json!({"cwd": "/w", "source": "startup"}),
        )],
    );
}

fn capture(s: &Store, session: &str) -> Option<String> {
    crate::session::get(s.conn(), session)
        .unwrap()
        .unwrap()
        .decision_capture
}

#[test]
fn keys_ignore_case_spacing_and_trailing_punctuation() {
    assert_eq!(
        what_key("  Use  SQLite\tfor state. "),
        "use sqlite for state"
    );
    assert_eq!(what_key("Use sqlite for state!?"), "use sqlite for state");
    assert_eq!(what_key("v1.2 stays"), "v1.2 stays");
    assert_eq!(clip("  héllo wörld  ", 5), "héllo");
    assert_eq!(clip("ab", 5), "ab");
}

#[test]
fn a_repeated_id_merges_and_supersedes_link_both_ways() {
    let mut s = Store::open_in_memory().unwrap();
    started(&mut s, "s1");
    script(
        &mut s,
        &[
            (
                "r",
                "s1",
                1,
                "decision",
                json!({"decision_id": "D1", "task_id": "1.1", "what": "Use SQLite",
                       "why": "one file", "alternatives": ["Postgres", "JSON files"]}),
            ),
            (
                "r",
                "s1",
                2,
                "decision",
                json!({"decision_id": "D1", "task_id": "1.1", "what": "use sqlite.",
                       "why": "one file, WAL", "alternatives": ["JSON files", "sled"]}),
            ),
            (
                "r",
                "s1",
                3,
                "decision",
                json!({"decision_id": "D2", "task_id": "1.1", "what": "Use Postgres",
                       "why": "scale", "alternatives": [], "supersedes": "D1"}),
            ),
        ],
    );
    let d1 = get(s.conn(), "r", "D1").unwrap().unwrap();
    assert_eq!(d1.what, "Use SQLite", "the first wording stays");
    assert_eq!(d1.why, "one file, WAL");
    assert_eq!(d1.alternatives, vec!["Postgres", "JSON files", "sled"]);
    assert_eq!(d1.merged_count, 2);
    assert_eq!(d1.superseded_by.as_deref(), Some("D2"));
    assert_ne!(d1.ts, d1.updated_ts);
    let live: Vec<String> = list(s.conn(), "r", Some("1.1"), false)
        .unwrap()
        .into_iter()
        .map(|d| d.decision_id)
        .collect();
    assert_eq!(live, vec!["D2"]);
    assert_eq!(list(s.conn(), "r", None, true).unwrap().len(), 2);
    let ids: Vec<String> = chain(s.conn(), "r", "D1")
        .unwrap()
        .into_iter()
        .map(|d| d.decision_id)
        .collect();
    assert_eq!(ids, vec!["D1", "D2"]);
    assert_eq!(chain(s.conn(), "r", "D9").unwrap(), Vec::new());
}

#[test]
fn the_capture_flag_is_set_and_cleared() {
    let mut s = Store::open_in_memory().unwrap();
    started(&mut s, "s1");
    script(&mut s, &[("r", "s1", 1, "precompact", json!({}))]);
    assert_eq!(capture(&s, "s1").as_deref(), Some("precompact"));
    script(&mut s, &[("r", "s1", 2, "decision_asked", json!({}))]);
    assert_eq!(capture(&s, "s1"), None);
    script(
        &mut s,
        &[(
            "r",
            "s1",
            3,
            "claimed_done",
            json!({"task_id": "1.1", "evidence": "x"}),
        )],
    );
    assert_eq!(capture(&s, "s1").as_deref(), Some("claimed_done"));
    // A later compaction takes over; a decision clears it.
    script(&mut s, &[("r", "s1", 4, "precompact", json!({}))]);
    assert_eq!(capture(&s, "s1").as_deref(), Some("precompact"));
    script(
        &mut s,
        &[(
            "r",
            "s1",
            5,
            "decision",
            json!({"decision_id": "D1", "what": "x", "why": "y"}),
        )],
    );
    assert_eq!(capture(&s, "s1"), None);
    assert_eq!(by_session(s.conn(), "s1").unwrap().len(), 1);
}
