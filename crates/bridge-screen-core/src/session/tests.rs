use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::{SessionState, current_unit, get, list_live, task_time, units};
use crate::store::{EventDraft, Store, append_in_at};

const T0: &str = "2026-10-13T09:00:00Z";

fn at(secs: i64) -> Timestamp {
    T0.parse::<Timestamp>().unwrap() + SignedDuration::from_secs(secs)
}

fn ev(store: &mut Store, session: &str, kind: &str, data: serde_json::Value, secs: i64) {
    store
        .write(|tx| append_in_at(tx, EventDraft::new("r", session, kind, data), at(secs)))
        .unwrap();
}

fn start(store: &mut Store, session: &str, secs: i64) {
    ev(
        store,
        session,
        "session_started",
        json!({"cwd": "/repo", "branch": "main", "source": "startup", "model": "m",
               "transcript_path": "/t.jsonl"}),
        secs,
    );
}

fn prompt(store: &mut Store, session: &str, secs: i64) {
    ev(
        store,
        session,
        "prompt",
        json!({"prompt_id": format!("p{secs}"), "chars": 10}),
        secs,
    );
}

fn stop(store: &mut Store, session: &str, secs: i64) {
    ev(
        store,
        session,
        "unit_stopped",
        json!({"unit_no": null, "summary": "done"}),
        secs,
    );
}

fn changed(store: &mut Store, session: &str, files: &[&str], secs: i64) {
    ev(
        store,
        session,
        "file_changed",
        json!({"tool": "Edit", "files": files}),
        secs,
    );
}

#[test]
fn unit_lifecycle_and_active_time() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    let started = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(started.state, SessionState::Idle);
    assert_eq!(started.repo_id, "r");
    assert_eq!(started.branch.as_deref(), Some("main"));

    prompt(&mut s, "a", 10);
    assert_eq!(
        get(s.conn(), "a").unwrap().unwrap().state,
        SessionState::Working
    );
    changed(&mut s, "a", &["src/b.rs", "src/a.rs"], 20);
    changed(&mut s, "a", &["src/a.rs", "src/c.rs"], 30);
    let open = current_unit(s.conn(), "a").unwrap().unwrap();
    assert_eq!(open.unit_no, 1);
    assert_eq!(open.prompt_id.as_deref(), Some("p10"));
    assert_eq!(open.stopped_at, None);

    stop(&mut s, "a", 130);
    assert_eq!(current_unit(s.conn(), "a").unwrap(), None);
    let all = units(s.conn(), "a").unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].active_secs, 120);
    assert_eq!(all[0].files, vec!["src/b.rs", "src/a.rs", "src/c.rs"]);
    let session = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.active_secs, 120);
    assert_eq!(session.unit_count, 1);
    assert_eq!(session.last_event_at, at(130).to_string());
}

#[test]
fn waits_and_idle_gaps_are_not_active_time() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    prompt(&mut s, "a", 0);
    ev(
        &mut s,
        "a",
        "needs_owner",
        json!({"kind": "permission_prompt"}),
        10,
    );
    assert_eq!(
        get(s.conn(), "a").unwrap().unwrap().state,
        SessionState::NeedsOwner
    );
    ev(
        &mut s,
        "a",
        "tool_intent",
        json!({"tool": "Edit", "files": ["x.rs"]}),
        610,
    );
    assert_eq!(
        get(s.conn(), "a").unwrap().unwrap().state,
        SessionState::Working
    );
    assert_eq!(
        current_unit(s.conn(), "a").unwrap().unwrap().waited_secs,
        600
    );
    stop(&mut s, "a", 620);
    assert_eq!(units(s.conn(), "a").unwrap()[0].active_secs, 20);

    // An hour of nothing between units counts for nothing.
    prompt(&mut s, "a", 620 + 3600);
    stop(&mut s, "a", 620 + 3600 + 30);
    let session = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(session.active_secs, 50);
    assert_eq!(session.unit_count, 2);
}

#[test]
fn interrupted_unit_does_not_count_the_wait_for_the_next_prompt() {
    // Esc interrupts the agent: no Stop. Claude Code then reports idle_prompt while it waits
    // for the owner; the hour before the next prompt is not agent time.
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    prompt(&mut s, "a", 0);
    ev(
        &mut s,
        "a",
        "needs_owner",
        json!({"kind": "idle_prompt"}),
        100,
    );
    assert_eq!(
        get(s.conn(), "a").unwrap().unwrap().state,
        SessionState::NeedsOwner
    );
    prompt(&mut s, "a", 3700);
    let all = units(s.conn(), "a").unwrap();
    assert_eq!(all[0].active_secs, 100);
    assert_eq!(all[0].waited_secs, 3600);
}

#[test]
fn missing_stop_is_closed_by_next_prompt() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    prompt(&mut s, "a", 0);
    prompt(&mut s, "a", 40);
    let all = units(s.conn(), "a").unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].stopped_at, Some(at(40).to_string()));
    assert_eq!(all[0].active_secs, 40);
    assert_eq!(all[1].unit_no, 2);
    assert_eq!(all[1].stopped_at, None);
    let session = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(session.state, SessionState::Working);
    assert_eq!(session.unit_count, 1);
}

#[test]
fn session_end_closes_open_unit() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    prompt(&mut s, "a", 0);
    ev(
        &mut s,
        "a",
        "session_ended",
        json!({"reason": "logout"}),
        90,
    );
    let session = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(session.state, SessionState::Ended);
    assert_eq!(session.ended_at, Some(at(90).to_string()));
    assert_eq!(session.end_reason.as_deref(), Some("logout"));
    assert_eq!(session.active_secs, 90);
    assert_eq!(current_unit(s.conn(), "a").unwrap(), None);
    assert_eq!(list_live(s.conn(), None).unwrap(), vec![]);
}

#[test]
fn resume_keeps_counters() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    prompt(&mut s, "a", 0);
    stop(&mut s, "a", 30);
    ev(&mut s, "a", "session_ended", json!({"reason": "other"}), 40);
    ev(
        &mut s,
        "a",
        "session_started",
        json!({"cwd": "/repo", "source": "resume"}),
        100,
    );
    let session = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.source, "resume");
    assert_eq!(session.started_at, at(0).to_string());
    assert_eq!(session.unit_count, 1);
    assert_eq!(session.active_secs, 30);
    assert_eq!(session.ended_at, None);
    assert_eq!(
        session.model.as_deref(),
        Some("m"),
        "model kept when omitted"
    );
}

#[test]
fn task_time_accumulates_per_task() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    // The projector copies the session's active task onto each new unit.
    s.conn()
        .execute(
            "UPDATE sessions SET active_task = '1.1' WHERE session_id = 'a'",
            [],
        )
        .unwrap();
    prompt(&mut s, "a", 0);
    stop(&mut s, "a", 60);
    prompt(&mut s, "a", 100);
    stop(&mut s, "a", 130);
    s.conn()
        .execute(
            "UPDATE sessions SET active_task = NULL WHERE session_id = 'a'",
            [],
        )
        .unwrap();
    prompt(&mut s, "a", 200);
    stop(&mut s, "a", 260);
    assert_eq!(task_time(s.conn(), "r", "1.1").unwrap(), Some((90, 2)));
    assert_eq!(get(s.conn(), "a").unwrap().unwrap().active_secs, 150);
}

#[test]
fn list_live_is_newest_first_and_per_repo() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "old", 0);
    start(&mut s, "new", 50);
    s.write(|tx| {
        append_in_at(
            tx,
            EventDraft::new(
                "other",
                "elsewhere",
                "session_started",
                json!({"cwd": "/o"}),
            ),
            at(60),
        )
    })
    .unwrap();
    let ids = |repo: Option<&str>| -> Vec<String> {
        list_live(s.conn(), repo)
            .unwrap()
            .into_iter()
            .map(|x| x.session_id)
            .collect()
    };
    assert_eq!(ids(Some("r")), vec!["new", "old"]);
    assert_eq!(ids(None), vec!["elsewhere", "new", "old"]);
}

#[test]
fn events_for_unknown_sessions_change_nothing() {
    let mut s = Store::open_in_memory().unwrap();
    prompt(&mut s, "ghost", 0);
    assert_eq!(get(s.conn(), "ghost").unwrap(), None);
    assert_eq!(units(s.conn(), "ghost").unwrap(), vec![]);
}

#[test]
fn late_events_after_session_end_do_not_revive_it() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    prompt(&mut s, "a", 0);
    stop(&mut s, "a", 30);
    ev(&mut s, "a", "session_ended", json!({"reason": "other"}), 40);
    ev(
        &mut s,
        "a",
        "needs_owner",
        json!({"kind": "idle_prompt"}),
        50,
    );
    stop(&mut s, "a", 60);
    changed(&mut s, "a", &["x.rs"], 70);
    let session = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(session.state, SessionState::Ended);
    assert_eq!(session.ended_at, Some(at(40).to_string()));
    assert_eq!(list_live(s.conn(), None).unwrap(), vec![]);

    // A new prompt does mean the session is alive again.
    prompt(&mut s, "a", 100);
    let session = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(session.state, SessionState::Working);
    assert_eq!(session.ended_at, None);
}

#[test]
fn compaction_mid_unit_keeps_the_unit_working() {
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    prompt(&mut s, "a", 0);
    ev(
        &mut s,
        "a",
        "session_started",
        json!({"cwd": "/repo", "source": "compact"}),
        50,
    );
    assert_eq!(
        get(s.conn(), "a").unwrap().unwrap().state,
        SessionState::Working
    );
    stop(&mut s, "a", 80);
    assert_eq!(units(s.conn(), "a").unwrap()[0].active_secs, 80);
}

#[test]
fn resume_after_a_crash_closes_the_open_unit_at_its_last_event() {
    // The process died mid-unit (no Stop, no SessionEnd) and was resumed a day later.
    let mut s = Store::open_in_memory().unwrap();
    start(&mut s, "a", 0);
    prompt(&mut s, "a", 0);
    changed(&mut s, "a", &["x.rs"], 40);
    ev(
        &mut s,
        "a",
        "session_started",
        json!({"cwd": "/repo", "source": "resume"}),
        86_400,
    );
    let all = units(s.conn(), "a").unwrap();
    assert_eq!(all[0].stopped_at, Some(at(40).to_string()));
    assert_eq!(all[0].active_secs, 40);
    let session = get(s.conn(), "a").unwrap().unwrap();
    assert_eq!(session.state, SessionState::Idle);
    assert_eq!(session.active_secs, 40);

    // A wait still open at the crash ends with it.
    prompt(&mut s, "a", 90_000);
    ev(
        &mut s,
        "a",
        "needs_owner",
        json!({"kind": "permission_prompt"}),
        90_030,
    );
    ev(
        &mut s,
        "a",
        "session_started",
        json!({"cwd": "/repo", "source": "resume"}),
        200_000,
    );
    let all = units(s.conn(), "a").unwrap();
    assert_eq!(all[1].active_secs, 30);
    assert_eq!(all[1].waited_secs, 0);
}
