//! Hiding through the binary: the owner's commands, and what the agent and the desktop
//! card may not do (spec 2026-11-14 §3).

mod common;

use common::{Env, Mcp};
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship it","intent":"The product ships"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Core","intent":"Core works"},
  {"op":"add_node","parent":"1","kind":"task","title":"Schema","estimate_min":30}
]"#;

fn setup() -> Env {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    env.hook("session-start", "s1");
    env
}

fn out(o: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn cli_hide_list_and_unhide() {
    let env = setup();
    assert!(out(&env.run(&["hidden"], "")).contains("Nothing is hidden"));
    let o = env.run(&["hide", "track", "1"], "");
    assert!(o.status.success(), "{}", out(&o));
    let listed = out(&env.run(&["hidden"], ""));
    assert!(listed.contains("1. track 'Ship it'"), "{listed}");
    let o = env.run(&["unhide", "1"], "");
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&env.run(&["hidden"], "")).contains("Nothing is hidden"));
    // Hiding a project and bringing it back by name.
    assert!(env.run(&["hide", "project"], "").status.success());
    let listed = out(&env.run(&["hidden"], ""));
    assert!(listed.contains("project"), "{listed}");
    // Unhiding what isn't hidden is a stale click, said in words.
    let o = env.run(&["unhide", "card:nothing"], "");
    assert!(!o.status.success());
    assert!(out(&o).contains("isn't hidden any more"), "{}", out(&o));
    assert!(env.run(&["unhide", "1"], "").status.success());
}

#[test]
fn agent_cannot_hide_and_the_card_cannot_do_destructive_things() {
    let env = setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    // No agent tool hides or removes anything.
    let resp = mcp.request("tools/list", &json!({}));
    let names: Vec<String> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    for banned in ["hide", "unhide", "remove", "close_track", "hidden"] {
        assert!(!names.iter().any(|n| n == banned), "{banned} in {names:?}");
    }
    // The card path, with its real one-time code, refuses them all.
    let shown = mcp.request(
        "tools/call",
        &json!({"name": "show_progress", "arguments": {}}),
    );
    let nonce = shown["result"]["_meta"]["bridge-screen/nonce"].clone();
    assert_eq!(nonce.as_str().map(str::len), Some(32), "{shown}");
    let actions: Vec<Value> = vec![
        json!({"kind": "hide", "repo": "r", "subject": "project:r", "label": "x", "stamp": 0}),
        json!({"kind": "unhide", "repo": "r", "subject": "project:r"}),
        json!({"kind": "remove", "repo": "r", "id": "1.1", "title": "Schema", "confirm": true}),
        json!({"kind": "close_track", "repo": "r", "track": 1}),
        json!({"kind": "reopen_track", "repo": "r", "track": 1}),
        json!({"kind": "start_track", "repo": "r", "title": "x"}),
    ];
    for action in actions {
        let (err, text) = mcp.call("owner_action", &json!({"action": action, "nonce": nonce}));
        assert!(err && text.starts_with("refused"), "{action}: {text}");
    }
    // And nothing was recorded.
    let store = bridge_screen_core::store::Store::open(&env.db_path()).unwrap();
    let kinds: Vec<String> = bridge_screen_core::store::read_all(store.conn(), None)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    for k in ["hidden", "unhidden", "node_removed", "track_closed"] {
        assert!(!kinds.contains(&k.to_string()), "{k}");
    }
}
