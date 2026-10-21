//! The forecast through the CLI (spec §9.3): `bridge-screen forecast`, and the history
//! entry a verification at L2 or above adds.

mod common;

use bridge_screen_core::plan::{EditContext, NodeKind, PlanOp, plan_edit};
use bridge_screen_core::store::{Actor, EventDraft, Store, append_in_at, read_all};
use common::Env;
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

/// A plan of four 30-minute tasks; `done` of them worked on for 30, 60 and 90 minutes
/// in the last hours and verified at L2.
fn seeded(env: &Env, done: usize) -> String {
    let repo = bridge_screen_core::repo::identify(env.repo.path()).unwrap();
    let mut store = Store::open(&env.db_path()).unwrap();
    store.register_repo(&repo).unwrap();
    let add = |parent: Option<&str>, kind, title: &str| PlanOp::AddNode {
        parent: parent.map(str::to_string),
        kind,
        title: title.into(),
        intent: Some("why".into()),
        estimate_min: Some(30),
        expected_files: vec![],
    };
    plan_edit(
        &mut store,
        &EditContext {
            repo_id: repo.id.clone(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: None,
            replan: false,
        },
        &[
            add(None, NodeKind::Goal, "Ship"),
            add(Some("G"), NodeKind::Milestone, "Auth"),
            add(Some("1"), NodeKind::Task, "Login form"),
            add(Some("1"), NodeKind::Task, "Logout"),
            add(Some("1"), NodeKind::Task, "Reset"),
            add(Some("1"), NodeKind::Task, "Audit log"),
        ],
    )
    .unwrap();
    let start = Timestamp::now() - SignedDuration::from_hours(6);
    store
        .write(|tx| {
            for (i, task) in ["1.1", "1.2", "1.3"].iter().take(done).enumerate() {
                let n = i64::try_from(i).unwrap();
                let sid = format!("s{task}");
                let t = |secs: i64| start + SignedDuration::from_secs(n * 7200 + secs);
                let ev = |kind: &str, data| EventDraft::new(&repo.id, &sid, kind, data);
                append_in_at(
                    tx,
                    ev(
                        "session_started",
                        json!({"cwd": env.repo_path(), "source": "startup"}),
                    ),
                    t(0),
                )?;
                append_in_at(
                    tx,
                    ev(
                        "task_claimed",
                        json!({"task_id": task, "mode": "auto", "confidence": 0.9}),
                    ),
                    t(0),
                )?;
                append_in_at(tx, ev("prompt", json!({"chars": 1})), t(1))?;
                append_in_at(
                    tx,
                    ev("unit_stopped", json!({"unit_no": 1})),
                    t(1 + (n + 1) * 1800),
                )?;
                append_in_at(
                    tx,
                    ev(
                        "verified",
                        json!({"verif_id": format!("v{task}"), "task_id": task, "level": 2,
                               "sha": "abc", "files": [], "evidence": "ok", "by": "agent"}),
                    ),
                    t(2 + (n + 1) * 1800),
                )?;
            }
            Ok(())
        })
        .unwrap();
    repo.name
}

fn forecasts(env: &Env) -> usize {
    let store = Store::open(&env.db_path()).unwrap();
    read_all(store.conn(), None)
        .unwrap()
        .iter()
        .filter(|e| e.kind == "forecast_ran")
        .count()
}

fn run(env: &Env, args: &[&str]) -> String {
    let out = env.run(args, "");
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn a_verification_at_l2_records_the_forecast_and_l1_does_not() {
    let env = Env::new();
    seeded(&env, 3);
    run(&env, &["verify", "1.4", "--level", "1"]);
    assert_eq!(forecasts(&env), 0);
    run(&env, &["verify", "1.4", "--level", "2"]);
    assert_eq!(forecasts(&env), 1);
    let store = Store::open(&env.db_path()).unwrap();
    let points: i64 = store
        .conn()
        .query_row("SELECT points FROM forecast_history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(points, 3);
    // Broken is no progress: nothing new to record.
    run(&env, &["verify", "1.4", "--broken", "it crashes"]);
    assert_eq!(forecasts(&env), 1);
}

#[test]
fn forecast_cli_says_the_range_and_its_basis() {
    let env = Env::new();
    let name = seeded(&env, 3);
    let text = run(&env, &["forecast"]);
    assert!(text.starts_with(&format!("{name}: likely by ")), "{text}");
    assert!(text.contains("could take until"), "{text}");
    assert!(
        text.contains("(50% / 90% chance, based on how long verified tasks actually took)"),
        "{text}"
    );
    // Ratios 1, 2 and 3: an average of 2×.
    assert!(
        text.contains("Based on 3 finished tasks; this project averages 2× its estimates."),
        "{text}"
    );
    assert!(text.contains("30m of estimated work left"), "{text}");
}

#[test]
fn forecast_cli_refuses_thin_data() {
    let env = Env::new();
    let name = seeded(&env, 1);
    let text = run(&env, &["forecast"]);
    assert!(
        text.starts_with(&format!(
            "{name}: no dates yet. Not enough finished tasks to forecast yet — 2 more are needed."
        )),
        "{text}"
    );
    assert!(!text.contains("likely by"), "{text}");
}

/// One live session (through the hooks), so `status` lists the project.
fn live(env: &Env) {
    env.hook("session-start", "live01");
}

#[test]
fn status_and_mcp_status_show_the_forecast_and_cost() {
    let env = Env::new();
    let name = seeded(&env, 3);
    live(&env);
    let repo = bridge_screen_core::repo::identify(env.repo.path()).unwrap();
    let mut store = Store::open(&env.db_path()).unwrap();
    let ts = Timestamp::now().to_string();
    store
        .append(EventDraft::new(
            &repo.id,
            "live01",
            "usage",
            json!({"path": "/t/live01.jsonl", "offset_after": 5, "replies": [
                {"message_id": "m1", "model": "claude-opus-5-5", "ts": ts,
                 "input": 1_000_000, "output": 100_000}]}),
        ))
        .unwrap();
    drop(store);
    let status = run(&env, &["status"]);
    let line = status
        .lines()
        .find(|l| l.starts_with(&format!("{name}  goal: Ship")))
        .unwrap_or_else(|| panic!("{status}"));
    assert!(line.contains(" · likely "), "{line}");
    assert!(line.ends_with(" · $6.00"), "{line}");

    let mut mcp = common::Mcp::start(&env, &env.repo_path());
    let (err, text) = common::call_as(&mut mcp, &env, "live01", "status", &json!({}));
    assert!(!err, "{text}");
    assert!(
        text.contains("Cost so far: this session $6.00 (1.1M tokens)"),
        "{text}"
    );
    assert!(text.contains("Forecast: likely by "), "{text}");
    assert!(
        text.contains("(based on 3 finished tasks; this project averages 2× its estimates)"),
        "{text}"
    );
}

#[test]
fn an_unpriced_model_is_named_in_status() {
    let env = Env::new();
    seeded(&env, 0);
    live(&env);
    let repo = bridge_screen_core::repo::identify(env.repo.path()).unwrap();
    let mut store = Store::open(&env.db_path()).unwrap();
    store
        .append(EventDraft::new(
            &repo.id,
            "live01",
            "usage",
            json!({"path": "/t/x.jsonl", "offset_after": 5, "replies": [
                {"message_id": "m1", "model": "mystery-model", "ts": Timestamp::now().to_string(),
                 "output": 5000}]}),
        ))
        .unwrap();
    drop(store);
    let status = run(&env, &["status"]);
    assert!(status.contains(" · cost unknown"), "{status}");
    assert!(
        status.contains("  no price for mystery-model: add one under [prices] in config.toml"),
        "{status}"
    );
    assert!(
        status.contains("forecast needs 3 more finished tasks"),
        "{status}"
    );
}
