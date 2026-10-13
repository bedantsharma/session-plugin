//! `bridge-screen hook <event>`: the binary run the way Claude Code runs it.

mod common;

use common::{EVENTS, Env};

#[test]
fn each_event_is_accepted() {
    let env = Env::new();
    for event in EVENTS {
        let out = env.hook(event, "s-accept");
        assert_eq!(out, "", "{event} printed something");
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
        assert_eq!(env.hook(event, "s1"), "");
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
    assert_eq!(all[5].data["reason"], "prompt_input_exit");
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
    assert_eq!(seqs, (1..=5).collect::<Vec<_>>());
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
            "unit_stopped"
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
    for kind in ["permission_prompt", "idle_prompt", "auth_success"] {
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
