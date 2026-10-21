//! `bridge-screen hook <event>`: the binary run the way Claude Code runs it.

mod common;

use common::{EVENTS, Env};

/// Hook stdout is empty or exactly one JSON object in Claude Code's hook output format.
fn assert_hook_output(event: &str, out: &str) {
    if out.is_empty() {
        return;
    }
    assert_eq!(out.lines().count(), 1, "{event}: {out}");
    let v: serde_json::Value = serde_json::from_str(out).unwrap();
    let pascal: String = event
        .split('-')
        .map(|w| w[..1].to_uppercase() + &w[1..])
        .collect();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], pascal.as_str());
    assert!(v["hookSpecificOutput"]["additionalContext"].is_string());
}

#[test]
fn each_event_is_accepted() {
    let env = Env::new();
    for event in EVENTS {
        let out = env.hook(event, "s-accept");
        assert_hook_output(event, &out);
        if !matches!(*event, "session-start" | "user-prompt-submit") {
            assert_eq!(out, "", "{event} printed something");
        }
    }
    assert_eq!(env.log(), "", "unexpected hook errors");
}

#[test]
fn hook_never_fails_the_session() {
    // (a) stdin that isn't JSON
    let env = Env::new();
    assert_eq!(env.hook_raw("user-prompt-submit", "not json"), "");
    let log = env.log();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("user-prompt-submit"), "{log}");

    // (b) BRIDGE_SCREEN_HOME is a file, not a folder: nowhere to log, still exit 0
    let env = Env::new();
    let file = env.home.path().join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    let mut env_b = Env::new();
    env_b.vars.push((
        "BRIDGE_SCREEN_HOME".into(),
        file.to_str().unwrap().to_string(),
    ));
    assert_eq!(env_b.hook("session-start", "s-b"), "");

    // (c) a database file full of garbage
    let env = Env::new();
    std::fs::write(env.db_path(), vec![0xAB_u8; 8192]).unwrap();
    assert_eq!(env.hook("stop", "s-c"), "");
    let log = env.log();
    assert!(log.contains(" stop "), "{log}");
}

#[test]
fn unknown_or_missing_hook_event_exits_zero() {
    // Exit code 2 would block the tool call or prompt; any bad command line must exit 0.
    let env = Env::new();
    for args in [
        &["hook", "brand-new-event"][..],
        &["hook"],
        &["hook", "stop", "x"],
    ] {
        let out = env.run(args, "{}");
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}");
    }
    let log = env.log();
    assert_eq!(log.lines().count(), 3, "{log}");
    assert!(log.contains("brand-new-event"), "{log}");
}

#[test]
fn errors_are_logged_before_the_home_folder_exists() {
    let env = Env::new();
    let home = env.home.path().join("not-yet");
    let mut env = env;
    env.vars
        .push(("BRIDGE_SCREEN_HOME".into(), home.to_str().unwrap().into()));
    assert_eq!(env.hook_raw("stop", "not json"), "");
    let log = std::fs::read_to_string(home.join("hook-errors.log")).unwrap_or_default();
    assert!(log.contains(" stop "), "{log:?}");
}

use bridge_screen_core::session::{self, SessionState};
use bridge_screen_core::store::{Event, Store, read_all};
use common::fixture_json;
use serde_json::{Value, json};

fn events(env: &Env) -> Vec<Event> {
    let store = Store::open(&env.db_path()).unwrap();
    read_all(store.conn(), None).unwrap()
}

fn kinds(env: &Env, session: &str) -> Vec<String> {
    events(env)
        .into_iter()
        .filter(|e| e.session_id == session)
        .map(|e| e.kind)
        .collect()
}

/// Send a fixture for `event` after changing it with `edit`.
fn hook_with(env: &Env, event: &str, session: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut payload = fixture_json(event, session, &env.repo_path());
    edit(&mut payload);
    env.hook_raw(event, &payload.to_string())
}

fn edit_file(env: &Env, event: &str, session: &str, file: &str) {
    let path = format!("{}/{file}", env.repo_path());
    hook_with(env, event, session, |p| {
        p["tool_input"]["file_path"] = json!(path);
    });
}

fn units(env: &Env, session: &str) -> Vec<session::Unit> {
    let store = Store::open(&env.db_path()).unwrap();
    session::units(store.conn(), session).unwrap()
}

#[test]
fn session_flow_records_events() {
    let env = Env::new();
    for event in [
        "session-start",
        "user-prompt-submit",
        "pre-tool-use",
        "post-tool-use",
        "stop",
        "session-end",
    ] {
        assert_hook_output(event, &env.hook(event, "s1"));
    }
    assert_eq!(env.log(), "");
    assert_eq!(
        kinds(&env, "s1"),
        vec![
            "session_started",
            "briefed",
            "prompt",
            "tool_intent",
            "file_changed",
            "unit_stopped",
            "unit_labeled",
            "session_ended"
        ]
    );
    let all = events(&env);
    let prompt = &all[2].data;
    assert_eq!(prompt["chars"], 29);
    assert!(
        !all[2].data.to_string().contains("login form"),
        "prompt text must never be stored"
    );
    assert_eq!(all[3].data["files"], json!(["src/login.rs"]));
    assert_eq!(all[5].data["unit_no"], 1);
    assert_eq!(
        all[5].data["summary"],
        "Added validation to the login form and a unit test."
    );
    assert_eq!(all[6].data["label"], "on_task");
    assert_eq!(all[7].data["reason"], "prompt_input_exit");
    let u = units(&env, "s1");
    assert_eq!(u.len(), 1);
    assert_eq!(u[0].files, vec!["src/login.rs"]);
    let store = Store::open(&env.db_path()).unwrap();
    let s = session::get(store.conn(), "s1").unwrap().unwrap();
    assert_eq!(s.state, SessionState::Ended);
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.model.as_deref(), Some("claude-opus-5-5"));
}

#[test]
fn repeated_intent_for_the_same_file_is_recorded_once_per_unit() {
    let env = Env::new();
    env.hook("session-start", "s1");
    env.hook("user-prompt-submit", "s1");
    env.hook("pre-tool-use", "s1");
    env.hook("pre-tool-use", "s1");
    edit_file(&env, "pre-tool-use", "s1", "src/other.rs");
    env.hook("stop", "s1");
    env.hook("user-prompt-submit", "s1");
    env.hook("pre-tool-use", "s1");
    let intents = kinds(&env, "s1")
        .into_iter()
        .filter(|k| k == "tool_intent")
        .count();
    assert_eq!(intents, 3);
}

#[test]
fn parallel_sessions_stay_separate() {
    let env = Env::new();
    env.hook("session-start", "A");
    env.hook("user-prompt-submit", "A");
    env.hook("session-start", "B");
    env.hook("user-prompt-submit", "B");
    edit_file(&env, "post-tool-use", "A", "src/a.rs");
    edit_file(&env, "post-tool-use", "B", "src/b.rs");
    // B's second prompt closes B's unit, not A's.
    env.hook("user-prompt-submit", "B");
    edit_file(&env, "post-tool-use", "A", "src/a2.rs");
    env.hook("stop", "A");
    env.hook("stop", "B");

    let a = units(&env, "A");
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].files, vec!["src/a.rs", "src/a2.rs"]);
    let b = units(&env, "B");
    assert_eq!(b.len(), 2);
    assert_eq!((b[0].unit_no, b[1].unit_no), (1, 2));
    assert_eq!(b[0].files, vec!["src/b.rs"]);
    assert_eq!(b[1].files, Vec::<String>::new());
    let seqs: Vec<i64> = events(&env)
        .into_iter()
        .filter(|e| e.session_id == "A")
        .map(|e| e.seq)
        .collect();
    // session_started, briefed, prompt, 2 × file_changed, unit_stopped, unit_labeled
    assert_eq!(seqs, (1..=7).collect::<Vec<_>>());
}

#[test]
fn non_git_directory_is_ignored() {
    let env = Env::new();
    let plain = tempfile::tempdir().unwrap();
    let cwd = plain.path().to_str().unwrap();
    for event in common::EVENTS {
        let out = env.hook_raw(event, &common::fixture(event, "s1", cwd));
        assert_eq!(out, "");
    }
    assert_eq!(env.log(), "");
    assert_eq!(events(&env), vec![]);
}

#[cfg(unix)]
#[test]
fn known_session_does_not_run_git() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut env = Env::new();
    env.hook("session-start", "s1");

    let bin = tempfile::tempdir().unwrap();
    let fake = bin.path().join("git");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho 'fake git must not run' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    env.vars.push(("PATH".into(), path));

    for event in [
        "user-prompt-submit",
        "pre-tool-use",
        "post-tool-use",
        "notification",
        "stop",
    ] {
        env.hook(event, "s1");
    }
    assert_eq!(env.log(), "");
    assert_eq!(
        kinds(&env, "s1"),
        vec![
            "session_started",
            "briefed",
            "prompt",
            "tool_intent",
            "file_changed",
            "needs_owner",
            "unit_stopped",
            "unit_labeled"
        ]
    );
}

#[test]
fn unknown_session_is_created_implicitly() {
    let env = Env::new();
    env.hook("user-prompt-submit", "late");
    assert_eq!(kinds(&env, "late"), vec!["session_started", "prompt"]);
    let store = Store::open(&env.db_path()).unwrap();
    let s = session::get(store.conn(), "late").unwrap().unwrap();
    assert_eq!(s.source, "unknown");
    assert_eq!(s.state, SessionState::Working);

    // First seen on a tool that records nothing: still tracked from then on, so later
    // hooks take the known-session path and don't run git again.
    hook_with(&env, "pre-tool-use", "later", |p| {
        p["tool_name"] = json!("Read");
        p["tool_input"] = json!({"file_path": "/etc/hosts"});
    });
    assert_eq!(kinds(&env, "later"), vec!["session_started"]);
}

#[test]
fn subagent_events_carry_actor() {
    let env = Env::new();
    env.hook("session-start", "s1");
    env.hook("user-prompt-submit", "s1");
    hook_with(&env, "post-tool-use", "s1", |p| {
        p["agent_id"] = json!("agent-7");
        p["agent_type"] = json!("Explore");
    });
    env.hook("subagent-stop", "s1");
    let all = events(&env);
    let changed = all.iter().find(|e| e.kind == "file_changed").unwrap();
    assert_eq!(changed.actor.agent_id.as_deref(), Some("agent-7"));
    assert_eq!(changed.actor.agent_type.as_deref(), Some("Explore"));
    let stopped = all.iter().find(|e| e.kind == "subagent_stopped").unwrap();
    assert_eq!(stopped.actor.agent_id.as_deref(), Some("def456"));
    assert_eq!(stopped.data["agent_type"], "Explore");
    let prompt = all.iter().find(|e| e.kind == "prompt").unwrap();
    assert_eq!(prompt.actor.agent_id, None);
}

#[test]
fn notification_kinds() {
    let env = Env::new();
    env.hook("session-start", "s1");
    // Only notifications that mean "waiting on the owner" are recorded.
    for kind in [
        "permission_prompt",
        "auth_success",
        "idle_prompt",
        "agent_completed",
        "elicitation_dialog",
        "elicitation_complete",
        "something_new",
    ] {
        hook_with(&env, "notification", "s1", |p| {
            p["notification_type"] = json!(kind);
        });
    }
    let got: Vec<Value> = events(&env)
        .into_iter()
        .filter(|e| e.kind == "needs_owner")
        .map(|e| e.data["kind"].clone())
        .collect();
    assert_eq!(
        got,
        vec![
            json!("permission_prompt"),
            json!("idle_prompt"),
            json!("other")
        ]
    );
}

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form validation","estimate_min":30},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset"}
]"#;

fn seed_plan(env: &Env) {
    let out = env.run(&["plan", "edit"], SEED);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn context_of(out: &str, event: &str) -> String {
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], event);
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

fn claim_for(env: &Env, session: &str, task: &str) {
    use bridge_screen_core::claim::{ClaimMode, ClaimRequest, ClaimTarget, claim};
    let mut store = Store::open(&env.db_path()).unwrap();
    let repo_id = session::get(store.conn(), session)
        .unwrap()
        .unwrap()
        .repo_id;
    claim(
        &mut store,
        &ClaimRequest {
            session_id: session.into(),
            repo_id,
            task: ClaimTarget::Existing(task.into()),
            mode: ClaimMode::Auto,
            confidence: 0.9,
            actor: bridge_screen_core::store::Actor::default(),
        },
    )
    .unwrap();
}

#[test]
fn session_start_without_a_plan_asks_for_one() {
    let env = Env::new();
    let out = env.hook("session-start", "s1");
    assert_eq!(
        context_of(&out, "SessionStart"),
        "bridge-screen briefing — stable\nNo plan yet. If this repo has work already, run `bridge-screen init` and follow it; otherwise ask the owner what the goal is."
    );
    // No plan, no claim reminders.
    for _ in 0..4 {
        assert_eq!(env.hook("user-prompt-submit", "s1"), "");
    }
}

#[test]
fn session_start_names_goal_and_active_task() {
    let env = Env::new();
    seed_plan(&env);
    let out = env.hook("session-start", "s1");
    assert_eq!(
        context_of(&out, "SessionStart"),
        "bridge-screen briefing — stable\nGoal: Ship — Owner sees progress\nClaim your task with claim_task and go."
    );
    claim_for(&env, "s1", "1.1");
    let resumed = hook_with(&env, "session-start", "s1", |p| {
        p["source"] = json!("resume");
    });
    assert_eq!(
        context_of(&resumed, "SessionStart"),
        "bridge-screen briefing — stable\nGoal: Ship — Owner sees progress\nActive task: 1.1 Login form validation — carry on."
    );
}

#[test]
fn claim_reminder_cadence() {
    let env = Env::new();
    seed_plan(&env);
    env.hook("session-start", "s1");
    let mut reminded = Vec::new();
    for n in 1..=5 {
        let out = env.hook("user-prompt-submit", "s1");
        if !out.is_empty() {
            reminded.push(n);
            if n == 1 {
                assert_eq!(
                    context_of(&out, "UserPromptSubmit"),
                    "bridge-screen: no active task yet. Likely matches for this request:\n  1.1 Login form validation (score .75)\nIf one clearly fits, call claim_task with mode \"auto\". If unsure where the work stands, ask the owner. If none fits, claim a new task under the right parent."
                );
            }
        }
        env.hook("stop", "s1");
    }
    assert_eq!(reminded, vec![1, 4]);
    claim_for(&env, "s1", "1.2");
    for _ in 6..=8 {
        assert_eq!(env.hook("user-prompt-submit", "s1"), "");
    }
    // The prompt text is used for ranking only, never stored.
    let stored = events(&env)
        .iter()
        .map(|e| e.data.to_string())
        .collect::<String>();
    assert!(!stored.contains("Add the login form validation"));
}

/// 50 sessions and 100k events in `env`'s database, like a long-used install.
fn big_database(env: &Env) -> bridge_screen_core::repo::RepoInfo {
    use bridge_screen_core::store::{EventDraft, append_in};

    const SESSIONS: usize = 50;
    const EVENTS: usize = 100_000;

    let repo = bridge_screen_core::repo::identify(env.repo.path()).unwrap();
    let mut store = Store::open(&env.db_path()).unwrap();
    store.register_repo(&repo).unwrap();
    let per_session = EVENTS / SESSIONS;
    for n in 0..SESSIONS {
        let sid = format!("perf-session-{n:02}");
        store
            .write(|tx| {
                let ev = |kind: &str, data: Value| EventDraft::new(&repo.id, &sid, kind, data);
                append_in(
                    tx,
                    ev(
                        "session_started",
                        json!({"cwd": env.repo_path(), "source": "startup"}),
                    ),
                )?;
                let mut written = 1;
                let mut unit = 0;
                while written < per_session {
                    unit += 1;
                    append_in(
                        tx,
                        ev(
                            "prompt",
                            json!({"prompt_id": format!("p{unit}"), "chars": 80}),
                        ),
                    )?;
                    for f in 0..6 {
                        let files = json!([format!("src/mod{}/file{f}.rs", unit % 20)]);
                        append_in(
                            tx,
                            ev("tool_intent", json!({"tool": "Edit", "files": files})),
                        )?;
                        append_in(
                            tx,
                            ev("file_changed", json!({"tool": "Edit", "files": files})),
                        )?;
                    }
                    append_in(
                        tx,
                        ev("unit_stopped", json!({"unit_no": unit, "summary": "done"})),
                    )?;
                    written += 14;
                }
                Ok(())
            })
            .unwrap();
    }
    let total: i64 = store
        .conn()
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert!(total >= i64::try_from(EVENTS).unwrap(), "{total}");
    repo
}

/// min / p50 / p95 / max of 200 timings, printed; p95 must stay under 50 ms.
fn report(label: &str, times: Vec<std::time::Duration>) {
    report_within(label, times, 50);
}

/// [`report`] with a different p95 budget (any number of timings).
fn report_within(label: &str, mut times: Vec<std::time::Duration>, budget_ms: u64) {
    use std::time::Duration;
    let n = times.len();
    assert!(n >= 20, "{n} timings");
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let p95 = times[n * 95 / 100 - 1];
    eprintln!(
        "{label} over {n} calls: min {:.1} ms, p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms(times[0]),
        ms(times[n / 2 - 1]),
        ms(p95),
        ms(times[n - 1])
    );
    assert!(
        p95 < Duration::from_millis(budget_ms),
        "{label}: p95 {:.1} ms",
        ms(p95)
    );
}

/// Hook latency on a large database (spec §11.1, §14): p95 must stay under 50 ms.
/// Release build only; run with
/// `cargo test --release -p bridge-screen --test hooks -- --ignored hook_latency --nocapture`.
#[test]
#[ignore = "perf check: run on demand with --release"]
fn hook_latency() {
    use std::time::Instant;

    let env = Env::new();
    big_database(&env);
    let session = "perf-session-07";
    env.hook("user-prompt-submit", session);
    let payload = common::fixture("post-tool-use", session, &env.repo_path());
    let times = (0..200)
        .map(|_| {
            let start = Instant::now();
            let out = env.hook_raw("post-tool-use", &payload);
            let took = start.elapsed();
            assert_eq!(out, "");
            took
        })
        .collect();
    assert_eq!(env.log(), "");
    report("hook post-tool-use", times);
}

/// The Stop path scores the unit against the plan and its history: a unit of 30 files on
/// a plan of 300 nodes, on the same large database.
#[test]
#[ignore = "perf check: run on demand with --release"]
fn hook_latency_stop_path() {
    use bridge_screen_core::plan::{EditContext, NodeKind, PlanOp, plan_edit};
    use bridge_screen_core::store::{Actor, EventDraft, append_in};
    use std::time::Instant;

    let env = Env::new();
    let repo = big_database(&env);
    let mut ops = vec![PlanOp::AddNode {
        parent: None,
        kind: NodeKind::Goal,
        title: "Ship the shop".into(),
        intent: Some("People can buy things and see their orders.".into()),
        estimate_min: None,
        expected_files: vec![],
    }];
    for m in 1..=10 {
        ops.push(PlanOp::AddNode {
            parent: Some("G".into()),
            kind: NodeKind::Milestone,
            title: format!("Milestone {m} storefront"),
            intent: Some("Part of the shop works end to end.".into()),
            estimate_min: None,
            expected_files: vec![],
        });
        for t in 1..=29 {
            ops.push(PlanOp::AddNode {
                parent: Some(m.to_string()),
                kind: NodeKind::Task,
                title: format!("Task {t} of milestone {m} checkout"),
                intent: None,
                estimate_min: Some(30),
                expected_files: vec![format!("src/mod{m}/task{t}.rs")],
            });
        }
    }
    let mut store = Store::open(&env.db_path()).unwrap();
    plan_edit(
        &mut store,
        &EditContext {
            repo_id: repo.id.clone(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: None,
            replan: false,
        },
        &ops,
    )
    .unwrap();
    let nodes: i64 = store
        .conn()
        .query_row("SELECT count(*) FROM plan_nodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(nodes, 301);
    let session = "perf-session-07";
    claim_for(&env, session, "3.5");

    let mut payload = common::fixture_json("stop", session, &env.repo_path());
    payload["last_assistant_message"] = json!(
        "Wired the checkout task into the storefront, added tests for the order summary and fixed the cart total rounding."
    );
    let payload = payload.to_string();
    // The first exec of a freshly built binary costs ~250 ms on macOS (the OS checks it
    // once per launch context, even for `--version`); it isn't the hook's work.
    assert!(env.run(&["--version"], "").status.success());
    let times = (0..200)
        .map(|i| {
            // A fresh unit of 30 changed files, recorded directly to keep the setup quick.
            store
                .write(|tx| {
                    let ev =
                        |kind: &str, data: Value| EventDraft::new(&repo.id, session, kind, data);
                    append_in(tx, ev("prompt", json!({"chars": 40})))?;
                    for f in 0..30 {
                        let file = if f % 3 == 0 {
                            format!("docs/notes{i}_{f}.md")
                        } else {
                            format!("src/mod3/part{f}.rs")
                        };
                        append_in(
                            tx,
                            ev("file_changed", json!({"tool": "Edit", "files": [file]})),
                        )?;
                    }
                    Ok(())
                })
                .unwrap();
            let start = Instant::now();
            env.hook_raw("stop", &payload);
            start.elapsed()
        })
        .collect();
    assert_eq!(env.log(), "");
    let labelled: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM units WHERE session_id = ?1 AND drift_score IS NOT NULL",
            [session],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(labelled, 200, "every unit was scored");
    report("hook stop (30 files, 300-node plan)", times);
}

/// 50 committed files and 5 symbols in them, for a claim: the L1 check at a commit then
/// does its full git work.
fn committed_evidence(env: &Env) -> (Vec<String>, Vec<String>) {
    let files: Vec<String> = (0..50).map(|n| format!("src/f{n}.rs")).collect();
    std::fs::create_dir_all(env.repo.path().join("src")).unwrap();
    for (n, f) in files.iter().enumerate() {
        std::fs::write(env.repo.path().join(f), format!("fn sym{n}() {{}}\n")).unwrap();
    }
    common::git(env.repo.path(), &["add", "-A"]);
    common::git(env.repo.path(), &["commit", "-q", "-m", "files"]);
    (files, (0..5).map(|n| format!("sym{n}")).collect())
}

/// A failed verification of `task`: it is back to work, so its next claim is unverified.
fn back_to_work(store: &mut Store, repo_id: &str, session: &str, task: &str) {
    use bridge_screen_core::store::EventDraft;
    let data = json!({"task_id": task, "level": 1, "evidence": "again", "by": "owner"});
    store
        .append(EventDraft::new(repo_id, session, "verification_failed", data).task(task))
        .unwrap();
}

/// PostToolUse after a Bash `git commit` while a checkpoint is pending: git runs here
/// (HEAD, its trailers), a checkpoint is recorded and the claimed task's L1 is checked
/// (50 files, 5 symbols). Budget p95 < 150 ms.
#[test]
#[ignore = "perf check: run on demand with --release"]
fn hook_latency_commit_path() {
    use bridge_screen_core::checkpoint::{ClaimDoneRequest, claim_done};
    use bridge_screen_core::plan::{EditContext, NodeKind, PlanOp, plan_edit};
    use bridge_screen_core::store::Actor;
    use std::time::Instant;

    let env = Env::new();
    let repo = big_database(&env);
    let mut store = Store::open(&env.db_path()).unwrap();
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
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                title: "Ship".into(),
                intent: Some("Ship it.".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            PlanOp::AddNode {
                parent: Some("G".into()),
                kind: NodeKind::Task,
                title: "Login".into(),
                intent: None,
                estimate_min: Some(30),
                expected_files: vec![],
            },
        ],
    )
    .unwrap();
    let session = "perf-session-07";
    claim_for(&env, session, "1");
    let (files, symbols) = committed_evidence(&env);
    let mut payload = common::fixture_json("post-tool-use", session, &env.repo_path());
    payload["tool_name"] = json!("Bash");
    payload["tool_input"] = json!({"command": "git commit -q -m 'wip(bridge): 1 Login'"});
    let payload = payload.to_string();
    assert!(env.run(&["--version"], "").status.success());
    let times = (0..200)
        .map(|i| {
            back_to_work(&mut store, &repo.id, session, "1");
            claim_done(
                &mut store,
                &ClaimDoneRequest {
                    session_id: session.into(),
                    task_id: None,
                    evidence: "tests".into(),
                    files: files.clone(),
                    symbols: symbols.clone(),
                    actor: Actor::default(),
                },
            )
            .unwrap();
            std::fs::write(env.repo.path().join("f.txt"), format!("{i}\n")).unwrap();
            common::git(env.repo.path(), &["add", "-A"]);
            common::git(
                env.repo.path(),
                &[
                    "commit",
                    "-q",
                    "-m",
                    "wip(bridge): 1 Login",
                    "-m",
                    &format!("Bridge-Task: 1\nBridge-Next: step {i}"),
                ],
            );
            let start = Instant::now();
            assert_eq!(env.hook_raw("post-tool-use", &payload), "");
            start.elapsed()
        })
        .collect();
    assert_eq!(env.log(), "");
    let recorded: i64 = store
        .conn()
        .query_row("SELECT count(*) FROM checkpoints", [], |r| r.get(0))
        .unwrap();
    assert_eq!(recorded, 200, "every commit was recorded");
    let l1: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM verifications WHERE level = 1 AND verified_by = 'auto'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(l1, 200, "every commit reached L1");
    report_within(
        "hook post-tool-use git commit (checkpoint pending)",
        times,
        150,
    );
}

#[test]
fn permission_wait_ends_when_any_tool_runs_again() {
    // PreToolUse(Bash) → permission dialog → Notification → approved → PostToolUse(Bash).
    // Bash isn't an edit tool, but its PostToolUse must still end the wait.
    let env = Env::new();
    env.hook("session-start", "s1");
    env.hook("user-prompt-submit", "s1");
    let bash = |p: &mut Value| {
        p["tool_name"] = json!("Bash");
        p["tool_input"] = json!({"command": "cargo test"});
    };
    hook_with(&env, "pre-tool-use", "s1", bash);
    hook_with(&env, "notification", "s1", |p| {
        p["notification_type"] = json!("permission_prompt");
    });
    let unit = |env: &Env| units(env, "s1").pop().unwrap();
    assert!(unit(&env).wait_started_at.is_some());
    hook_with(&env, "post-tool-use", "s1", bash);
    assert_eq!(unit(&env).wait_started_at, None);
    let store = Store::open(&env.db_path()).unwrap();
    let s = session::get(store.conn(), "s1").unwrap().unwrap();
    assert_eq!(s.state, SessionState::Working);
    // Outside a wait, non-edit tools record nothing.
    let before = events(&env).len();
    hook_with(&env, "post-tool-use", "s1", bash);
    hook_with(&env, "pre-tool-use", "s1", bash);
    assert_eq!(events(&env).len(), before);
}

/// UserPromptSubmit when the navigator has verification and dependency-ready candidates
/// (SQL only, no git): p95 under 50 ms on the large database.
#[test]
#[ignore = "perf check: run on demand with --release"]
fn hook_latency_prompt_with_verify_notes() {
    use bridge_screen_core::plan::{EditContext, NodeKind, PlanOp, plan_edit};
    use bridge_screen_core::store::{Actor, EventDraft};
    use bridge_screen_core::verify::{By, verified_draft};
    use std::time::Instant;

    let env = Env::new();
    let repo = big_database(&env);
    let mut store = Store::open(&env.db_path()).unwrap();
    let task = |title: &str| PlanOp::AddNode {
        parent: Some("G".into()),
        kind: NodeKind::Task,
        title: title.into(),
        intent: None,
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
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                title: "Ship".into(),
                intent: Some("Ship it.".into()),
                estimate_min: None,
                expected_files: vec![],
            },
            task("Login"),
            task("Reset"),
            task("Audit"),
            PlanOp::AddDep {
                id: "1".into(),
                depends_on: "2".into(),
                repo: None,
            },
        ],
    )
    .unwrap();
    let session = "perf-session-07";
    // 2 is verified by another session; this session claimed 3 done earlier.
    store
        .append(
            verified_draft(
                &repo.id,
                "perf-session-03",
                "2",
                2,
                "abc",
                vec![],
                "ok",
                By::Agent,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    store
        .append(EventDraft::new(
            &repo.id,
            session,
            "claimed_done",
            json!({"task_id": "3", "evidence": "done"}),
        ))
        .unwrap();
    claim_for(&env, session, "1");
    let payload = common::fixture("user-prompt-submit", session, &env.repo_path());
    assert!(env.run(&["--version"], "").status.success());
    let times = (0..200)
        .map(|_| {
            let start = Instant::now();
            env.hook_raw("user-prompt-submit", &payload);
            start.elapsed()
        })
        .collect();
    assert_eq!(env.log(), "");
    let sent: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM notes_sent WHERE session_id = ?1
             AND kind IN ('verify_nudge', 'dependency_ready')",
            [session],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sent >= 2, "both notes went out ({sent})");
    report(
        "hook user-prompt-submit (verify and dependency notes)",
        times,
    );
}

/// SessionStart with 50 verifications at 50 different commits, none stale: the staleness
/// scan (one `git log` for all of them) on the large database. Budget p95 < 150 ms, as
/// for the other git paths.
#[test]
#[ignore = "perf check: run on demand with --release"]
fn hook_latency_session_start_staleness() {
    use bridge_screen_core::verify::{By, verified_draft};
    use std::time::Instant;

    let env = Env::new();
    let repo = big_database(&env);
    let mut store = Store::open(&env.db_path()).unwrap();
    for i in 0..50 {
        let file = format!("f{i}.txt");
        std::fs::write(env.repo.path().join(&file), "x\n").unwrap();
        common::git(env.repo.path(), &["add", "-A"]);
        common::git(env.repo.path(), &["commit", "-q", "-m", &file]);
        let out = common::git_cmd(env.repo.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let sha = String::from_utf8(out.stdout).unwrap().trim().to_string();
        store
            .append(
                verified_draft(
                    &repo.id,
                    "perf-session-01",
                    &format!("t{i}"),
                    2,
                    &sha,
                    vec![file],
                    "ok",
                    By::Agent,
                    None,
                )
                .unwrap(),
            )
            .unwrap();
    }
    assert!(env.run(&["--version"], "").status.success());
    let times = (0..40)
        .map(|i| {
            let payload =
                common::fixture("session-start", &format!("start-{i:03}"), &env.repo_path());
            let start = Instant::now();
            env.hook_raw("session-start", &payload);
            start.elapsed()
        })
        .collect();
    assert_eq!(env.log(), "");
    let stale: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'verification_stale'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
    report_within(
        "hook session-start (50 verifications, staleness scan)",
        times,
        1000,
    );
}

/// A plan file with `tasks` tasks of three steps, `pad` bytes of text per step.
fn plan_file(tasks: usize, pad: usize, tag: &str) -> String {
    use std::fmt::Write as _;
    let mut out = format!("# Perf {tag} Implementation Plan\n\n**Goal:** Fast.\n\n");
    for t in 1..=tasks {
        let _ = writeln!(out, "### Task {t}: Task {t} {tag}\n");
        for k in 1..=3 {
            let _ = writeln!(out, "- [ ] **Step {k}: do {}**", "x".repeat(pad));
        }
        out.push('\n');
    }
    out
}

/// SessionStart with 20 plan files that haven't changed (stat only, no reads) and with one
/// 300 KiB plan file that changed since the last SessionStart (read, diffed, imported).
/// Budget p95 < 150 ms, as for SessionStart's other work.
#[test]
#[ignore = "perf check: run on demand with --release"]
fn hook_latency_session_start_plan_import() {
    use std::time::Instant;

    let env = Env::new();
    big_database(&env);
    let dir = env.repo.path().join("docs/superpowers/plans");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..20 {
        std::fs::write(
            dir.join(format!("p{i:02}.md")),
            plan_file(8, 20, &format!("{i}")),
        )
        .unwrap();
    }
    assert!(env.run(&["--version"], "").status.success());
    // The first SessionStart imports them all; after that they are unchanged.
    env.hook("session-start", "first-start");
    let unchanged = (0..40)
        .map(|i| {
            let payload =
                common::fixture("session-start", &format!("same-{i:03}"), &env.repo_path());
            let start = Instant::now();
            env.hook_raw("session-start", &payload);
            start.elapsed()
        })
        .collect();
    report_within(
        "hook session-start (20 unchanged plan files)",
        unchanged,
        150,
    );
    // One big file, changed before every SessionStart (one step renamed each time).
    let big = dir.join("p99-big.md");
    std::fs::remove_file(dir.join("p19.md")).unwrap();
    let base = plan_file(300, 310, "big");
    assert!(base.len() > 300 * 1024, "{}", base.len());
    std::fs::write(&big, &base).unwrap();
    env.hook("session-start", "big-first");
    let changed = (0..40)
        .map(|i| {
            let text = base.replacen("Step 1: do", &format!("Step 1: done {i}"), 1);
            std::fs::write(&big, text).unwrap();
            let payload =
                common::fixture("session-start", &format!("chg-{i:03}"), &env.repo_path());
            let start = Instant::now();
            env.hook_raw("session-start", &payload);
            start.elapsed()
        })
        .collect();
    assert_eq!(env.log(), "");
    let store = Store::open(&env.db_path()).unwrap();
    let imports: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'plan_imported'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(imports, 20 + 1 + 40, "every change imported once");
    report_within(
        "hook session-start (one changed 300 KiB plan file)",
        changed,
        150,
    );
}

/// UserPromptSubmit for a session with 20 open waits and an active task with 5 links into
/// another repo (SQL only): p95 under 50 ms on the large database.
#[test]
#[ignore = "perf check: run on demand with --release"]
fn hook_latency_prompt_with_waits_and_external_deps() {
    use bridge_screen_core::plan::{EditContext, NodeKind, PlanOp, plan_edit};
    use bridge_screen_core::repo::RepoInfo;
    use bridge_screen_core::store::{Actor, EventDraft};
    use std::time::Instant;

    let env = Env::new();
    let repo = big_database(&env);
    let mut store = Store::open(&env.db_path()).unwrap();
    store
        .register_repo(&RepoInfo {
            id: "otherrepo0000000".into(),
            name: "api".into(),
            root: "/nonexistent/api".into(),
        })
        .unwrap();
    let ctx = |repo_id: &str| EditContext {
        repo_id: repo_id.into(),
        session_id: "owner".into(),
        actor: Actor::default(),
        reason: None,
        replan: false,
    };
    let goal = PlanOp::AddNode {
        parent: None,
        kind: NodeKind::Goal,
        title: "Ship".into(),
        intent: Some("Ship it.".into()),
        estimate_min: None,
        expected_files: vec![],
    };
    let task = |title: String| PlanOp::AddNode {
        parent: Some("G".into()),
        kind: NodeKind::Task,
        title,
        intent: None,
        estimate_min: Some(30),
        expected_files: vec![],
    };
    let mut api = vec![goal.clone()];
    api.extend((1..=25).map(|i| task(format!("Endpoint {i}"))));
    plan_edit(&mut store, &ctx("otherrepo0000000"), &api).unwrap();
    let mut web = vec![goal, task("Page".into())];
    web.extend((1..=5).map(|i| PlanOp::AddDep {
        id: "1".into(),
        depends_on: i.to_string(),
        repo: Some("api".into()),
    }));
    plan_edit(&mut store, &ctx(&repo.id), &web).unwrap();
    let session = "perf-session-09";
    claim_for(&env, session, "1");
    for i in 0..20 {
        store
            .append(EventDraft::new(
                &repo.id,
                session,
                "waiting_on",
                json!({"wait_id": format!("W{}", i + 1), "target_repo_id": "otherrepo0000000",
                       "target_task": (i + 6).to_string(), "target_text": format!("api:{}", i + 6),
                       "reason": "needed"}),
            ))
            .unwrap();
    }
    let payload = common::fixture("user-prompt-submit", session, &env.repo_path());
    assert!(env.run(&["--version"], "").status.success());
    let times = (0..200)
        .map(|_| {
            let start = Instant::now();
            env.hook_raw("user-prompt-submit", &payload);
            start.elapsed()
        })
        .collect();
    assert_eq!(env.log(), "");
    report(
        "hook user-prompt-submit (20 waits, 5 links into another repo)",
        times,
    );
}

mod questions {
    //! "The agent asked you a question" (spec §10.1): a needs-you item from the Stop
    //! message or the ask tool, closed by the owner's answer.

    use super::common::{Env, fixture_json};
    use bridge_screen_core::config::Config;
    use bridge_screen_core::store::{Store, read_all};
    use bridge_screen_core::view::{self, NeedsKind};
    use serde_json::json;

    fn queue(env: &Env) -> Vec<(NeedsKind, String)> {
        let store = Store::open(&env.db_path()).unwrap();
        view::glance(store.conn(), &Config::default(), jiff::Timestamp::now())
            .unwrap()
            .needs_you
            .into_iter()
            .map(|i| (i.kind, i.title))
            .collect()
    }

    fn stop_saying(env: &Env, session: &str, message: &str) {
        let mut p = fixture_json("stop", session, &env.repo_path());
        p["last_assistant_message"] = json!(message);
        assert_eq!(env.hook_raw("stop", &p.to_string()), "");
    }

    fn ask_tool(env: &Env, session: &str, event: &str) {
        let mut p = fixture_json(event, session, &env.repo_path());
        p["tool_name"] = json!("AskUserQuestion");
        p["tool_input"] = json!({"questions": [{"question": "Which database should I use?"}]});
        env.hook_raw(event, &p.to_string());
    }

    #[test]
    fn a_turn_ending_with_a_question_needs_the_owner_until_the_next_prompt() {
        let env = Env::new();
        env.hook("session-start", "qa0001");
        env.hook("user-prompt-submit", "qa0001");
        stop_saying(
            &env,
            "qa0001",
            "Tests pass.\n\nShould I also migrate the old rows?",
        );
        assert_eq!(
            queue(&env),
            vec![(
                NeedsKind::Question,
                "qa0001 asked you a question".to_string()
            )]
        );
        let store = Store::open(&env.db_path()).unwrap();
        let asked: Vec<_> = read_all(store.conn(), None)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "owner_question")
            .collect();
        drop(store);
        assert_eq!(asked.len(), 1);
        // The event carries no message text.
        assert_eq!(asked[0].data, json!({"unit_no": 1, "source": "turn_end"}));
        let status = String::from_utf8(env.run(&["status"], "").stdout).unwrap();
        assert!(
            status.contains("needs you") && status.contains("asked you a question"),
            "{status}"
        );
        // The owner answers: the next prompt closes it.
        env.hook("user-prompt-submit", "qa0001");
        assert_eq!(queue(&env), vec![]);
        // A turn ending with a statement closes an earlier turn-end question too.
        stop_saying(&env, "qa0001", "Shall I go on?");
        assert_eq!(queue(&env).len(), 1);
        stop_saying(&env, "qa0001", "Went ahead with the migration.");
        assert_eq!(queue(&env), vec![]);
        assert_eq!(env.log(), "");
    }

    #[test]
    fn the_ask_tool_needs_the_owner_until_it_returns() {
        let env = Env::new();
        env.hook("session-start", "qb0001");
        env.hook("user-prompt-submit", "qb0001");
        ask_tool(&env, "qb0001", "pre-tool-use");
        assert_eq!(queue(&env)[0].0, NeedsKind::Question);
        // A Stop without a question doesn't close the ask tool's question.
        stop_saying(&env, "qb0001", "Waiting for the answer.");
        assert_eq!(queue(&env)[0].0, NeedsKind::Question);
        ask_tool(&env, "qb0001", "post-tool-use");
        assert_eq!(queue(&env), vec![]);
        let store = Store::open(&env.db_path()).unwrap();
        let text = serde_json::to_string(&read_all(store.conn(), None).unwrap()).unwrap();
        assert!(!text.contains("Which database"), "question text stored");
        assert_eq!(env.log(), "");
    }

    #[test]
    fn a_question_inside_code_is_no_question() {
        let env = Env::new();
        env.hook("session-start", "qc0001");
        env.hook("user-prompt-submit", "qc0001");
        stop_saying(&env, "qc0001", "Here:\n```\nwhy?\n```");
        assert_eq!(queue(&env), vec![]);
    }
}

/// About `bytes` of new transcript: model replies (each written twice, as Claude Code
/// does) between user lines, with ids unique to `batch`.
fn transcript_slice(batch: usize, bytes: usize) -> String {
    let ts = jiff::Timestamp::now().to_string();
    let mut out = String::with_capacity(bytes + 2048);
    let mut n = 0;
    while out.len() < bytes {
        let id = format!("msg_{batch}_{n}");
        let line = format!(
            r#"{{"parentUuid":"p","isSidechain":false,"type":"assistant","requestId":"req_{id}","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-opus-5-5","role":"assistant","content":[{{"type":"text","text":"{pad}"}}],"usage":{{"input_tokens":3,"cache_creation_input_tokens":300,"cache_read_input_tokens":90000,"output_tokens":120,"cache_creation":{{"ephemeral_1h_input_tokens":300,"ephemeral_5m_input_tokens":0}}}}}}}}"#,
            pad = "x".repeat(400)
        );
        out.push_str(&line);
        out.push('\n');
        out.push_str(&line);
        out.push('\n');
        out.push_str(r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#);
        out.push('\n');
        n += 1;
    }
    out
}

/// Stop and SubagentStop each with 512 KiB of unread transcript (spec §9.2, §11.1): a
/// backlog is read in bounded slices, each within the hook budget, on the large database.
/// The subagent's transcript gets twice what its SubagentStop reads, so every Stop also
/// finishes 512 KiB of it: the worst case, two full slices in one Stop.
#[test]
#[ignore = "perf check: run on demand with --release"]
fn hook_latency_stop_and_subagent_stop_with_a_transcript_backlog() {
    use bridge_screen_core::store::{EventDraft, append_in};
    use bridge_screen_core::telemetry::MAX_HOOK_BYTES;
    use std::io::Write as _;
    use std::time::Instant;

    let env = Env::new();
    let repo = big_database(&env);
    let session = "perf-session-07";
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join(format!("{session}.jsonl"));
    let sub = dir.path().join("agent-a1.jsonl");
    std::fs::write(&main, "").unwrap();
    std::fs::write(&sub, "").unwrap();
    let mut store = Store::open(&env.db_path()).unwrap();
    let append = |path: &std::path::Path, text: &str| {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    };
    let slice = usize::try_from(MAX_HOOK_BYTES).unwrap();
    assert!(env.run(&["--version"], "").status.success());
    let mut stop_payload = common::fixture_json("stop", session, &env.repo_path());
    stop_payload["transcript_path"] = json!(main.to_string_lossy());
    let stop_payload = stop_payload.to_string();
    let mut sub_payload = common::fixture_json("subagent-stop", session, &env.repo_path());
    sub_payload["transcript_path"] = json!(main.to_string_lossy());
    sub_payload["agent_id"] = json!("a1");
    sub_payload["agent_transcript_path"] = json!(sub.to_string_lossy());
    let sub_payload = sub_payload.to_string();
    let (mut stops, mut subs) = (Vec::new(), Vec::new());
    for i in 0..100 {
        // A new unit, and 512 KiB more of each transcript to read.
        store
            .write(|tx| {
                append_in(
                    tx,
                    EventDraft::new(&repo.id, session, "prompt", json!({"chars": 40})),
                )?;
                Ok(())
            })
            .unwrap();
        append(&sub, &transcript_slice(2 * i + 1, 2 * slice));
        let start = Instant::now();
        env.hook_raw("subagent-stop", &sub_payload);
        subs.push(start.elapsed());
        append(&main, &transcript_slice(2 * i, slice));
        let start = Instant::now();
        env.hook_raw("stop", &stop_payload);
        stops.push(start.elapsed());
    }
    assert_eq!(env.log(), "");
    let replies: i64 = store
        .conn()
        .query_row("SELECT count(*) FROM usage_replies", [], |r| r.get(0))
        .unwrap();
    assert!(replies > 100 * 2 * 200, "{replies} replies recorded");
    report(
        "hook stop (512 KiB transcript backlog + 512 KiB of a subagent's)",
        stops,
    );
    report("hook subagent-stop (512 KiB transcript backlog)", subs);
}

#[test]
fn a_newer_data_format_is_logged_once_per_session() {
    let env = Env::new();
    env.hook("session-start", "s1");
    bridge_screen_core::store::Store::open(&env.db_path())
        .unwrap()
        .conn()
        .execute(
            "UPDATE meta SET value = '99' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    for _ in 0..3 {
        assert_eq!(env.hook("user-prompt-submit", "s1"), "");
    }
    env.hook("user-prompt-submit", "s2");
    let log = env.log();
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("updated while this session was open"))
        .collect();
    assert_eq!(lines.len(), 2, "{log}");
}

#[test]
fn stop_refreshes_branch() {
    use bridge_screen_core::session;
    use bridge_screen_core::store::Store;
    let env = Env::new();
    let repo = env.repo.path().to_path_buf();
    let turn = |session: &str, cwd: &str| {
        env.hook_raw(
            "user-prompt-submit",
            &common::fixture("user-prompt-submit", session, cwd),
        );
        env.hook_raw("stop", &common::fixture("stop", session, cwd));
    };
    let state = |session: &str| {
        let store = Store::open(&env.db_path()).unwrap();
        let branch = session::get(store.conn(), session).unwrap().unwrap().branch;
        let n: i64 = store
            .conn()
            .query_row(
                "SELECT count(*) FROM events WHERE session_id = ?1 AND kind = 'branch_changed'",
                [session],
                |r| r.get(0),
            )
            .unwrap();
        (branch, n)
    };
    // A normal repo: the branch switched after SessionStart is picked up at Stop.
    env.hook("session-start", "s1");
    common::git(&repo, &["checkout", "-q", "-b", "feat/login"]);
    turn("s1", &env.repo_path());
    assert_eq!(state("s1"), (Some("feat/login".to_string()), 1));
    // Unchanged: no new event.
    turn("s1", &env.repo_path());
    assert_eq!(state("s1"), (Some("feat/login".to_string()), 1));
    // Detached HEAD.
    common::git(&repo, &["checkout", "-q", "--detach"]);
    turn("s1", &env.repo_path());
    let (branch, n) = state("s1");
    assert!(
        branch.as_deref().unwrap().starts_with("detached at "),
        "{branch:?}"
    );
    assert_eq!(n, 2);
    // A linked worktree, where `.git` is a file.
    let parent = tempfile::tempdir().unwrap();
    let wt = parent.path().join("wt");
    common::git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-one",
            wt.to_str().unwrap(),
        ],
    );
    let wt_s = wt.to_string_lossy().into_owned();
    env.hook_raw(
        "session-start",
        &common::fixture("session-start", "s2", &wt_s),
    );
    common::git(&wt, &["checkout", "-q", "-b", "wt-two"]);
    turn("s2", &wt_s);
    assert_eq!(state("s2"), (Some("wt-two".to_string()), 1));
    // s1's turn ending with its cwd in the other worktree: s1's own worktree is read.
    turn("s1", &wt_s);
    let (branch, n) = state("s1");
    assert!(
        branch.as_deref().unwrap().starts_with("detached at "),
        "{branch:?}"
    );
    assert_eq!(n, 2);
}
