//! The briefing, read-back nudge and compact re-briefing through the hooks (spec §8.4).

mod common;

use bridge_screen_core::handoff::{BriefingStatus, READBACK_NUDGE, ReadbackState};
use bridge_screen_core::session::{self, Session};
use bridge_screen_core::store::{Store, read_all};
use common::{Env, fixture_json};
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress. More words."},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form validation","estimate_min":30},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset"}
]"#;

fn env() -> Env {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    env
}

fn sess(env: &Env, id: &str) -> Session {
    let store = Store::open(&env.db_path()).unwrap();
    session::get(store.conn(), id).unwrap().unwrap()
}

fn context_of(out: &str, event: &str) -> String {
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], event);
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

fn hook_with(env: &Env, event: &str, session: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut payload = fixture_json(event, session, &env.repo_path());
    edit(&mut payload);
    env.hook_raw(event, &payload.to_string())
}

/// s1 edits src/login.rs (on disk) and ends dirty; then s2 starts. Returns s2's briefing.
fn unstable(env: &Env) -> String {
    env.hook("session-start", "s1");
    env.hook("user-prompt-submit", "s1");
    let path = env.repo.path().join("src/login.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "half\n").unwrap();
    hook_with(env, "post-tool-use", "s1", |p| {
        p["tool_input"]["file_path"] = json!(path.to_str().unwrap());
    });
    env.hook("stop", "s1");
    env.hook("session-end", "s1");
    context_of(&env.hook("session-start", "s2"), "SessionStart")
}

#[test]
fn stable_start_is_three_lines_without_read_back() {
    let env = env();
    let text = context_of(&env.hook("session-start", "s1"), "SessionStart");
    assert_eq!(
        text,
        "bridge-screen briefing — stable\nGoal: Ship — Owner sees progress.\nClaim your task with claim_task and go."
    );
    let s = sess(&env, "s1");
    assert_eq!(s.briefing_status, Some(BriefingStatus::Stable));
    assert_eq!(s.readback, ReadbackState::NotNeeded);
    // No nudge when stable.
    env.hook("user-prompt-submit", "s1");
    assert_eq!(env.hook("pre-tool-use", "s1"), "");
    assert_eq!(env.log(), "");
}

#[test]
fn leftovers_make_an_unstable_briefing_with_read_back() {
    let env = env();
    let text = unstable(&env);
    assert!(
        text.starts_with("bridge-screen briefing — unstable\n"),
        "{text}"
    );
    assert!(
        text.contains("Leftovers (1, resolve with resolve_leftover):"),
        "{text}"
    );
    assert!(text.contains(" src/login.rs (no task) → adopt"), "{text}");
    assert!(text.lines().count() <= 40);
    assert!(
        text.ends_with(
            "call handoff_readback with the goal, what's in flight, and your next action."
        )
    );
    assert_eq!(sess(&env, "s2").readback, ReadbackState::Pending);
    assert_eq!(env.log(), "");
}

#[test]
fn edit_before_read_back_is_nudged_once() {
    let env = env();
    unstable(&env);
    env.hook("user-prompt-submit", "s2");
    let out = env.hook("pre-tool-use", "s2");
    assert_eq!(context_of(&out, "PreToolUse"), READBACK_NUDGE);
    // Not blocking, and only once.
    assert_eq!(env.hook("pre-tool-use", "s2"), "");
    hook_with(&env, "pre-tool-use", "s2", |p| {
        p["tool_input"]["file_path"] = json!(format!("{}/other.rs", env.repo_path()));
    });
    let store = Store::open(&env.db_path()).unwrap();
    let nudges = read_all(store.conn(), None)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "note_sent" && e.data["kind"] == "read_back")
        .count();
    assert_eq!(nudges, 1);
    assert_eq!(env.log(), "");
}

#[test]
fn compaction_gets_a_short_rebriefing() {
    let env = env();
    env.hook("session-start", "s1");
    let out = hook_with(&env, "session-start", "s1", |p| {
        p["source"] = json!("compact");
    });
    assert_eq!(
        context_of(&out, "SessionStart"),
        "bridge-screen: re-briefing after compaction.\nActive task: none — claim one with claim_task."
    );
}
