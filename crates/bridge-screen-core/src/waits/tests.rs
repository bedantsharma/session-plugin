use serde_json::json;

use super::*;
use crate::repo::RepoInfo;
use crate::store::{EventDraft, Store, read_all};

/// Repo `r` (web) with tasks 1.1 and 1.2, repo `b` (api) with 1.1; sessions w1 (web, on
/// 1.1) and a1 (api).
fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    for (id, name) in [("r", "web"), ("b", "api")] {
        s.register_repo(&RepoInfo {
            id: id.into(),
            name: name.into(),
            root: format!("/code/{name}").into(),
        })
        .unwrap();
        for (nid, parent, kind) in [
            ("G", None, "goal"),
            ("1", Some("G"), "milestone"),
            ("1.1", Some("1"), "task"),
            ("1.2", Some("1"), "task"),
        ] {
            s.append(EventDraft::new(
                id,
                "owner",
                "node_added",
                json!({"id": nid, "parent_id": parent, "kind": kind,
                       "title": format!("{name} {nid}"), "intent": "why"}),
            ))
            .unwrap();
        }
    }
    for (repo, sid) in [("r", "w1"), ("b", "a1")] {
        s.append(EventDraft::new(
            repo,
            sid,
            "session_started",
            json!({"cwd": "/x", "source": "startup", "branch": "feat/x"}),
        ))
        .unwrap();
    }
    s.append(
        EventDraft::new(
            "r",
            "w1",
            "task_claimed",
            json!({"task_id": "1.1", "mode": "confirmed", "confidence": 1.0}),
        )
        .task("1.1"),
    )
    .unwrap();
    s
}

fn sess(s: &Store, id: &str) -> Session {
    session::get(s.conn(), id).unwrap().unwrap()
}

fn open_wait(s: &mut Store, sid: &str, target: &str) -> Result<Opened> {
    let me = sess(s, sid);
    s.write(|tx| open(tx, &me, target, "need it first", &Actor::default()))
}

fn verify_l2(s: &mut Store, repo: &str, task: &str, sha: &str) {
    s.write(|tx| {
        let d = crate::verify::verified_draft(
            repo,
            "a1",
            task,
            2,
            sha,
            vec![],
            "tests",
            crate::verify::By::Agent,
            None,
        )?;
        append_in(tx, d)?;
        clear_for_in(tx, repo, task, sha)
    })
    .unwrap();
}

#[test]
fn targets_parse_as_tasks_or_text() {
    let s = store();
    let t = |text: &str| parse_target(s.conn(), "r", text);
    assert_eq!(
        t("1.2").unwrap(),
        Target::Task {
            repo_id: "r".into(),
            id: "1.2".into()
        }
    );
    assert_eq!(
        t("api:1.1").unwrap(),
        Target::Task {
            repo_id: "b".into(),
            id: "1.1".into()
        }
    );
    assert_eq!(
        t("Sam's API key").unwrap(),
        Target::Text("Sam's API key".into())
    );
    assert_eq!(
        t("note: later").unwrap(),
        Target::Text("note: later".into())
    );
    assert_eq!(
        t("9.9").unwrap_err().to_string(),
        "no task 9.9 in that plan"
    );
    assert_eq!(t("nope:1.1").unwrap_err().to_string(), "no repo named nope");
    assert!(t("1").unwrap_err().to_string().contains("milestone"));
    assert!(t("  ").is_err());
}

#[test]
fn free_text_wait_clears_by_hand_only() {
    let mut s = store();
    let o = open_wait(&mut s, "w1", "Sam's API key").unwrap();
    assert_eq!(o.wait.wait_id, "W1");
    assert!(!o.offer_link);
    verify_l2(&mut s, "r", "1.2", "abc1234");
    assert!(get(s.conn(), "W1").unwrap().unwrap().is_open());
    assert_eq!(cleared_notes(s.conn(), &sess(&s, "w1")).unwrap(), vec![]);
    // Another session can't clear it; its own session can, once.
    let other = s.write(|tx| clear(tx, "W1", Some("a1"), &Actor::default()));
    assert!(
        other
            .unwrap_err()
            .to_string()
            .contains("another session's wait")
    );
    s.write(|tx| clear(tx, "W1", Some("w1"), &Actor::default()))
        .unwrap();
    let w = get(s.conn(), "W1").unwrap().unwrap();
    assert_eq!(w.cleared_why, Some(ClearedWhy::Agent));
    let again = s.write(|tx| clear(tx, "w1", None, &Actor::default()));
    assert!(matches!(again, Err(Error::Stale(_))));
}

#[test]
fn waiting_on_a_done_task_is_refused() {
    let mut s = store();
    verify_l2(&mut s, "b", "1.1", "abc1234def");
    let err = open_wait(&mut s, "w1", "api:1.1").unwrap_err();
    assert_eq!(
        err.to_string(),
        "api:1.1 is already done @abc1234 — merge or rebase to pick it up"
    );
    assert_eq!(of_session(s.conn(), "w1").unwrap(), vec![]);
}

#[test]
fn a_task_target_clears_once_with_its_sha() {
    let mut s = store();
    let o = open_wait(&mut s, "w1", "1.2").unwrap();
    assert_eq!(o.display, "1.2 web 1.2");
    assert!(o.offer_link);
    open_wait(&mut s, "a1", "web:1.2").unwrap();
    verify_l2(&mut s, "r", "1.2", "abc1234");
    for id in ["W1", "W2"] {
        let w = get(s.conn(), id).unwrap().unwrap();
        assert_eq!(
            (w.cleared_why, w.cleared_sha.as_deref()),
            (Some(ClearedWhy::TargetDone), Some("abc1234"))
        );
    }
    // A later verification (or a stale drop) leaves it alone.
    verify_l2(&mut s, "r", "1.2", "fff0000");
    assert_eq!(
        get(s.conn(), "W1").unwrap().unwrap().cleared_sha.as_deref(),
        Some("abc1234")
    );
    let notes = cleared_notes(s.conn(), &sess(&s, "w1")).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].text,
        "bridge-screen: 1.2 web 1.2 is done on `feat/x` @abc1234. Merge or rebase to pick it up."
    );
    assert_eq!(notes[0].state_key, "wait:W1");
    let cleared = read_all(s.conn(), None)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "wait_cleared")
        .count();
    assert_eq!(cleared, 2);
}

#[test]
fn promote_wait_adds_a_plan_link() {
    let mut s = store();
    open_wait(&mut s, "w1", "api:1.1").unwrap();
    open_wait(&mut s, "w1", "the design doc").unwrap();
    let line = s
        .write(|tx| promote(tx, "W1", Some("w1"), &Actor::default()))
        .unwrap();
    assert_eq!(line, "1.1 now depends on api:1.1; the wait W1 is closed.");
    let n = get_node(s.conn(), "r", "1.1").unwrap().unwrap();
    assert_eq!(n.external_deps.len(), 1);
    assert_eq!(
        get(s.conn(), "W1").unwrap().unwrap().cleared_why,
        Some(ClearedWhy::Promoted)
    );
    let err = s
        .write(|tx| promote(tx, "W2", None, &Actor::default()))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("only a task can become a plan link")
    );
    // Already a plan link: no offer.
    let o = open_wait(&mut s, "w1", "api:1.1").unwrap();
    assert!(!o.offer_link);
}

#[test]
fn waiting_session_is_amber() {
    let mut s = store();
    open_wait(&mut s, "w1", "Sam's API key").unwrap();
    let g = crate::view::glance(
        s.conn(),
        &crate::config::Config::default(),
        jiff::Timestamp::now(),
    )
    .unwrap();
    let row = g
        .projects
        .iter()
        .flat_map(|p| &p.sessions)
        .find(|r| r.session_id == "w1")
        .unwrap();
    assert_eq!(row.light, crate::view::Light::Amber);
    assert_eq!(row.light_word, "waiting");
    assert_eq!(row.waiting.len(), 1);
    assert_eq!(row.waiting[0].target, "Sam's API key");
    assert!(
        g.needs_you
            .iter()
            .all(|i| i.session_id.as_deref() != Some("w1")),
        "a wait is not a needs-you item"
    );
}
