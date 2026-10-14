//! `bridge-screen status`: the compact text view.

mod common;

use bridge_screen_core::claim::{ClaimMode, ClaimRequest, ClaimTarget, claim};
use bridge_screen_core::plan::{EditContext, NodeKind, PlanOp, plan_edit};
use bridge_screen_core::repo::RepoInfo;
use bridge_screen_core::store::{Actor, EventDraft, Store, append_in_at};
use common::Env;
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

fn at(secs: i64) -> Timestamp {
    "2026-10-13T09:00:00Z".parse::<Timestamp>().unwrap() + SignedDuration::from_secs(secs)
}

fn ev(s: &mut Store, repo: &str, session: &str, kind: &str, data: serde_json::Value, secs: i64) {
    s.write(|tx| append_in_at(tx, EventDraft::new(repo, session, kind, data), at(secs)))
        .unwrap();
}

/// `n` units of `secs` seconds each, starting at `from`.
fn units(s: &mut Store, repo: &str, session: &str, n: i64, secs: i64, from: i64) {
    for i in 0..n {
        let t = from + i * (secs + 10);
        ev(s, repo, session, "prompt", json!({"chars": 1}), t);
        ev(s, repo, session, "unit_stopped", json!({}), t + secs);
    }
}

fn start(s: &mut Store, repo: &str, session: &str, secs: i64) {
    ev(
        s,
        repo,
        session,
        "session_started",
        json!({"cwd": "/x", "source": "startup"}),
        secs,
    );
}

fn register(s: &Store, id: &str, name: &str) {
    s.register_repo(&RepoInfo {
        id: id.into(),
        name: name.into(),
        root: format!("/code/{name}").into(),
    })
    .unwrap();
}

fn status(env: &Env) -> String {
    // The scripted sessions are dated in the past: keep the crash scan away from them.
    let config = env.home.path().join("config.toml");
    if !config.exists() {
        std::fs::write(&config, "crash_timeout_min = 100000000\n").unwrap();
    }
    let out = env.run(&["status"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn no_live_sessions() {
    let env = Env::new();
    assert_eq!(status(&env), "bridge-screen · no live sessions\n");
    let mut s = Store::open(&env.db_path()).unwrap();
    register(&s, "r1", "shop");
    start(&mut s, "r1", "gone00001", 0);
    ev(&mut s, "r1", "gone00001", "session_ended", json!({}), 5);
    drop(s);
    assert_eq!(status(&env), "bridge-screen · no live sessions\n");
}

#[test]
fn sessions_grouped_by_project_needs_you_first() {
    let env = Env::new();
    let mut s = Store::open(&env.db_path()).unwrap();
    register(&s, "r1", "shop");
    register(&s, "r2", "api");
    plan_edit(
        &mut s,
        &EditContext {
            repo_id: "r1".into(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: None,
            replan: false,
        },
        &[
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                title: "Online shop".into(),
                intent: Some("People can buy things.".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("G".into()),
                kind: NodeKind::Task,
                title: "Cart".into(),
                intent: None,
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("G".into()),
                kind: NodeKind::Task,
                title: "Login form".into(),
                intent: None,
                estimate_min: None,
                expected_files: vec![],
            },
        ],
    )
    .unwrap();

    // shop: one working session on 2 Login form (3 units, 18 min), one waiting on the
    // owner without a task (1 unit, 2 min), one idle.
    start(&mut s, "r1", "work00aaa", 0);
    claim(
        &mut s,
        &ClaimRequest {
            session_id: "work00aaa".into(),
            repo_id: "r1".into(),
            task: ClaimTarget::Existing("2".into()),
            mode: ClaimMode::Auto,
            confidence: 1.0,
            actor: Actor::default(),
        },
    )
    .unwrap();
    units(&mut s, "r1", "work00aaa", 3, 360, 10);
    ev(&mut s, "r1", "work00aaa", "prompt", json!({}), 5000);
    start(&mut s, "r1", "wait00bbb", 100);
    units(&mut s, "r1", "wait00bbb", 1, 125, 110);
    ev(
        &mut s,
        "r1",
        "wait00bbb",
        "needs_owner",
        json!({"kind": "idle_prompt"}),
        400,
    );
    start(&mut s, "r1", "idle00ccc", 200);
    // api: no plan, one idle session.
    start(&mut s, "r2", "apis00ddd", 300);
    units(&mut s, "r2", "apis00ddd", 2, 61, 310);
    drop(s);

    assert_eq!(
        status(&env),
        "\
bridge-screen · 4 live sessions · 2 projects
api  no plan yet
  S apis00  idle      (no task)             2 units · 2m · on task
shop  goal: Online shop  · done 0% · claimed 0% · 2 unestimated
  S wait00  needs you (no task)             1 unit  · 2m · on task
  S work00  working   2 Login form          3 units · 18m · on task
  S idle00  idle      (no task)             0 units · 0m · on task
"
    );
}

#[test]
fn long_titles_are_cut_to_fit() {
    let env = Env::new();
    let mut s = Store::open(&env.db_path()).unwrap();
    register(&s, "r1", "shop");
    plan_edit(
        &mut s,
        &EditContext {
            repo_id: "r1".into(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: None,
            replan: false,
        },
        &[
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                title: "G".into(),
                intent: Some("i".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("G".into()),
                kind: NodeKind::Task,
                title: "A very long task title that keeps going".into(),
                intent: None,
                estimate_min: None,
                expected_files: vec![],
            },
        ],
    )
    .unwrap();
    start(&mut s, "r1", "long00aaa", 0);
    claim(
        &mut s,
        &ClaimRequest {
            session_id: "long00aaa".into(),
            repo_id: "r1".into(),
            task: ClaimTarget::Existing("1".into()),
            mode: ClaimMode::Auto,
            confidence: 1.0,
            actor: Actor::default(),
        },
    )
    .unwrap();
    drop(s);
    let out = status(&env);
    assert_eq!(
        out.lines().nth(2).unwrap(),
        "  S long00  idle      1 A very long task…   0 units · 0m · on task"
    );
}

#[test]
fn detours_drift_and_disagreements_show() {
    let env = Env::new();
    let mut s = Store::open(&env.db_path()).unwrap();
    register(&s, "r1", "shop");
    // det: 25 minutes on an open detour.
    start(&mut s, "r1", "det00aaa", 0);
    ev(&mut s, "r1", "det00aaa", "prompt", json!({}), 10);
    ev(
        &mut s,
        "r1",
        "det00aaa",
        "detour_opened",
        json!({"detour_id": "D", "reason": "CI cache is broken", "resume_cue": "x"}),
        10,
    );
    ev(&mut s, "r1", "det00aaa", "unit_stopped", json!({}), 1510);
    ev(
        &mut s,
        "r1",
        "det00aaa",
        "unit_labeled",
        json!({"unit_no": 1, "label": "detour", "source": "declared"}),
        1510,
    );
    // dri: 40 minutes of drift, labelled drift by the owner and red.
    start(&mut s, "r1", "dri00bbb", 5);
    ev(&mut s, "r1", "dri00bbb", "prompt", json!({}), 10);
    ev(
        &mut s,
        "r1",
        "dri00bbb",
        "unit_stopped",
        json!({"score": 0.9}),
        2410,
    );
    ev(
        &mut s,
        "r1",
        "dri00bbb",
        "unit_labeled",
        json!({"unit_no": 1, "label": "drift", "source": "owner"}),
        2410,
    );
    ev(
        &mut s,
        "r1",
        "dri00bbb",
        "nav_state_changed",
        json!({"from": "green", "to": "red", "away_secs": 2400}),
        2410,
    );
    // dis: on task, but the agent disagreed with the heuristic.
    start(&mut s, "r1", "dis00ccc", 7);
    ev(&mut s, "r1", "dis00ccc", "prompt", json!({}), 10);
    ev(
        &mut s,
        "r1",
        "dis00ccc",
        "unit_stopped",
        json!({"score": 0.9}),
        70,
    );
    ev(
        &mut s,
        "r1",
        "dis00ccc",
        "unit_labeled",
        json!({"unit_no": 1, "label": "on_task", "source": "agent", "heuristic": "drift"}),
        70,
    );
    drop(s);
    assert_eq!(
        status(&env),
        "\
bridge-screen · 3 live sessions · 1 project
shop  no plan yet
  S dri00b  needs you (no task)             1 unit  · 40m · drift ⚠ 40m
  S dis00c  idle      (no task)             1 unit  · 1m · on task ⚠
  S det00a  idle      (no task)             1 unit  · 25m · detour 25m · D1 \"CI cache is broken\"
"
    );
}

#[test]
fn checkpoints_read_back_and_leftovers_show() {
    let env = Env::new();
    let mut s = Store::open(&env.db_path()).unwrap();
    register(&s, "r1", "shop");
    // ok: idle, nothing pending.
    start(&mut s, "r1", "ok000aaa", 0);
    // pend: a checkpoint is due and the read-back is pending.
    start(&mut s, "r1", "pend0bbb", 1);
    ev(
        &mut s,
        "r1",
        "pend0bbb",
        "checkpoint_due",
        json!({"reason": "claimed_done"}),
        2,
    );
    ev(
        &mut s,
        "r1",
        "pend0bbb",
        "briefed",
        json!({"status": "watcher", "readback_required": true}),
        2,
    );
    // miss: the checkpoint was asked for and missed — it needs the owner.
    start(&mut s, "r1", "miss0ccc", 3);
    ev(
        &mut s,
        "r1",
        "miss0ccc",
        "checkpoint_due",
        json!({"reason": "detour_closed"}),
        4,
    );
    ev(
        &mut s,
        "r1",
        "miss0ccc",
        "checkpoint_missed",
        json!({"unit_no": 2}),
        5,
    );
    for (id, report_only) in [("l1", false), ("l2", false), ("l3", true)] {
        ev(
            &mut s,
            "r1",
            "ok000aaa",
            "leftover_found",
            json!({"leftover_id": id, "from_session": "gone", "worktree": "/w",
                   "path": format!("{id}.rs"), "suggested": "undo", "report_only": report_only}),
            6,
        );
    }
    drop(s);
    assert_eq!(
        status(&env),
        "\
bridge-screen · 3 live sessions · 1 project
shop  no plan yet
  leftovers: 3 open (1 report-only)
  S miss0c  needs you (no task)             0 units · 0m · on task · checkpoint missed
  S pend0b  idle      (no task)             0 units · 0m · on task · checkpoint pending · read-back pending
  S ok000a  idle      (no task)             0 units · 0m · on task
"
    );
}

/// Every needs-you kind and the escalation line, rendered from the glance model.
#[test]
#[allow(clippy::too_many_lines)] // one scenario with every kind
fn status_from_glance_matches_every_kind() {
    let env = Env::new();
    let mut s = Store::open(&env.db_path()).unwrap();
    register(&s, "r1", "shop");
    plan_edit(
        &mut s,
        &EditContext {
            repo_id: "r1".into(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: None,
            replan: false,
        },
        &[
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                title: "Online shop".into(),
                intent: Some("People can buy things.".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("G".into()),
                kind: NodeKind::Task,
                title: "Cart".into(),
                intent: None,
                estimate_min: Some(30),
                expected_files: vec![],
            },
        ],
    )
    .unwrap();
    // drift_red: 40 minutes of drift, red.
    start(&mut s, "r1", "red00aaa", 0);
    ev(&mut s, "r1", "red00aaa", "prompt", json!({}), 10);
    ev(
        &mut s,
        "r1",
        "red00aaa",
        "unit_stopped",
        json!({"score": 0.9}),
        2410,
    );
    ev(
        &mut s,
        "r1",
        "red00aaa",
        "unit_labeled",
        json!({"unit_no": 1, "label": "drift", "source": "owner"}),
        2410,
    );
    ev(
        &mut s,
        "r1",
        "red00aaa",
        "nav_state_changed",
        json!({"from": "green", "to": "red", "away_secs": 2400}),
        2410,
    );
    // permission_wait, twice.
    for (sid, t) in [("perm0bbb", 20), ("perm0ccc", 30)] {
        start(&mut s, "r1", sid, t);
        ev(&mut s, "r1", sid, "prompt", json!({}), t + 1);
        ev(
            &mut s,
            "r1",
            sid,
            "needs_owner",
            json!({"kind": "permission_prompt"}),
            t + 2,
        );
    }
    // verify_overdue: 1 Cart, claimed done and nudged twice.
    start(&mut s, "r1", "ver00ddd", 40);
    for (kind, data) in [
        ("claimed_done", json!({"task_id": "1", "evidence": "ok"})),
        ("verify_nudged", json!({"task_id": "1", "level": 0})),
        ("verify_nudged", json!({"task_id": "1", "level": 0})),
        ("needs_owner", json!({"kind": "verify", "task_id": "1"})),
    ] {
        ev(&mut s, "r1", "ver00ddd", kind, data, 50);
    }
    // question: the agent ended its turn with a question.
    start(&mut s, "r1", "ask0fff", 70);
    ev(&mut s, "r1", "ask0fff", "prompt", json!({}), 71);
    ev(&mut s, "r1", "ask0fff", "unit_stopped", json!({}), 131);
    ev(
        &mut s,
        "r1",
        "ask0fff",
        "owner_question",
        json!({"unit_no": 1, "source": "turn_end"}),
        131,
    );
    // leftover: a dead session's file.
    ev(
        &mut s,
        "r1",
        "ver00ddd",
        "leftover_found",
        json!({"leftover_id": "l1", "from_session": "dead0eee", "worktree": "/w",
               "path": "a.rs", "suggested": "undo"}),
        60,
    );
    drop(s);
    assert_eq!(
        status(&env),
        "\
6 need you
bridge-screen · 5 live sessions · 1 project
shop  goal: Online shop  · done 0% · claimed 100%
  leftovers: 1 open (0 report-only)
  S red00a  needs you (no task)             1 unit  · 40m · drift ⚠ 40m
  S ask0ff  needs you (no task)             1 unit  · 1m · on task · asked you a question
  S perm0c  needs you (no task)             0 units · 0m · on task
  S perm0b  needs you (no task)             0 units · 0m · on task
  needs you: verify 1 Cart — bridge-screen verify 1 (or --broken \"…\")
  S ver00d  idle      (no task)             0 units · 0m · on task · verify pending: 1
"
    );
}
