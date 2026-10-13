use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::plan::{NodeStatus, get_node};
use crate::session::{self, SessionState};
use crate::store::{EventDraft, Store, append_in_at};

const T0: &str = "2026-10-13T09:00:00Z";

fn at(secs: i64) -> Timestamp {
    T0.parse::<Timestamp>().unwrap() + SignedDuration::from_secs(secs)
}

fn ev(s: &mut Store, session: &str, kind: &str, data: Value, secs: i64) {
    s.write(|tx| append_in_at(tx, EventDraft::new("r", session, kind, data), at(secs)))
        .unwrap();
}

fn node(s: &mut Store, id: &str, parent: Option<&str>, kind: &str) {
    ev(
        s,
        "owner",
        "node_added",
        json!({"id": id, "parent_id": parent, "kind": kind, "title": format!("T{id}"),
               "intent": "why", "expected_files": []}),
        0,
    );
}

/// A plan (G, 1, 1.1, 1.2) and session s1 in worktree /wt on task 1.1.
fn setup() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    node(&mut s, "G", None, "goal");
    node(&mut s, "1", Some("G"), "milestone");
    node(&mut s, "1.1", Some("1"), "task");
    node(&mut s, "1.2", Some("1"), "task");
    ev(
        &mut s,
        "s1",
        "session_started",
        json!({"cwd": "/wt/sub", "worktree": "/wt", "source": "startup"}),
        1,
    );
    ev(
        &mut s,
        "s1",
        "task_claimed",
        json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        2,
    );
    s
}

fn sess(s: &Store, id: &str) -> session::Session {
    session::get(s.conn(), id).unwrap().unwrap()
}

#[test]
fn session_carries_worktree_and_last_task() {
    let s = setup();
    let x = sess(&s, "s1");
    assert_eq!(x.worktree.as_deref(), Some("/wt"));
    assert_eq!(x.last_task.as_deref(), Some("1.1"));
    assert_eq!(x.readback, ReadbackState::NotNeeded);
    assert_eq!(
        (x.end_kind, x.checkpoint_pending, x.checkpoint_missed),
        (None, None, 0)
    );
}

#[test]
fn claimed_done_records_l0_and_marks_the_task_claimed() {
    let mut s = setup();
    ev(
        &mut s,
        "s1",
        "claimed_done",
        json!({"task_id": "1.1", "evidence": "tests pass"}),
        10,
    );
    let status = |s: &Store, id: &str| get_node(s.conn(), "r", id).unwrap().unwrap().status;
    assert_eq!(status(&s, "1.1"), NodeStatus::Claimed);
    let claims = claims_done(s.conn(), "r").unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(
        (claims[0].task_id.as_str(), claims[0].evidence.as_str()),
        ("1.1", "tests pass")
    );
    // A higher status is kept.
    s.conn()
        .execute("UPDATE plan_nodes SET status = 'l2' WHERE id = '1.2'", [])
        .unwrap();
    ev(
        &mut s,
        "s1",
        "claimed_done",
        json!({"task_id": "1.2", "evidence": "x"}),
        11,
    );
    assert_eq!(status(&s, "1.2"), NodeStatus::L2);
    // Ending the session doesn't put a claimed task back to todo.
    ev(
        &mut s,
        "s1",
        "session_ended",
        json!({"reason": "other"}),
        12,
    );
    assert_eq!(status(&s, "1.1"), NodeStatus::Claimed);
}

#[test]
fn checkpoint_due_keeps_the_first_reason_until_a_checkpoint() {
    let mut s = setup();
    ev(
        &mut s,
        "s1",
        "checkpoint_due",
        json!({"reason": "claimed_done"}),
        10,
    );
    ev(
        &mut s,
        "s1",
        "checkpoint_due",
        json!({"reason": "detour_closed"}),
        11,
    );
    assert_eq!(
        sess(&s, "s1").checkpoint_pending.as_deref(),
        Some("claimed_done")
    );
    ev(
        &mut s,
        "s1",
        "note_sent",
        json!({"unit_no": 3, "kind": "checkpoint", "channel": "stop", "state_key": "cp"}),
        12,
    );
    ev(&mut s, "s1", "checkpoint_missed", json!({"unit_no": 3}), 13);
    let x = sess(&s, "s1");
    assert_eq!((x.checkpoint_asked_unit, x.checkpoint_missed), (Some(3), 3));
    ev(
        &mut s,
        "s1",
        "checkpoint",
        json!({"sha": "abc123", "task_id": "1.1", "next_action": "write the test"}),
        14,
    );
    let x = sess(&s, "s1");
    assert_eq!(x.checkpoint_pending, None);
    assert_eq!(x.checkpoint_asked_unit, None);
    assert_eq!(x.checkpoint_missed, 0);
    assert_eq!(x.last_checkpoint.as_deref(), Some("abc123"));
    let cp = latest_checkpoint(s.conn(), "s1").unwrap().unwrap();
    assert_eq!(cp.sha, "abc123");
    assert_eq!(cp.task_id.as_deref(), Some("1.1"));
    assert_eq!(cp.next_action.as_deref(), Some("write the test"));
    assert_eq!(cp.ts, at(14).to_string());
}

#[test]
fn leftovers_open_resolve_and_the_first_resolution_wins() {
    let mut s = setup();
    for (id, path, report_only) in [("L1", "a.rs", false), ("L2", "b.rs", true)] {
        ev(
            &mut s,
            "s2",
            "leftover_found",
            json!({"leftover_id": id, "from_session": "s1", "worktree": "/wt", "path": path,
                   "task_id": "1.1", "suggested": "undo", "report_only": report_only}),
            20,
        );
    }
    let open = leftovers(s.conn(), "r", true).unwrap();
    assert_eq!(open.len(), 2);
    assert_eq!(open[0].state, LeftoverState::Open);
    assert_eq!(open[1].state, LeftoverState::ReportOnly);
    assert_eq!(open[0].suggested, LeftoverAction::Undo);
    ev(
        &mut s,
        "owner",
        "leftover_resolved",
        json!({"leftover_id": "L1", "action": "adopt", "commit": "c0ffee", "by": "owner"}),
        21,
    );
    ev(
        &mut s,
        "s2",
        "leftover_resolved",
        json!({"leftover_id": "L1", "action": "undo", "stash_ref": "5747", "by": "agent"}),
        22,
    );
    let l1 = get_leftover(s.conn(), "L1").unwrap().unwrap();
    assert_eq!(l1.state, LeftoverState::Resolved);
    assert_eq!(l1.resolution, Some(LeftoverAction::Adopt));
    assert_eq!(l1.resolved_by, Some(ResolvedBy::Owner));
    assert_eq!(l1.commit_sha.as_deref(), Some("c0ffee"));
    assert_eq!(l1.stash_ref, None);
    assert_eq!(leftovers(s.conn(), "r", true).unwrap().len(), 1);
    assert_eq!(leftovers(s.conn(), "r", false).unwrap().len(), 2);
    ev(
        &mut s,
        "s2",
        "leftovers_analysed",
        json!({"from_session": "s1"}),
        23,
    );
    assert!(sess(&s, "s1").leftovers_analysed);
}

#[test]
fn notes_readback_and_briefing() {
    let mut s = setup();
    ev(
        &mut s,
        "s1",
        "handoff_note",
        json!({"task_id": "1.1", "in_flight": "half of the form",
               "if_then": ["if the test flakes, rerun once"]}),
        30,
    );
    let note = latest_note(s.conn(), "r", Some("1.1")).unwrap().unwrap();
    assert_eq!(note.session_id, "s1");
    assert_eq!(note.in_flight, "half of the form");
    assert_eq!(note.if_then, vec!["if the test flakes, rerun once"]);
    assert_eq!(latest_note(s.conn(), "r", Some("1.2")).unwrap(), None);

    ev(
        &mut s,
        "s1",
        "briefed",
        json!({"status": "watcher", "readback_required": true}),
        31,
    );
    let x = sess(&s, "s1");
    assert_eq!(x.briefing_status, Some(BriefingStatus::Watcher));
    assert_eq!(x.readback, ReadbackState::Pending);
    ev(
        &mut s,
        "s1",
        "readback",
        json!({"result": "mismatch", "diff": "task"}),
        32,
    );
    assert_eq!(sess(&s, "s1").readback, ReadbackState::Mismatch);
    ev(
        &mut s,
        "s1",
        "briefed",
        json!({"status": "stable", "readback_required": false}),
        33,
    );
    assert_eq!(sess(&s, "s1").readback, ReadbackState::NotNeeded);
}

#[test]
fn session_ended_stores_its_kind() {
    let mut s = setup();
    ev(
        &mut s,
        "s1",
        "session_ended",
        json!({"reason": "other", "end_kind": "dirty", "dirty_files": ["a.rs"]}),
        40,
    );
    let x = sess(&s, "s1");
    assert_eq!(x.end_kind, Some(EndKind::Dirty));
    assert_eq!(x.state, SessionState::Ended);
    assert_eq!(x.active_task, None);
    assert_eq!(x.last_task.as_deref(), Some("1.1"), "kept for recovery");
}

#[test]
fn crash_closes_the_unit_at_the_last_event() {
    let mut s = setup();
    ev(&mut s, "s1", "prompt", json!({"chars": 3}), 10);
    ev(
        &mut s,
        "s1",
        "file_changed",
        json!({"tool": "Edit", "files": ["a.rs"]}),
        100,
    );
    ev(
        &mut s,
        "s1",
        "session_crashed",
        json!({"last_event_at": at(100).to_string()}),
        5000,
    );
    let x = sess(&s, "s1");
    assert_eq!(x.state, SessionState::Ended);
    assert_eq!(x.end_kind, Some(EndKind::Crashed));
    assert_eq!(x.ended_at.as_deref(), Some(at(100).to_string().as_str()));
    assert_eq!(x.last_event_at, at(100).to_string());
    assert_eq!(x.active_secs, 90, "the crash gap isn't active time");
    assert_eq!(x.active_task, None);
    let u = session::units(s.conn(), "s1").unwrap();
    assert_eq!(
        u[0].stopped_at.as_deref(),
        Some(at(100).to_string().as_str())
    );
    assert_eq!(u[0].active_secs, 90);
    assert_eq!(
        get_node(s.conn(), "r", "1.1").unwrap().unwrap().status,
        NodeStatus::Todo
    );
    // A second crash event changes nothing; a resume brings the session back.
    ev(
        &mut s,
        "s1",
        "session_crashed",
        json!({"last_event_at": at(100).to_string()}),
        6000,
    );
    assert_eq!(sess(&s, "s1").active_secs, 90);
    ev(
        &mut s,
        "s1",
        "session_started",
        json!({"cwd": "/wt", "source": "resume"}),
        7000,
    );
    let x = sess(&s, "s1");
    assert_eq!((x.state, x.end_kind), (SessionState::Idle, None));
    assert_eq!(x.worktree.as_deref(), Some("/wt"));
}
