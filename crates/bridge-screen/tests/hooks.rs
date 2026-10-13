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
            "prompt",
            "tool_intent",
            "file_changed",
            "unit_stopped",
            "unit_labeled",
            "session_ended"
        ]
    );
    let all = events(&env);
    let prompt = &all[1].data;
    assert_eq!(prompt["chars"], 29);
    assert!(
        !all[1].data.to_string().contains("login form"),
        "prompt text must never be stored"
    );
    assert_eq!(all[2].data["files"], json!(["src/login.rs"]));
    assert_eq!(all[4].data["unit_no"], 1);
    assert_eq!(
        all[4].data["summary"],
        "Added validation to the login form and a unit test."
    );
    assert_eq!(all[5].data["label"], "on_task");
    assert_eq!(all[6].data["reason"], "prompt_input_exit");
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
    // session_started, prompt, 2 × file_changed, unit_stopped, unit_labeled
    assert_eq!(seqs, (1..=6).collect::<Vec<_>>());
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

fn repo_name(env: &Env) -> String {
    env.repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
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
        format!(
            "bridge-screen: tracking this session in {}. No plan yet — ask the owner whether to create one (bridge-screen plan edit / plan_edit tool).",
            repo_name(&env)
        )
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
        format!(
            "bridge-screen: tracking this session in {}. Goal: Ship.\nClaim your task with claim_task before working (see the bridge-screen skill).",
            repo_name(&env)
        )
    );
    claim_for(&env, "s1", "1.1");
    let resumed = hook_with(&env, "session-start", "s1", |p| {
        p["source"] = json!("resume");
    });
    assert_eq!(
        context_of(&resumed, "SessionStart"),
        format!(
            "bridge-screen: tracking this session in {}. Goal: Ship.\nActive task: 1.1 Login form validation.",
            repo_name(&env)
        )
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
fn report(label: &str, mut times: Vec<std::time::Duration>) {
    use std::time::Duration;
    assert_eq!(times.len(), 200);
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let p95 = times[189];
    eprintln!(
        "{label} over {} calls: min {:.1} ms, p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        times.len(),
        ms(times[0]),
        ms(times[99]),
        ms(p95),
        ms(times[199])
    );
    assert!(
        p95 < Duration::from_millis(50),
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
