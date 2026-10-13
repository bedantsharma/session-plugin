//! `bridge-screen mcp`: JSON-RPC over stdio, the way Claude Code talks to it.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use bridge_screen_core::plan::{NodeStatus, get_node};
use bridge_screen_core::session;
use bridge_screen_core::store::Store;
use common::{BIN, Env, fixture_json};
use serde_json::{Value, json};

const PREFIX: &str = "mcp__plugin_bridge-screen_bridge-screen__";

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form","estimate_min":30},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset"}
]"#;

struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: i64,
}

impl Mcp {
    /// Start the server and complete the MCP handshake.
    fn start(env: &Env, project_dir: &str) -> Self {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .current_dir(env.repo.path())
            .env("BRIDGE_SCREEN_HOME", env.home.path())
            .env("CLAUDE_PROJECT_DIR", project_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut mcp = Self {
            stdin: child.stdin.take(),
            child,
            lines: rx,
            next_id: 1,
        };
        let init = mcp.request(
            "initialize",
            &json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        );
        assert!(
            init["result"]["capabilities"]["tools"].is_object(),
            "{init}"
        );
        mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        mcp
    }

    fn send(&mut self, msg: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(10))
                .expect("no response from bridge-screen mcp within 10 s");
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                return msg;
            }
        }
    }

    /// Call a tool; returns (is_error, text).
    fn call(&mut self, tool: &str, args: &Value) -> (bool, String) {
        let resp = self.request("tools/call", &json!({"name": tool, "arguments": args}));
        let result = &resp["result"];
        assert!(
            result.is_object(),
            "protocol error instead of a tool result: {resp}"
        );
        let text = result["content"][0]["text"].as_str().unwrap().to_string();
        (result["isError"] == true, text)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn setup() -> Env {
    let env = Env::new();
    let out = env.run(&["plan", "edit"], SEED);
    assert!(out.status.success());
    env.hook("session-start", "s1");
    env.hook("session-start", "s2");
    env
}

/// What the PreToolUse hook sees just before Claude Code calls our tool.
fn bind(env: &Env, session: &str, tool: &str, args: &Value, tool_use_id: &str) {
    let mut p = fixture_json("pre-tool-use", session, &env.repo_path());
    p["tool_name"] = json!(format!("{PREFIX}{tool}"));
    p["tool_input"] = args.clone();
    p["tool_use_id"] = json!(tool_use_id);
    assert_eq!(env.hook_raw("pre-tool-use", &p.to_string()), "");
}

fn active_task(env: &Env, session: &str) -> Option<String> {
    let store = Store::open(&env.db_path()).unwrap();
    session::get(store.conn(), session)
        .unwrap()
        .unwrap()
        .active_task
}

#[test]
fn lists_three_tools_with_schemas() {
    let env = setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let resp = mcp.request("tools/list", &json!({}));
    let tools = resp["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["claim_task", "plan_edit", "status"]);
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(tool["description"].as_str().unwrap().len() > 20, "{tool}");
    }
    let claim = tools.iter().find(|t| t["name"] == "claim_task").unwrap();
    let props = &claim["inputSchema"]["properties"];
    for field in ["id", "new_task", "mode", "confidence"] {
        assert!(props[field].is_object(), "claim_task.{field}: {claim}");
    }
    assert!(props["mode"].to_string().contains("confirmed"), "{claim}");
    let edit = tools.iter().find(|t| t["name"] == "plan_edit").unwrap();
    assert!(
        edit["inputSchema"].to_string().contains("set_estimate"),
        "plan_edit schema should describe the ops: {edit}"
    );
}

#[test]
fn claim_task_binds_to_the_calling_session() {
    let env = setup();
    let args = json!({"id": "1.1", "mode": "auto", "confidence": 0.9});
    bind(&env, "s2", "claim_task", &args, "toolu_1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (is_error, text) = mcp.call("claim_task", &args);
    assert!(!is_error, "{text}");
    assert_eq!(text, "Claimed 1.1 Login form.");
    assert_eq!(active_task(&env, "s2").as_deref(), Some("1.1"));
    assert_eq!(active_task(&env, "s1"), None);

    // Switching reports the release.
    let args = json!({"id": "1.2", "mode": "confirmed", "confidence": 1});
    bind(&env, "s2", "claim_task", &args, "toolu_2");
    let (_, text) = mcp.call("claim_task", &args);
    assert_eq!(text, "Claimed 1.2 Password reset. Released 1.1.");

    // A new task under a milestone.
    let args = json!({"new_task": {"parent": "1", "title": "Remember me", "estimate_min": 20},
                      "mode": "auto", "confidence": 0.7});
    bind(&env, "s1", "claim_task", &args, "toolu_3");
    let (is_error, text) = mcp.call("claim_task", &args);
    assert!(!is_error, "{text}");
    assert_eq!(text, "Claimed 1.3 Remember me.");
    assert_eq!(active_task(&env, "s1").as_deref(), Some("1.3"));
}

#[test]
fn identical_calls_from_two_sessions_each_bind_once() {
    let env = setup();
    let args = json!({"id": "1.1", "mode": "auto", "confidence": 0.9});
    bind(&env, "s1", "claim_task", &args, "toolu_a");
    bind(&env, "s2", "claim_task", &args, "toolu_b");
    let mut first = Mcp::start(&env, "/nonexistent");
    let mut second = Mcp::start(&env, "/nonexistent");
    assert!(!first.call("claim_task", &args).0);
    assert!(!second.call("claim_task", &args).0);
    assert_eq!(active_task(&env, "s1").as_deref(), Some("1.1"));
    assert_eq!(active_task(&env, "s2").as_deref(), Some("1.1"));
}

#[test]
fn unbound_call_without_fallback_is_a_tool_error() {
    let env = setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (is_error, text) = mcp.call("status", &json!({}));
    assert!(is_error);
    assert_eq!(
        text,
        "bridge-screen couldn't tell which session this is; is the plugin's hook enabled?"
    );
}

#[test]
fn unbound_call_falls_back_to_the_project_session() {
    let env = setup();
    let mut mcp = Mcp::start(&env, &env.repo_path());
    let (is_error, text) = mcp.call("status", &json!({}));
    assert!(!is_error, "{text}");
    assert!(text.contains("Goal: Ship"), "{text}");
}

#[test]
fn claim_task_errors_are_tool_errors() {
    let env = setup();
    let mut mcp = Mcp::start(&env, &env.repo_path());
    let (is_error, text) = mcp.call(
        "claim_task",
        &json!({"id": "G", "mode": "auto", "confidence": 1}),
    );
    assert!(is_error);
    assert_eq!(text, "G is a goal; claim a task or subtask");
    let (is_error, text) = mcp.call("claim_task", &json!({"mode": "auto", "confidence": 1}));
    assert!(is_error);
    assert_eq!(text, "give exactly one of id or new_task");
    let (is_error, text) = mcp.call(
        "claim_task",
        &json!({"id": "1.1", "mode": "maybe", "confidence": 1}),
    );
    assert!(is_error);
    assert_eq!(
        text,
        "invalid arguments: unknown variant `maybe`, expected `auto` or `confirmed`"
    );
}

#[test]
fn plan_edit_adds_nodes_and_reports_op_errors() {
    let env = setup();
    let mut mcp = Mcp::start(&env, &env.repo_path());
    let (is_error, text) = mcp.call(
        "plan_edit",
        &json!({"ops": [
            {"op": "add_node", "parent": "1", "kind": "task", "title": "Audit log"},
            {"op": "set_estimate", "id": "1.2", "estimate_min": 15}
        ], "reason": "found more work"}),
    );
    assert!(!is_error, "{text}");
    assert_eq!(text, "Applied 2 operation(s). Added: 1.3.");
    let store = Store::open(&env.db_path()).unwrap();
    let repo = session::get(store.conn(), "s1").unwrap().unwrap().repo_id;
    let added = get_node(store.conn(), &repo, "1.3").unwrap().unwrap();
    assert_eq!(added.title, "Audit log");
    assert_eq!(added.status, NodeStatus::Todo);
    drop(store);

    let (is_error, text) = mcp.call(
        "plan_edit",
        &json!({"ops": [{"op": "add_node", "parent": "1.1", "kind": "milestone", "title": "x", "intent": "y"}]}),
    );
    assert!(is_error);
    assert!(text.starts_with("op 0:"), "{text}");
    let (is_error, text) = mcp.call(
        "plan_edit",
        &json!({"ops": [
            {"op": "set_estimate", "id": "1.1", "estimate_min": 5},
            {"op": "set_estimate", "id": "1.1", "estimate": 5}
        ]}),
    );
    assert!(is_error);
    assert!(text.starts_with("op 1: unknown field `estimate`"), "{text}");
}
