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
    // A higher status (from a verification) is kept.
    ev(
        &mut s,
        "s1",
        "verified",
        json!({"verif_id": "v1", "task_id": "1.2", "level": 2, "sha": "a",
               "evidence": "ok", "by": "agent"}),
        10,
    );
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

// ---- briefing and read-back (spec §8.4) ----

fn brief(s: &Store, id: &str) -> Briefing {
    briefing(s.conn(), &sess(s, id)).unwrap()
}

/// `setup()` plus session s0 that worked on 1.1 earlier and ended.
fn with_previous() -> Store {
    let mut s = setup();
    ev(
        &mut s,
        "s0",
        "session_started",
        json!({"cwd": "/wt", "worktree": "/wt", "source": "startup"}),
        0,
    );
    ev(
        &mut s,
        "s0",
        "task_claimed",
        json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        0,
    );
    s
}

#[test]
fn stable_with_nothing_pending_is_three_lines() {
    let mut s = Store::open_in_memory().unwrap();
    node(&mut s, "G", None, "goal");
    node(&mut s, "1", Some("G"), "milestone");
    node(&mut s, "1.1", Some("1"), "task");
    ev(
        &mut s,
        "s9",
        "session_started",
        json!({"cwd": "/wt", "source": "startup"}),
        1,
    );
    let b = brief(&s, "s9");
    assert_eq!(b.status, BriefingStatus::Stable);
    assert!(!b.readback_required);
    assert_eq!(
        b.text,
        "bridge-screen briefing — stable\nGoal: TG — why\nClaim your task with claim_task and go."
    );
}

#[test]
fn status_classification() {
    let mut s = setup();
    assert_eq!(brief(&s, "s1").status, BriefingStatus::Stable);
    ev(
        &mut s,
        "s1",
        "claimed_done",
        json!({"task_id": "1.2", "evidence": "e"}),
        3,
    );
    assert_eq!(brief(&s, "s1").status, BriefingStatus::Watcher);
    s.conn()
        .execute(
            "UPDATE plan_nodes SET status = 'failed' WHERE id = '1.1'",
            [],
        )
        .unwrap();
    assert_eq!(brief(&s, "s1").status, BriefingStatus::Unstable);
    s.conn()
        .execute(
            "UPDATE plan_nodes SET status = 'active' WHERE id = '1.1'",
            [],
        )
        .unwrap();
    ev(
        &mut s,
        "s1",
        "checkpoint_due",
        json!({"reason": "claimed_done"}),
        4,
    );
    ev(&mut s, "s1", "checkpoint_missed", json!({"unit_no": 1}), 5);
    let b = brief(&s, "s1");
    assert_eq!(b.status, BriefingStatus::Unstable);
    assert!(b.readback_required);
    assert!(b.text.contains("checkpoint missed"), "{}", b.text);
}

#[test]
fn briefing_is_bounded_and_scaled() {
    let mut s = setup();
    for n in 1..=10 {
        node(&mut s, &format!("1.{}", n + 2), Some("1"), "task");
        ev(
            &mut s,
            "old",
            "claimed_done",
            json!({"task_id": format!("1.{}", n + 2), "evidence": "e"}),
            3,
        );
    }
    for n in 0..30 {
        ev(
            &mut s,
            "s1",
            "leftover_found",
            json!({"leftover_id": format!("l{n:03}"), "from_session": "s0", "worktree": "/wt",
                   "path": format!("src/f{n}.rs"), "task_id": "1.1", "suggested": "resume",
                   "report_only": false}),
            4,
        );
    }
    let b = brief(&s, "s1");
    assert_eq!(b.status, BriefingStatus::Unstable);
    assert!(b.readback_required);
    let lines: Vec<&str> = b.text.lines().collect();
    assert!(lines.len() <= 40, "{} lines:\n{}", lines.len(), b.text);
    assert_eq!(lines[0], "bridge-screen briefing — unstable");
    assert!(
        b.text
            .contains("Verify first (10): 1.3 T1.3, 1.4 T1.4, 1.5 T1.5 (+7 more)"),
        "{}",
        b.text
    );
    assert!(
        b.text
            .contains("Leftovers (30, resolve with resolve_leftover):"),
        "{}",
        b.text
    );
    assert!(
        b.text.contains("    l000 src/f0.rs (1.1) → resume"),
        "{}",
        b.text
    );
    assert!(b.text.contains("    (+25 more)"), "{}", b.text);
    assert!(
        b.text.contains("l004 ") && !b.text.contains("l005 "),
        "{}",
        b.text
    );
    assert!(
        b.text.ends_with(
            "Read back before your first edit: call handoff_readback with the goal, what's in flight, and your next action."
        ),
        "{}",
        b.text
    );
}

#[test]
fn if_then_and_cue_carry_over_from_the_previous_session() {
    let mut s = with_previous();
    ev(
        &mut s,
        "s0",
        "checkpoint",
        json!({"sha": "abcdef123456", "task_id": "1.1", "next_action": "add the empty-password test"}),
        5,
    );
    ev(
        &mut s,
        "s0",
        "handoff_note",
        json!({"task_id": "1.1", "in_flight": "validation half wired",
               "if_then": ["if the form test flakes, rerun it once", "if CI is red, check the lockfile"]}),
        6,
    );
    ev(
        &mut s,
        "s0",
        "session_ended",
        json!({"end_kind": "clean"}),
        7,
    );
    // A new session picks up 1.1 (s1 is the one in setup, already on it).
    let b = brief(&s, "s1");
    assert!(
        b.text
            .contains("Summary: 1.1 T1.1; last checkpoint abcdef1 \"add the empty-password test\""),
        "{}",
        b.text
    );
    assert!(
        b.text.contains("In flight: validation half wired"),
        "{}",
        b.text
    );
    assert!(
        b.text.contains("If/then:\n  - if the form test flakes, rerun it once\n  - if CI is red, check the lockfile"),
        "{}",
        b.text
    );
    assert!(
        b.text.contains("Next: add the empty-password test"),
        "{}",
        b.text
    );
    assert!(b.text.contains("Milestone: T1 — why"), "{}", b.text);
}

#[test]
fn dependencies_are_listed() {
    let mut s = setup();
    ev(
        &mut s,
        "owner",
        "dep_added",
        json!({"id": "1.1", "depends_on": "1.2"}),
        3,
    );
    let b = brief(&s, "s1");
    assert!(b.text.contains("Dependencies: 1.2 waiting"), "{}", b.text);
    ev(
        &mut s,
        "s1",
        "claimed_done",
        json!({"task_id": "1.2", "evidence": "e"}),
        4,
    );
    assert!(brief(&s, "s1").text.contains("Dependencies: 1.2 ready"));
}

#[test]
fn compact_rebriefing_is_short() {
    let mut s = setup();
    ev(
        &mut s,
        "s1",
        "detour_opened",
        json!({"detour_id": "D1", "reason": "CI is red", "blocks": "1.1", "resume_cue": "rerun login test"}),
        3,
    );
    ev(
        &mut s,
        "s1",
        "item_suspended",
        json!({"item_id": "I1", "kind": "task", "ref_id": "1.1", "resume_cue": "rerun login test", "by_detour": "D1"}),
        3,
    );
    let text = compact_rebriefing(s.conn(), &sess(&s, "s1")).unwrap();
    assert!(text.lines().count() <= 8, "{text}");
    assert_eq!(
        text,
        "bridge-screen: re-briefing after compaction.\nActive task: 1.1 T1.1\nOn return: rerun login test\nOpen detours: D1 CI is red (current)"
    );
}

#[test]
fn readback_ok_and_one_correction() {
    let mut s = with_previous();
    ev(
        &mut s,
        "s0",
        "checkpoint",
        json!({"sha": "abc", "task_id": "1.1", "next_action": "add the empty-password test"}),
        5,
    );
    let me = sess(&s, "s1");
    let ok = check_readback(
        s.conn(),
        &me,
        "ship it",
        "working on 1.1, the form",
        "write the empty password test",
    )
    .unwrap();
    assert_eq!(ok.result, ReadbackState::Ok);
    assert_eq!(ok.reply, "Read-back matches the log. Go ahead.");
    let bad = check_readback(
        s.conn(),
        &me,
        "ship it",
        "working on 1.2",
        "refactor the router",
    )
    .unwrap();
    assert_eq!(bad.result, ReadbackState::Mismatch);
    assert_eq!(
        bad.reply,
        "Close, but the log says: active task 1.1 \"T1.1\", next action \"add the empty-password test\". Carry on from there."
    );
    assert!(bad.diff.as_deref().unwrap().contains("task"));
    ev(
        &mut s,
        "s1",
        "readback",
        json!({"result": "mismatch", "diff": bad.diff}),
        6,
    );
    // Only one correction per session.
    let again = check_readback(s.conn(), &sess(&s, "s1"), "x", "nothing", "nothing").unwrap();
    assert_eq!(again.result, ReadbackState::Mismatch);
    assert_eq!(again.reply, "Read-back recorded.");
}

#[test]
fn many_missed_checkpoints_stay_within_the_cap_and_keep_the_read_back_request() {
    let mut s = setup();
    for n in 0..45 {
        let id = format!("m{n:02}");
        ev(
            &mut s,
            &id,
            "session_started",
            json!({"cwd": "/wt", "source": "startup"}),
            3,
        );
        ev(
            &mut s,
            &id,
            "checkpoint_due",
            json!({"reason": "claimed_done"}),
            4,
        );
        ev(&mut s, &id, "checkpoint_missed", json!({"unit_no": 1}), 5);
    }
    let b = brief(&s, "s1");
    let lines: Vec<&str> = b.text.lines().collect();
    assert!(lines.len() <= 40, "{} lines:\n{}", lines.len(), b.text);
    assert_eq!(*lines.last().unwrap(), READBACK_ASK, "{}", b.text);
    assert!(
        b.text
            .contains("(+42 more sessions with a missed checkpoint)"),
        "{}",
        b.text
    );
}

#[test]
fn stable_with_a_checkpoint_needs_no_read_back() {
    let mut s = setup();
    ev(
        &mut s,
        "s1",
        "checkpoint",
        json!({"sha": "abcdef1234", "task_id": "1.1", "next_action": "wire the form"}),
        3,
    );
    let b = brief(&s, "s1");
    assert_eq!(b.status, BriefingStatus::Stable);
    assert!(b.text.contains("Next: wire the form"), "{}", b.text);
    assert!(!b.readback_required);
    assert!(!b.text.contains(READBACK_ASK), "{}", b.text);
}
