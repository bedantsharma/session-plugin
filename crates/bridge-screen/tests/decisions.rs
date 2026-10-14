//! Decisions through the binary (spec §8.6): `log_decision`, the capture asks at task
//! completion, with the handoff note and after compaction, and `bridge-screen decisions`.

mod common;

use bridge_screen_core::store::{Store, read_all};
use common::{Env, Mcp, call_as, fixture_json};
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form","estimate_min":30},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset","estimate_min":10}
]"#;

const ASK: &str = "Log any decisions this task settled (log_decision): what, why, alternatives.";

fn setup() -> (Env, Mcp) {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    env.hook("session-start", "s1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (err, text) = call(
        &mut mcp,
        &env,
        "claim_task",
        &json!({"id": "1.1", "mode": "auto", "confidence": 0.9}),
    );
    assert!(!err, "{text}");
    env.hook("user-prompt-submit", "s1");
    (env, mcp)
}

fn call(mcp: &mut Mcp, env: &Env, tool: &str, args: &Value) -> (bool, String) {
    call_as(mcp, env, "s1", tool, args)
}

fn ok(mcp: &mut Mcp, env: &Env, tool: &str, args: &Value) -> String {
    let (err, text) = call(mcp, env, tool, args);
    assert!(!err, "{tool}: {text}");
    text
}

fn kinds(env: &Env, kind: &str) -> usize {
    let store = Store::open(&env.db_path()).unwrap();
    read_all(store.conn(), None)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == kind)
        .count()
}

fn context(out: &str) -> String {
    if out.trim().is_empty() {
        return String::new();
    }
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

fn compact(env: &Env) -> String {
    let mut p = fixture_json("session-start", "s1", &env.repo_path());
    p["source"] = json!("compact");
    context(&env.hook_raw("session-start", &p.to_string()))
}

#[test]
fn claim_done_asks_for_decisions_only_when_none_are_logged() {
    let (env, mut mcp) = setup();
    let text = ok(&mut mcp, &env, "claim_done", &json!({"evidence": "done"}));
    assert!(text.ends_with(ASK), "{text}");
    assert_eq!(kinds(&env, "decision_asked"), 1);
    // The ask went out with the result: the next note doesn't repeat it.
    assert!(!context(&env.hook("user-prompt-submit", "s1")).contains(ASK));

    // Task 1.2 with a decision already logged: no ask.
    ok(
        &mut mcp,
        &env,
        "claim_task",
        &json!({"id": "1.2", "mode": "auto", "confidence": 0.9}),
    );
    let logged = ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Reset links expire in 1 h", "why": "security review",
                "alternatives": ["24 h"]}),
    );
    assert_eq!(logged, "Logged D1.");
    let text = ok(&mut mcp, &env, "claim_done", &json!({"evidence": "done"}));
    assert!(!text.contains(ASK), "{text}");
    let status = ok(&mut mcp, &env, "status", &json!({}));
    assert!(
        status.contains("Decisions on 1.2: 1 (latest: D1 Reset links expire in 1 h)"),
        "{status}"
    );
}

#[test]
fn handoff_note_asks_for_decisions_when_none_are_logged() {
    let (env, mut mcp) = setup();
    let text = ok(
        &mut mcp,
        &env,
        "handoff_note",
        &json!({"in_flight": "form half done"}),
    );
    assert_eq!(
        text,
        format!("Handoff note saved for 1.1 Login form. {ASK}")
    );
    ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Validate on blur", "why": "fewer errors"}),
    );
    let text = ok(
        &mut mcp,
        &env,
        "handoff_note",
        &json!({"in_flight": "form nearly done"}),
    );
    assert_eq!(text, "Handoff note saved for 1.1 Login form.");
}

#[test]
fn compaction_rebriefs_with_decisions_and_asks_once() {
    let (env, mut mcp) = setup();
    ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Validate on blur", "why": "fewer errors"}),
    );
    ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Repo-wide: Rust 2024 edition", "why": "new lints", "task": null}),
    );
    env.hook("pre-compact", "s1");
    let text = compact(&env);
    assert!(
        text.contains(
            "Decisions so far:\n  D1 Validate on blur\n  D2 Repo-wide: Rust 2024 edition"
        ),
        "{text}"
    );
    assert_eq!(text.matches(ASK).count(), 1, "{text}");
    // Asked once: neither a second re-briefing nor the next note asks again.
    assert!(!compact(&env).contains(ASK));
    assert_eq!(kinds(&env, "decision_asked"), 1);
}

#[test]
fn the_first_note_after_compaction_carries_the_ask() {
    let (env, mut mcp) = setup();
    ok(&mut mcp, &env, "claim_done", &json!({"evidence": "done"}));
    env.hook("pre-compact", "s1");
    // The next prompt's note (the verify nudge) gets the ask as one more line.
    let note = context(&env.hook("user-prompt-submit", "s1"));
    assert!(
        note.starts_with("bridge-screen: 1.1 is claimed done"),
        "{note}"
    );
    assert!(note.ends_with(&format!("\nbridge-screen: {ASK}")), "{note}");
    assert_eq!(kinds(&env, "decision_asked"), 2);
    // Once per flag.
    for _ in 0..5 {
        assert!(!context(&env.hook("user-prompt-submit", "s1")).contains(ASK));
    }
}

#[test]
fn supersede_refusals_are_tool_errors() {
    let (env, mut mcp) = setup();
    ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Use cookies", "why": "simple"}),
    );
    let text = ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Use tokens", "why": "mobile", "supersedes": "D1"}),
    );
    assert_eq!(text, "Logged D2 (supersedes D1).");
    let (err, text) = call(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Use sessions", "why": "x", "supersedes": "D1"}),
    );
    assert!(err);
    assert_eq!(
        text,
        "D1 was already superseded by D2 — supersede D2 instead"
    );
    let text = ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "use tokens.", "why": "mobile and web"}),
    );
    assert_eq!(text, "Merged into D2 (same decision on 1.1).");
}

#[test]
fn decisions_cli_hides_superseded_ones_unless_all() {
    let (env, mut mcp) = setup();
    let out = env.run(&["decisions"], "");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("No decisions logged for "));
    ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Use cookies", "why": "simple", "alternatives": ["tokens"]}),
    );
    ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Use tokens", "why": "mobile", "supersedes": "D1"}),
    );
    ok(
        &mut mcp,
        &env,
        "log_decision",
        &json!({"what": "Rust 2024", "why": "lints", "task": null}),
    );
    let shown = |args: &[&str]| String::from_utf8(env.run(args, "").stdout).unwrap();
    assert_eq!(
        shown(&["decisions"]),
        "D2 1.1 Use tokens — mobile [supersedes D1]\nD3 Rust 2024 — lints\n"
    );
    assert_eq!(
        shown(&["decisions", "--all", "--task", "1.1"]),
        "D1 1.1 Use cookies — simple (alternatives: tokens) [superseded by D2]\nD2 1.1 Use tokens — mobile [supersedes D1]\n"
    );
}
