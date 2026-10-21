use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::repo::RepoInfo;
use crate::store::{EventDraft, Store, append_in_at};

fn t0() -> Timestamp {
    "2026-10-14T09:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0() + SignedDuration::from_secs(secs)
}

/// Append events at scripted times (seconds after `t0`).
fn script(s: &mut Store, events: &[(&str, &str, i64, &str, Value)]) {
    s.write(|tx| {
        for (repo, session, secs, kind, data) in events {
            let mut draft = EventDraft::new(*repo, *session, *kind, data.clone());
            if let Some(t) = data["task_id"].as_str() {
                draft = draft.task(t);
            }
            append_in_at(tx, draft, at(*secs))?;
        }
        Ok(())
    })
    .unwrap();
}

fn ev(s: &mut Store, session: &str, secs: i64, kind: &str, data: Value) {
    script(s, &[("r", session, secs, kind, data)]);
}

fn node(id: &str, parent: Option<&str>, kind: &str, estimate: Option<u32>) -> Value {
    json!({"id": id, "parent_id": parent, "kind": kind, "title": format!("T{id}"),
           "intent": "why", "estimate_min": estimate, "expected_files": []})
}

/// Repo `r` ("shop") with G → 1 (milestone) → 1.1, 1.2 and 2 (milestone) → 2.1.
fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.register_repo(&RepoInfo {
        id: "r".into(),
        name: "shop".into(),
        root: "/code/shop".into(),
    })
    .unwrap();
    let mut events = Vec::new();
    for (id, parent, kind, est) in [
        ("G", None, "goal", None),
        ("1", Some("G"), "milestone", None),
        ("1.1", Some("1"), "task", Some(30)),
        ("1.2", Some("1"), "task", Some(30)),
        ("2", Some("G"), "milestone", None),
        ("2.1", Some("2"), "task", Some(60)),
    ] {
        events.push(("r", "owner", 0, "node_added", node(id, parent, kind, est)));
    }
    script(&mut s, &events);
    s
}

fn start(s: &mut Store, session: &str, secs: i64) {
    ev(
        s,
        session,
        secs,
        "session_started",
        json!({"cwd": format!("/wt/{session}"), "source": "startup",
               "worktree": format!("/wt/{session}"), "branch": "feat"}),
    );
}

fn claim(s: &mut Store, session: &str, secs: i64, task: &str) {
    ev(
        s,
        session,
        secs,
        "task_claimed",
        json!({"task_id": task, "mode": "auto", "confidence": 1.0}),
    );
}

fn unit(s: &mut Store, session: &str, from: i64, secs: i64) {
    ev(s, session, from, "prompt", json!({"chars": 1}));
    ev(s, session, from + secs, "unit_stopped", json!({}));
}

fn red(s: &mut Store, session: &str, secs: i64) {
    ev(
        s,
        session,
        secs,
        "nav_state_changed",
        json!({"from": "green", "to": "red", "away_secs": 4000}),
    );
}

fn glance_at(s: &Store, secs: i64) -> Glance {
    glance(s.conn(), &Config::default(), at(secs)).unwrap()
}

fn kinds(g: &Glance) -> Vec<(NeedsKind, String)> {
    g.needs_you.iter().map(|i| (i.kind, i.id.clone())).collect()
}

fn row<'a>(g: &'a Glance, session: &str) -> &'a SessionRow {
    g.projects
        .iter()
        .flat_map(|p| &p.sessions)
        .find(|r| r.session_id == session)
        .unwrap()
}

/// A task claimed done, nudged twice and escalated to the owner.
fn overdue(s: &mut Store, session: &str, task: &str, secs: i64) {
    ev(
        s,
        session,
        secs,
        "claimed_done",
        json!({"task_id": task, "evidence": "tests pass"}),
    );
    for i in 1..=2 {
        ev(
            s,
            session,
            secs + i,
            "verify_nudged",
            json!({"task_id": task, "level": 0}),
        );
    }
    ev(
        s,
        session,
        secs + 3,
        "needs_owner",
        json!({"kind": "verify", "task_id": task}),
    );
}

fn leftover(s: &mut Store, by: &str, id: &str, from: &str, path: &str, secs: i64) {
    ev(
        s,
        by,
        secs,
        "leftover_found",
        json!({"leftover_id": id, "from_session": from, "worktree": format!("/wt/{from}"),
               "path": path, "task_id": "1.2", "suggested": "undo"}),
    );
}

#[test]
#[allow(clippy::too_many_lines)] // one scenario with every kind
fn each_kind_has_its_actions_and_rank() {
    let mut s = store();
    start(&mut s, "drift0001", 0);
    claim(&mut s, "drift0001", 1, "1.1");
    ev(&mut s, "drift0001", 2, "prompt", json!({}));
    ev(
        &mut s,
        "drift0001",
        3,
        "detour_opened",
        json!({"detour_id": "dt", "reason": "CI cache is broken", "resume_cue": "x", "blocks": "1.1"}),
    );
    ev(&mut s, "drift0001", 4000, "unit_stopped", json!({}));
    red(&mut s, "drift0001", 4000);
    start(&mut s, "perm00002", 10);
    ev(&mut s, "perm00002", 20, "prompt", json!({}));
    ev(
        &mut s,
        "perm00002",
        30,
        "needs_owner",
        json!({"kind": "permission_prompt"}),
    );
    start(&mut s, "verif0003", 40);
    claim(&mut s, "verif0003", 41, "1.2");
    overdue(&mut s, "verif0003", "1.2", 50);
    leftover(&mut s, "verif0003", "lab", "dead00004", "src/a.rs", 60);
    leftover(&mut s, "verif0003", "lcd", "dead00004", "src/b.rs", 61);
    // Report-only leftovers sit in a live session's worktree: never an owner item.
    ev(
        &mut s,
        "verif0003",
        62,
        "leftover_found",
        json!({"leftover_id": "lro", "from_session": "dead00005", "worktree": "/wt/x",
               "path": "c.rs", "suggested": "undo", "report_only": true}),
    );

    let g = glance_at(&s, 5000);
    assert_eq!(
        kinds(&g),
        vec![
            (NeedsKind::DriftRed, "drift_red:drift0001".to_string()),
            (
                NeedsKind::PermissionWait,
                "permission_wait:perm00002".to_string()
            ),
            (NeedsKind::VerifyOverdue, "verify_overdue:r:1.2".to_string()),
            (NeedsKind::Leftover, "leftover:r:dead00004".to_string()),
        ]
    );
    assert_eq!(g.needs_you_count, 4);
    assert!(!g.escalated);
    let labels =
        |i: &NeedsYou| -> Vec<String> { i.actions.iter().map(|a| a.label.clone()).collect() };
    let drift = &g.needs_you[0];
    assert_eq!(labels(drift), vec!["Continue", "Make it a task", "Stop"]);
    assert_eq!(drift.actions[0].inputs[0].name, "reason");
    assert!(!drift.actions[0].inputs[0].required);
    assert_eq!(
        drift.actions[1]
            .inputs
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        vec!["parent", "title"]
    );
    assert_eq!(
        serde_json::to_value(&drift.actions[2].action).unwrap(),
        json!({"kind": "drift", "session": "drift0001", "answer": "stop"})
    );
    assert_eq!(drift.age_s, 1000);
    let perm = &g.needs_you[1];
    assert_eq!(perm.actions, vec![]);
    assert_eq!(
        perm.respond_in,
        Some(RespondIn {
            session_short: "perm00".into(),
            worktree: Some("/wt/perm00002".into())
        })
    );
    let verify = &g.needs_you[2];
    assert_eq!(labels(verify), vec!["✓ Works", "✗ Broken"]);
    assert_eq!(
        serde_json::to_value(&verify.actions[0].action).unwrap(),
        json!({"kind": "verify", "repo": "r", "task": "1.2", "level": 4})
    );
    assert_eq!(verify.actions[1].inputs[0].name, "note");
    let left = &g.needs_you[3];
    assert_eq!(labels(left), vec!["Adopt", "Resume", "Undo"]);
    assert_eq!(left.files, vec!["src/a.rs", "src/b.rs"]);
    assert_eq!(left.actions[2].tone, Tone::Primary, "the suggested action");
    assert_eq!(
        serde_json::to_value(&left.actions[0].action).unwrap(),
        json!({"kind": "resolve", "repo": "r", "leftover_ids": ["lab", "lcd"], "action": "adopt"})
    );
    // Lights match the queue: red for the queued sessions.
    assert_eq!(row(&g, "drift0001").light, Light::Red);
    assert_eq!(row(&g, "perm00002").light, Light::Red);
    assert_eq!(
        row(&g, "perm00002").rest,
        Some(Rest::NeedsYou {
            why: NeedsKind::PermissionWait
        })
    );
}

#[test]
fn a_drift_without_a_detour_offers_no_make_task_and_an_answer_clears_it() {
    let mut s = store();
    start(&mut s, "drift0001", 0);
    unit(&mut s, "drift0001", 1, 100);
    red(&mut s, "drift0001", 101);
    let g = glance_at(&s, 200);
    let labels: Vec<&str> = g.needs_you[0]
        .actions
        .iter()
        .map(|a| a.label.as_str())
        .collect();
    assert_eq!(labels, vec!["Continue", "Stop"]);
    // Stop leaves the session red, but the flag is answered: no longer queued.
    script(
        &mut s,
        &[(
            "r",
            "owner",
            300,
            "owner_action",
            json!({"action": "drift", "answer": "stop", "target": "drift0001"}),
        )],
    );
    let g = glance_at(&s, 400);
    assert_eq!(g.needs_you, vec![]);
    let r = row(&g, "drift0001");
    // The answered flag is no needs-you item on any screen, `status` included.
    assert_eq!((r.light, r.state_label.as_str()), (Light::Amber, "idle"));
}

#[test]
fn info_states_are_session_flags_with_ages_never_queue_items() {
    let mut s = store();
    start(&mut s, "flags0001", 0);
    ev(
        &mut s,
        "flags0001",
        10,
        "checkpoint_due",
        json!({"reason": "claimed_done"}),
    );
    ev(
        &mut s,
        "flags0001",
        20,
        "briefed",
        json!({"status": "unstable", "readback_required": true}),
    );
    ev(
        &mut s,
        "flags0001",
        30,
        "squash_failed",
        json!({"reason": "conflict"}),
    );
    start(&mut s, "miss00002", 0);
    ev(
        &mut s,
        "miss00002",
        40,
        "checkpoint_due",
        json!({"reason": "detour_closed"}),
    );
    ev(
        &mut s,
        "miss00002",
        50,
        "checkpoint_missed",
        json!({"unit_no": 2}),
    );
    // Claude idle at the prompt is no needs-you item either.
    start(&mut s, "idle00003", 0);
    unit(&mut s, "idle00003", 1, 10);
    ev(
        &mut s,
        "idle00003",
        60,
        "needs_owner",
        json!({"kind": "idle_prompt"}),
    );

    let g = glance_at(&s, 100);
    assert_eq!(g.needs_you_count, 0);
    assert_eq!(g.needs_you, vec![]);
    let f = &row(&g, "flags0001").flags;
    let got: Vec<(FlagKind, Option<i64>)> = f.iter().map(|f| (f.kind, f.age_s)).collect();
    assert_eq!(
        got,
        vec![
            (FlagKind::CheckpointPending, Some(90)),
            (FlagKind::ReadbackPending, Some(80)),
            (FlagKind::HandoffUnstable, Some(80)),
            (FlagKind::SquashFailed, Some(70)),
        ]
    );
    let m = row(&g, "miss00002");
    assert_eq!(m.flags[0].kind, FlagKind::CheckpointMissed);
    assert_eq!(m.flags[0].age_s, Some(50));
    // A missed checkpoint is owed by the agent: a flag, not "needs you" (owner decision).
    assert_eq!(m.state_label, "idle");
    assert_ne!(m.light, Light::Red);
    assert_eq!(row(&g, "flags0001").state_label, "idle");
    let idle = row(&g, "idle00003");
    assert_eq!(
        (
            idle.light,
            idle.light_word.as_str(),
            idle.state_label.as_str()
        ),
        (Light::Amber, "idle", "idle")
    );
    assert_eq!(idle.rest, Some(Rest::Idle));
}

#[test]
fn rest_tells_done_from_idle() {
    let mut s = store();
    // Claimed done and checkpointed: done.
    start(&mut s, "done00001", 0);
    claim(&mut s, "done00001", 1, "1.1");
    unit(&mut s, "done00001", 2, 10);
    ev(
        &mut s,
        "done00001",
        20,
        "claimed_done",
        json!({"task_id": "1.1", "evidence": "ok"}),
    );
    // Claimed done, but the checkpoint is still owed: idle.
    start(&mut s, "owed00002", 0);
    claim(&mut s, "owed00002", 1, "1.2");
    unit(&mut s, "owed00002", 2, 10);
    ev(
        &mut s,
        "owed00002",
        20,
        "claimed_done",
        json!({"task_id": "1.2", "evidence": "ok"}),
    );
    ev(
        &mut s,
        "owed00002",
        21,
        "checkpoint_due",
        json!({"reason": "claimed_done"}),
    );
    // Stopped mid-task without a question: idle.
    start(&mut s, "mid000003", 0);
    claim(&mut s, "mid000003", 1, "2.1");
    unit(&mut s, "mid000003", 2, 10);
    // Working: no rest.
    start(&mut s, "work00004", 0);
    ev(&mut s, "work00004", 5, "prompt", json!({}));

    let g = glance_at(&s, 100);
    let done = row(&g, "done00001");
    assert_eq!(
        done.rest,
        Some(Rest::Done {
            task: NodeRef {
                id: "1.1".into(),
                title: "T1.1".into()
            }
        })
    );
    assert_eq!(
        (
            done.light,
            done.light_word.as_str(),
            done.state_label.as_str()
        ),
        (Light::Green, "done", "done")
    );
    assert_eq!(row(&g, "owed00002").rest, Some(Rest::Idle));
    assert_eq!(row(&g, "mid000003").rest, Some(Rest::Idle));
    let work = row(&g, "work00004");
    assert_eq!(work.rest, None);
    assert_eq!(
        (work.light, work.light_word.as_str()),
        (Light::Green, "on task")
    );
    assert_eq!(g.needs_you_count, 0);
}

#[test]
fn oldest_first_within_a_rank_and_stable_between_builds() {
    let mut s = store();
    for sid in ["late0001", "early002", "mid00003"] {
        start(&mut s, sid, 0);
        unit(&mut s, sid, 1, 10);
    }
    red(&mut s, "early002", 100);
    red(&mut s, "mid00003", 200);
    red(&mut s, "late0001", 300);
    // Same age: by id.
    start(&mut s, "twin0002", 0);
    start(&mut s, "twin0001", 0);
    for sid in ["twin0002", "twin0001"] {
        ev(&mut s, sid, 400, "prompt", json!({}));
        ev(
            &mut s,
            sid,
            400,
            "needs_owner",
            json!({"kind": "permission_prompt"}),
        );
    }
    let a = glance_at(&s, 1000);
    let ids: Vec<&str> = a.needs_you.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "drift_red:early002",
            "drift_red:mid00003",
            "drift_red:late0001",
            "permission_wait:twin0001",
            "permission_wait:twin0002",
        ]
    );
    let b = glance_at(&s, 1000);
    assert_eq!(a, b);
}

#[test]
fn more_than_four_items_escalate() {
    let mut s = store();
    for i in 0..4 {
        let sid = format!("wait000{i}");
        start(&mut s, &sid, 0);
        ev(&mut s, &sid, 1, "prompt", json!({}));
        ev(&mut s, &sid, 2, "needs_owner", json!({"kind": "other"}));
    }
    let g = glance_at(&s, 10);
    assert_eq!((g.needs_you_count, g.escalated), (4, false));
    start(&mut s, "wait0004", 0);
    ev(&mut s, "wait0004", 1, "prompt", json!({}));
    ev(
        &mut s,
        "wait0004",
        2,
        "needs_owner",
        json!({"kind": "permission_prompt"}),
    );
    let g = glance_at(&s, 10);
    assert_eq!((g.needs_you_count, g.escalated), (5, true));
    assert_eq!(g.projects[0].needs_you, 5);
}

#[test]
fn milestones_show_hill_progress_and_stuck() {
    let mut s = store();
    ev(
        &mut s,
        "owner",
        1,
        "hill_moved",
        json!({"id": "1", "hill": "uphill"}),
    );
    ev(
        &mut s,
        "owner",
        1,
        "hill_moved",
        json!({"id": "2", "hill": "downhill"}),
    );
    ev(
        &mut s,
        "owner",
        2,
        "verified",
        json!({"verif_id": "v1", "task_id": "1.1", "level": 2, "sha": "abc", "evidence": "t",
               "by": "agent"}),
    );
    // Two sessions worked under milestone 1 without moving it: not stuck yet.
    for (i, sid) in ["m1s00001", "m1s00002"].iter().enumerate() {
        let t = 100 + i64::try_from(i).unwrap() * 100;
        start(&mut s, sid, t);
        claim(&mut s, sid, t + 1, "1.2");
        unit(&mut s, sid, t + 2, 10);
        ev(&mut s, sid, t + 20, "session_ended", json!({}));
    }
    let g = glance_at(&s, 1000);
    let m = &g.projects[0].milestones;
    assert_eq!(m.len(), 2);
    assert_eq!(
        (m[0].hill, m[0].done_pct, m[0].stuck),
        (Some(Hill::Uphill), 50, false)
    );
    assert_eq!((m[1].hill, m[1].done_pct), (Some(Hill::Downhill), 0));
    // A third: stuck for 3 sessions.
    start(&mut s, "m1s00003", 400);
    claim(&mut s, "m1s00003", 401, "1.2");
    unit(&mut s, "m1s00003", 402, 10);
    let g = glance_at(&s, 1000);
    let m = &g.projects[0].milestones[0];
    assert_eq!((m.stuck, m.stuck_sessions), (true, 3));
    assert!(!g.projects[0].milestones[1].stuck);
    // The hill moving since the earliest of them clears it.
    ev(
        &mut s,
        "owner",
        500,
        "hill_moved",
        json!({"id": "1", "hill": "downhill"}),
    );
    assert!(!glance_at(&s, 1000).projects[0].milestones[0].stuck);
    // Four sessions after that move: stuck for those 4, not for all 7 that ever worked
    // under it.
    for (i, sid) in ["m1s00004", "m1s00005", "m1s00006", "m1s00007"]
        .iter()
        .enumerate()
    {
        let t = 600 + i64::try_from(i).unwrap() * 100;
        start(&mut s, sid, t);
        claim(&mut s, sid, t + 1, "1.2");
        unit(&mut s, sid, t + 2, 10);
        ev(&mut s, sid, t + 20, "session_ended", json!({}));
    }
    let m = &glance_at(&s, 2000).projects[0].milestones[0];
    assert_eq!((m.stuck, m.stuck_sessions), (true, 4));
}

#[test]
fn ended_sessions_are_left_out() {
    let mut s = store();
    start(&mut s, "gone0001", 0);
    red(&mut s, "gone0001", 1);
    ev(&mut s, "gone0001", 2, "session_ended", json!({}));
    start(&mut s, "live0002", 3);
    let g = glance_at(&s, 10);
    assert_eq!(g.live_sessions, 1);
    let ids: Vec<&str> = g.projects[0]
        .sessions
        .iter()
        .map(|r| r.session_id.as_str())
        .collect();
    assert_eq!(ids, vec!["live0002"]);
    assert_eq!(g.needs_you, vec![]);
    // Nothing finished yet: no dates, the reason instead.
    let f = g.projects[0].forecast.as_ref().unwrap();
    assert_eq!(f.compact, None);
    assert_eq!(
        f.text,
        "Not enough finished tasks to forecast yet — 3 more are needed"
    );
    assert_eq!(f.short, "forecast needs 3 more finished tasks");
    assert_eq!(g.projects[0].cost, None);
    assert_eq!(
        g.projects[0].goal,
        Some(NodeRef {
            id: "G".into(),
            title: "TG".into()
        })
    );
}

#[test]
fn a_question_is_a_needs_you_rest() {
    let mut s = store();
    start(&mut s, "ask00001", 0);
    claim(&mut s, "ask00001", 1, "1.1");
    unit(&mut s, "ask00001", 2, 10);
    ev(
        &mut s,
        "ask00001",
        12,
        "owner_question",
        json!({"unit_no": 1, "source": "turn_end"}),
    );
    let g = glance_at(&s, 312);
    let r = row(&g, "ask00001");
    assert_eq!(
        r.rest,
        Some(Rest::NeedsYou {
            why: NeedsKind::Question
        })
    );
    assert_eq!((r.light, r.state_label.as_str()), (Light::Red, "needs you"));
    let q = &g.needs_you[0];
    assert_eq!(q.kind, NeedsKind::Question);
    assert_eq!(q.detail, "5 min ago · answer in that session · on 1.1 T1.1");
    assert_eq!(q.rank, 1);
    assert_eq!(
        q.respond_in.as_ref().unwrap().worktree.as_deref(),
        Some("/wt/ask00001")
    );
    ev(&mut s, "ask00001", 400, "prompt", json!({}));
    assert_eq!(glance_at(&s, 401).needs_you, vec![]);
}

#[test]
fn far_off_title_names_the_session_and_the_time_away() {
    assert_eq!(far_off_title("b1", 2400), "b1 · far off its task (40 min)");
    assert_eq!(far_off_title("b1", 30), "b1 · far off its task");
}

#[test]
fn project_and_session_rows_carry_forecast_and_cost() {
    let mut s = store();
    for (id, est) in [("1.3", 30), ("1.4", 30)] {
        ev(
            &mut s,
            "owner",
            0,
            "node_added",
            node(id, Some("1"), "task", Some(est)),
        );
    }
    // Three tasks finish at their estimate (30 min each); 1.4 and 2.1 are left.
    for (n, task) in ["1.1", "1.2", "1.3"].iter().enumerate() {
        let sid = format!("done000{n}");
        let from = i64::try_from(n).unwrap() * 2000;
        start(&mut s, &sid, from);
        claim(&mut s, &sid, from, task);
        unit(&mut s, &sid, from + 1, 1800);
        ev(
            &mut s,
            &sid,
            from + 1802,
            "verified",
            json!({"verif_id": format!("v{task}"), "task_id": task, "level": 2, "sha": "a",
                   "files": [], "evidence": "ok", "by": "agent"}),
        );
        ev(&mut s, &sid, from + 1803, "session_ended", json!({}));
    }
    start(&mut s, "live0001", 7000);
    claim(&mut s, "live0001", 7000, "1.4");
    ev(&mut s, "live0001", 7001, "prompt", json!({"chars": 1}));
    ev(
        &mut s,
        "live0001",
        7010,
        "usage",
        json!({"path": "/t", "offset_after": 1, "replies": [
            {"message_id": "m1", "model": "claude-opus-5-5", "ts": at(7005).to_string(),
             "input": 1_000_000, "output": 100_000},
            {"message_id": "m2", "model": "mystery", "ts": at(7006).to_string(),
             "output": 2_000_000}]}),
    );
    let g = glance_at(&s, 7100);
    let p = &g.projects[0];
    let f = p.forecast.as_ref().unwrap();
    assert_eq!(f.forecast.points, 3);
    assert_eq!(f.forecast.remaining_min, 90);
    // Every ratio is 1.0: P50 = P90 = 90 minutes, minus nothing spent yet on 1.4.
    assert_eq!((f.forecast.p50_hours, f.forecast.p90_hours), (1.5, 1.5));
    assert!(f.text.starts_with("likely by "), "{}", f.text);
    assert!(f.short.starts_with("likely "), "{}", f.short);
    assert!(f.compact.is_some());
    assert_eq!(
        f.help,
        "50% and 90% chance, based on how long 3 verified tasks actually took"
    );
    let cost = p.cost.as_ref().unwrap();
    assert_eq!(cost.tokens, 3_100_000);
    assert_eq!(cost.unpriced_models, ["mystery"]);
    assert_eq!(cost.text, "≥ $6.00 (some models unpriced)");
    assert_eq!(
        cost.summary,
        "≥ $6.00 spent (some models unpriced) · 3.1M tokens"
    );
    let live = row(&g, "live0001").cost.as_ref().unwrap();
    assert_eq!(live.usd, Some(6.0));
}
