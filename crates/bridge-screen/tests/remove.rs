//! Remove for good (spec 2026-11-14 §4): the owner's action, what it cleans up for
//! everyone, and why a plan file can't undo it.

mod common;

use bridge_screen_core::config::Config;
use bridge_screen_core::error::Error;
use bridge_screen_core::owner::{ActCtx, OwnerAction, act};
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{NodeStatus, get_node};
use bridge_screen_core::session;
use bridge_screen_core::store::{Event, EventDraft, Store, read_all};
use common::Env;
use jiff::Timestamp;
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship it","intent":"The product ships"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form","estimate_min":30},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset","estimate_min":20},
  {"op":"add_node","parent":"1.1","kind":"subtask","title":"Validation","estimate_min":10}
]"#;

fn setup() -> Env {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    env.hook("session-start", "s1");
    env
}

fn store(env: &Env) -> Store {
    Store::open(&env.db_path()).unwrap()
}

fn repo(env: &Env) -> String {
    session::get(store(env).conn(), "s1")
        .unwrap()
        .unwrap()
        .repo_id
}

fn events(env: &Env) -> Vec<Event> {
    read_all(store(env).conn(), None).unwrap()
}

fn append(env: &Env, session: &str, kind: &str, data: Value) {
    let mut s = store(env);
    let task = data["task_id"].as_str().map(str::to_string);
    let mut d = EventDraft::new(repo(env), session, kind, data);
    if let Some(t) = task {
        d = d.task(t);
    }
    s.append(d).unwrap();
}

fn act_on(env: &Env, action: &OwnerAction) -> Result<String, Error> {
    let home = Home::at(env.home.path());
    let cfg = Config::default();
    let ctx = ActCtx {
        home: &home,
        cfg: &cfg,
        now: Timestamp::now(),
        dir: None,
    };
    act(&mut store(env), &ctx, action)
}

fn remove(env: &Env, id: &str, title: &str, confirm: bool) -> Result<String, Error> {
    act_on(
        env,
        &OwnerAction::Remove {
            repo: repo(env),
            id: id.into(),
            title: title.into(),
            confirm,
        },
    )
}

fn status(env: &Env, id: &str) -> Option<NodeStatus> {
    get_node(store(env).conn(), &repo(env), id)
        .unwrap()
        .map(|n| n.status)
}

fn glance(env: &Env) -> bridge_screen_core::view::Glance {
    bridge_screen_core::view::glance(store(env).conn(), &Config::default(), Timestamp::now())
        .unwrap()
}

fn pass(env: &Env, task: &str, minutes_ago: i64) {
    let _ = minutes_ago;
    append(
        env,
        "s1",
        "verified",
        json!({"verif_id": format!("v{task}"), "task_id": task, "level": 2, "sha": "abc",
               "files": [], "evidence": "ok", "by": "agent"}),
    );
}

#[test]
fn remove_needs_confirm() {
    let env = setup();
    let err = remove(&env, "1.2", "Password reset", false).unwrap_err();
    assert!(err.to_string().contains("confirm it first"), "{err}");
    assert_eq!(status(&env, "1.2"), Some(NodeStatus::Todo));
}

#[test]
fn remove_stale_when_title_changed_or_gone() {
    let env = setup();
    let err = remove(&env, "1.2", "Something else", true).unwrap_err();
    assert!(matches!(err, Error::Stale(_)), "{err}");
    assert!(
        err.to_string().contains("now 'Password reset'") || err.to_string().contains("changed"),
        "{err}"
    );
    assert_eq!(status(&env, "1.2"), Some(NodeStatus::Todo));
    remove(&env, "1.2", "Password reset", true).unwrap();
    let err = remove(&env, "1.2", "Password reset", true).unwrap_err();
    assert!(matches!(err, Error::Stale(_)), "{err}");
}

#[test]
fn remove_drops_subtree_everywhere() {
    let env = setup();
    pass(&env, "1.2", 0);
    let before = glance(&env);
    let row = &before.projects[0];
    assert_eq!(
        row.done_pct, 66,
        "20 of 30 minutes (1.1 is a parent; its step 1.1.1 counts)"
    );
    let line = remove(&env, "1.1", "Login form", true).unwrap();
    assert_eq!(
        line,
        "Removed task 1.1 'Login form' (it was not started). It has left the bars, the queue and the forecast."
    );
    for id in ["1.1", "1.1.1"] {
        assert_eq!(status(&env, id), Some(NodeStatus::Dropped), "{id}");
    }
    let after = glance(&env);
    // The bar now counts only Password reset: 20 of 20.
    assert_eq!(after.projects[0].done_pct, 100);
    let tree =
        bridge_screen_core::plan::runs::run_tree(store(&env).conn(), &repo(&env), 1).unwrap();
    assert!(tree.iter().all(|n| !n.id.starts_with("1.1")), "{tree:?}");
    let f = bridge_screen_core::forecast::run(
        store(&env).conn(),
        &repo(&env),
        Timestamp::now(),
        &jiff::tz::TimeZone::UTC,
    )
    .unwrap();
    assert_eq!(f.remaining_min, 0);
}

#[test]
fn remove_clears_nudges_and_overdue_card() {
    let env = setup();
    append(
        &env,
        "s1",
        "claimed_done",
        json!({"task_id": "1.2", "evidence": "done"}),
    );
    for _ in 0..2 {
        append(
            &env,
            "s1",
            "verify_nudged",
            json!({"task_id": "1.2", "level": 0}),
        );
    }
    append(
        &env,
        "s1",
        "needs_owner",
        json!({"kind": "verify", "task_id": "1.2"}),
    );
    assert!(
        glance(&env)
            .needs_you
            .iter()
            .any(|n| n.id.starts_with("verify_overdue")),
        "the overdue card is there first"
    );
    remove(&env, "1.2", "Password reset", true).unwrap();
    assert!(
        glance(&env)
            .needs_you
            .iter()
            .all(|n| !n.id.starts_with("verify_overdue"))
    );
    let n: i64 = store(&env)
        .conn()
        .query_row("SELECT count(*) FROM verify_nudges", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn remove_clears_open_waits() {
    let env = setup();
    env.hook("session-start", "s2");
    append(
        &env,
        "s2",
        "waiting_on",
        json!({"wait_id": "W1", "target_repo_id": repo(&env), "target_task": "1.2",
               "target_text": "1.2 Password reset", "reason": "needs the reset flow"}),
    );
    remove(&env, "1.2", "Password reset", true).unwrap();
    let why: Option<String> = store(&env)
        .conn()
        .query_row(
            "SELECT cleared_why FROM waits WHERE wait_id = 'W1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(why.as_deref(), Some("target_removed"));
}

#[test]
fn remove_releases_live_session_task() {
    let env = setup();
    append(
        &env,
        "s1",
        "task_claimed",
        json!({"task_id": "1.1.1", "mode": "auto", "confidence": 0.9}),
    );
    assert_eq!(
        session::get(store(&env).conn(), "s1")
            .unwrap()
            .unwrap()
            .active_task
            .as_deref(),
        Some("1.1.1")
    );
    let text = bridge_screen_core::owner::remove_impact(store(&env).conn(), &repo(&env), "1.1")
        .unwrap()
        .text;
    assert!(text.contains("1 session is working on it"), "{text}");
    remove(&env, "1.1", "Login form", true).unwrap();
    assert_eq!(
        session::get(store(&env).conn(), "s1")
            .unwrap()
            .unwrap()
            .active_task,
        None
    );
    assert!(
        events(&env)
            .iter()
            .any(|e| e.kind == "task_released" && e.data["reason"] == "removed")
    );
}

#[test]
fn agent_remove_node_also_cleans_up() {
    let env = setup();
    append(
        &env,
        "s1",
        "task_claimed",
        json!({"task_id": "1.2", "mode": "auto", "confidence": 0.9}),
    );
    let out = env.run(
        &["plan", "edit", "--reason", "not needed"],
        r#"[{"op":"remove_node","id":"1.2"}]"#,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        session::get(store(&env).conn(), "s1")
            .unwrap()
            .unwrap()
            .active_task,
        None
    );
    // An agent's removal is not "for good": the event is not marked by the owner.
    let removed: Vec<Value> = events(&env)
        .into_iter()
        .filter(|e| e.kind == "node_removed")
        .map(|e| e.data)
        .collect();
    assert!(
        removed.iter().all(|d| d.get("by_owner").is_none()),
        "{removed:?}"
    );
}

#[test]
fn history_says_you_removed_it() {
    let env = setup();
    append(
        &env,
        "s1",
        "claimed_done",
        json!({"task_id": "1.2", "evidence": "done"}),
    );
    remove(&env, "1.2", "Password reset", true).unwrap();
    let page = bridge_screen_core::view::drill::history(
        store(&env).conn(),
        &repo(&env),
        &bridge_screen_core::view::drill::HistoryFilter {
            limit: 100,
            ..Default::default()
        },
    )
    .unwrap();
    let sentences: Vec<&str> = page.events.iter().map(|r| r.summary.as_str()).collect();
    assert!(
        sentences.contains(
            &"You removed task 1.2 'Password reset' for good (it was claimed, not checked)"
        ),
        "{sentences:?}"
    );
}

#[test]
fn remove_track_drops_all_its_nodes_and_bar() {
    let env = setup();
    assert!(
        env.run(&["track", "start", "Docs refresh"], "")
            .status
            .success()
    );
    let ops = r#"[{"op":"add_node","parent":"T2-G","kind":"milestone","title":"Pages","intent":"p"},
                  {"op":"add_node","parent":"T2-1","kind":"task","title":"Readme","estimate_min":10}]"#;
    assert!(env.run(&["plan", "edit"], ops).status.success());
    assert_eq!(glance(&env).projects.len(), 1);
    let text = bridge_screen_core::owner::remove_impact(store(&env).conn(), &repo(&env), "T2-G")
        .unwrap()
        .text;
    assert!(
        text.starts_with("Remove the track 'Docs refresh' for good? 1 task, 0 done."),
        "{text}"
    );
    let line = remove(&env, "T2-G", "Docs refresh", true).unwrap();
    assert!(
        line.contains("track 'Docs refresh' (1 task, 0 done)"),
        "{line}"
    );
    for id in ["T2-G", "T2-1", "T2-1.1"] {
        assert_eq!(status(&env, id), Some(NodeStatus::Dropped), "{id}");
    }
    let open =
        bridge_screen_core::plan::track::open_tracks(store(&env).conn(), &repo(&env)).unwrap();
    assert_eq!(open.len(), 1);
    // The number is not reused.
    let out = env.run(&["track", "start", "Third"], "");
    assert!(String::from_utf8_lossy(&out.stdout).contains("track 3 'Third' (goal T3-G)"));
}

#[test]
fn cost_keeps_the_spend() {
    let env = setup();
    append(
        &env,
        "s1",
        "usage",
        json!({"path": "/t", "offset_after": 1, "replies": [
            {"message_id": "a", "model": "claude-opus-5-5", "ts": Timestamp::now().to_string(),
             "output": 100}]}),
    );
    append(
        &env,
        "s1",
        "task_claimed",
        json!({"task_id": "1.2", "mode": "auto", "confidence": 0.9}),
    );
    let before = bridge_screen_core::telemetry::totals(
        store(&env).conn(),
        &bridge_screen_core::telemetry::Scope::Repo(repo(&env)),
    )
    .unwrap();
    remove(&env, "1.2", "Password reset", true).unwrap();
    let after = bridge_screen_core::telemetry::totals(
        store(&env).conn(),
        &bridge_screen_core::telemetry::Scope::Repo(repo(&env)),
    )
    .unwrap();
    assert_eq!(before, after);
}

#[test]
fn finished_removed_task_still_teaches_calibration() {
    let env = setup();
    // Three finished tasks teach the ratio; one of them is removed afterwards.
    for t in ["1.2", "1.1.1"] {
        append(
            &env,
            "s1",
            "task_claimed",
            json!({"task_id": t, "mode": "auto", "confidence": 0.9}),
        );
        pass(&env, t, 0);
    }
    let cal = |env: &Env| {
        bridge_screen_core::forecast::calibration(store(env).conn(), &repo(env), Timestamp::now())
            .unwrap()
            .pooled
            .len()
    };
    let before = cal(&env);
    remove(&env, "1.2", "Password reset", true).unwrap();
    assert_eq!(
        cal(&env),
        before,
        "a finished task the owner removed keeps teaching"
    );
}

#[test]
fn card_nonce_refuses_remove() {
    let a = OwnerAction::Remove {
        repo: "r".into(),
        id: "1".into(),
        title: "x".into(),
        confirm: true,
    };
    assert!(!a.allowed_on_card());
}
