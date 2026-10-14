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

fn plan(s: &mut Store, repo: &str) {
    for (id, parent, kind) in [
        ("G", None, "goal"),
        ("1", Some("G"), "milestone"),
        ("1.1", Some("1"), "task"),
        ("1.2", Some("1"), "task"),
    ] {
        script(
            s,
            &[(
                repo,
                "owner",
                0,
                "node_added",
                json!({"id": id, "parent_id": parent, "kind": kind, "title": format!("T{id}"),
                       "intent": "why", "expected_files": []}),
            )],
        );
    }
}

fn req(what: &str, supersedes: Option<&str>) -> LogRequest {
    LogRequest {
        session_id: "s1".into(),
        task: DecisionTask::Active,
        what: what.into(),
        why: format!("because {what}"),
        alternatives: vec![],
        supersedes: supersedes.map(str::to_string),
        actor: crate::store::Actor::default(),
    }
}

fn store_with_task() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    plan(&mut s, "r");
    plan(&mut s, "r2");
    started(&mut s, "s1");
    script(
        &mut s,
        &[
            (
                "r",
                "s1",
                1,
                "task_claimed",
                json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
            ),
            (
                "r2",
                "s2",
                1,
                "session_started",
                json!({"cwd": "/w2", "source": "startup"}),
            ),
        ],
    );
    s
}

fn err(r: Result<Logged>) -> String {
    r.unwrap_err().to_string()
}

#[test]
fn decision_chain_rules() {
    let mut s = store_with_task();
    let d1 = log_decision(&mut s, &req("Use SQLite", None)).unwrap();
    assert_eq!(d1.reply(), "Logged D1.");
    assert_eq!(d1.task_id.as_deref(), Some("1.1"));
    let d2 = log_decision(&mut s, &req("Use Postgres", Some("D1"))).unwrap();
    assert_eq!(d2.reply(), "Logged D2 (supersedes D1).");
    let d3 = log_decision(&mut s, &req("Use CockroachDB", Some("D2"))).unwrap();
    assert_eq!(d3.decision_id, "D3");
    // Superseding an already-superseded decision names the newer one.
    assert_eq!(
        err(log_decision(&mut s, &req("Use files", Some("D1")))),
        "D1 was already superseded by D2 — supersede D2 instead"
    );
    // Unknown here, or from another repo.
    let mut other = req("Other repo", None);
    other.session_id = "s2".into();
    other.task = DecisionTask::RepoWide;
    assert_eq!(log_decision(&mut s, &other).unwrap().decision_id, "D1");
    assert_eq!(
        err(log_decision(&mut s, &req("Use files", Some("D9")))),
        "no decision D9 in this repo"
    );
    let mut cross = req("Cross", Some("D3"));
    cross.session_id = "s2".into();
    cross.task = DecisionTask::RepoWide;
    assert_eq!(
        err(log_decision(&mut s, &cross)),
        "no decision D3 in this repo"
    );
    // Itself: the same `what` merges into D3, which can't supersede itself.
    assert_eq!(
        err(log_decision(&mut s, &req("use cockroachdb.", Some("D3")))),
        "D3 can't supersede itself"
    );
    let ids: Vec<String> = chain(s.conn(), "r", "D2")
        .unwrap()
        .into_iter()
        .map(|d| d.decision_id)
        .collect();
    assert_eq!(ids, vec!["D1", "D2", "D3"]);
    // Refusals recorded nothing.
    assert_eq!(list(s.conn(), "r", None, true).unwrap().len(), 3);
    assert_eq!(
        err(log_decision(&mut s, &req("  ", None))),
        "what and why are both required"
    );
}

#[test]
fn duplicates_merge_and_rebuild_gives_the_same_rows() {
    let mut s = store_with_task();
    let mut first = req("Use SQLite", None);
    first.alternatives = vec!["Postgres".into(), "JSON".into()];
    log_decision(&mut s, &first).unwrap();
    let mut again = req("  use   SQLITE! ", None);
    again.why = "WAL is enough".into();
    again.alternatives = vec!["JSON".into(), "sled".into(), " ".into()];
    let merged = log_decision(&mut s, &again).unwrap();
    assert_eq!(merged.reply(), "Merged into D1 (same decision on 1.1).");
    // The same `what` on another task, or repo-wide, is a different decision.
    let mut other_task = req("Use SQLite", None);
    other_task.task = DecisionTask::Task("1.2".into());
    assert_eq!(log_decision(&mut s, &other_task).unwrap().decision_id, "D2");
    let mut wide = req("Use SQLite", None);
    wide.task = DecisionTask::RepoWide;
    let repo_wide = log_decision(&mut s, &wide).unwrap();
    assert_eq!(repo_wide.task_id, None);
    assert_eq!(repo_wide.decision_id, "D3");
    let rows = list(s.conn(), "r", Some("1.1"), true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].why, "WAL is enough");
    assert_eq!(rows[0].alternatives, vec!["Postgres", "JSON", "sled"]);
    assert_eq!(rows[0].merged_count, 2);
    // Long text is cut, not refused.
    let mut long = req(&"w".repeat(300), None);
    long.why = "y".repeat(900);
    long.alternatives = (0..9).map(|i| format!("alt{i}")).collect();
    let id = log_decision(&mut s, &long).unwrap().decision_id;
    let cut = get(s.conn(), "r", &id).unwrap().unwrap();
    assert_eq!(
        (
            cut.what.chars().count(),
            cut.why.chars().count(),
            cut.alternatives.len()
        ),
        (MAX_WHAT, MAX_WHY, MAX_ALTERNATIVES)
    );
    let before = list(s.conn(), "r", None, true).unwrap();
    s.rebuild().unwrap();
    assert_eq!(list(s.conn(), "r", None, true).unwrap(), before);
}

#[test]
fn a_merge_that_supersedes_links_both_ways() {
    let mut s = store_with_task();
    log_decision(&mut s, &req("Use SQLite", None)).unwrap();
    log_decision(&mut s, &req("Use Postgres", None)).unwrap();
    let merged = log_decision(&mut s, &req("use postgres", Some("D1"))).unwrap();
    assert_eq!(
        merged.reply(),
        "Merged into D2 (same decision on 1.1); it supersedes D1."
    );
    let d2 = get(s.conn(), "r", "D2").unwrap().unwrap();
    assert_eq!(d2.supersedes.as_deref(), Some("D1"));
    let ids = |s: &Store, id: &str| -> Vec<String> {
        chain(s.conn(), "r", id)
            .unwrap()
            .into_iter()
            .map(|d| d.decision_id)
            .collect()
    };
    assert_eq!(ids(&s, "D2"), vec!["D1", "D2"]);
    assert_eq!(ids(&s, "D1"), vec!["D1", "D2"]);
    // The same again merges once more.
    let again = log_decision(&mut s, &req("Use Postgres", Some("D1"))).unwrap();
    assert_eq!(again.decision_id, "D2");
    // A merge can't make one decision supersede two.
    log_decision(&mut s, &req("Use files", None)).unwrap();
    assert_eq!(
        err(log_decision(&mut s, &req("Use Postgres", Some("D3")))),
        "D2 (the same decision) already supersedes D1 — log a new decision to supersede D3"
    );
    let before = list(s.conn(), "r", None, true).unwrap();
    s.rebuild().unwrap();
    assert_eq!(list(s.conn(), "r", None, true).unwrap(), before);
}

#[test]
fn ids_stay_unique_under_concurrent_writers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    {
        let mut s = Store::open(&path).unwrap();
        plan(&mut s, "r");
        started(&mut s, "s1");
        started(&mut s, "s2");
    }
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = ["s1", "s2"]
        .into_iter()
        .map(|sid| {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut s = Store::open(&path).unwrap();
                barrier.wait();
                for i in 0..20 {
                    let mut r = req(&format!("{sid} decision {i}"), None);
                    r.session_id = sid.into();
                    r.task = DecisionTask::RepoWide;
                    log_decision(&mut s, &r).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let s = Store::open(&path).unwrap();
    let ids: Vec<String> = list(s.conn(), "r", None, true)
        .unwrap()
        .into_iter()
        .map(|d| d.decision_id)
        .collect();
    let want: Vec<String> = (1..=40).map(|n| format!("D{n}")).collect();
    assert_eq!(ids, want);
}
