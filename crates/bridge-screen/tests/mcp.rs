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
    assert_eq!(
        names,
        vec![
            "claim_done",
            "claim_task",
            "close_detour",
            "handoff_note",
            "handoff_readback",
            "label_unit",
            "plan_edit",
            "promote_detour",
            "resolve_leftover",
            "resume",
            "start_detour",
            "status"
        ]
    );
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

/// Bind `args` to `session` through the hook, then call the tool.
fn call_as(mcp: &mut Mcp, env: &Env, session: &str, tool: &str, args: &Value) -> (bool, String) {
    let id = format!("toolu_{}", ulid_like(tool, args));
    bind(env, session, tool, args, &id);
    mcp.call(tool, args)
}

/// A unique-enough tool_use_id per call.
fn ulid_like(tool: &str, args: &Value) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    format!(
        "{tool}_{}_{}",
        args.to_string().len(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn claimed_setup() -> Env {
    let env = setup();
    let args = json!({"id": "1.1", "mode": "auto", "confidence": 0.9});
    bind(&env, "s1", "claim_task", &args, "toolu_claim");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    assert!(!mcp.call("claim_task", &args).0);
    env.hook("user-prompt-submit", "s1");
    env
}

#[test]
fn detour_round_trip_returns_the_cue_verbatim() {
    let env = claimed_setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let cue = "rerun tests/login.rs::rejects_empty after fixing the build";
    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "start_detour",
        &json!({"reason": "build is broken", "resume_cue": cue}),
    );
    assert!(!is_error, "{text}");
    assert_eq!(
        text,
        format!(
            "Detour D1 opened: build is broken. Suspended 1.1 Login form — on return: {cue}. If you stop before returning, leave a handoff_note."
        )
    );
    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "close_detour",
        &json!({"outcome": "done"}),
    );
    assert!(!is_error, "{text}");
    assert_eq!(
        text,
        format!("Detour closed. Return to 1.1 Login form. On return: {cue}.")
    );
    let kinds: Vec<String> =
        bridge_screen_core::store::read_all(Store::open(&env.db_path()).unwrap().conn(), None)
            .unwrap()
            .into_iter()
            .filter(|e| e.session_id == "s1")
            .map(|e| e.kind)
            .collect();
    for kind in [
        "detour_opened",
        "item_suspended",
        "detour_closed",
        "item_resumed",
        "note_sent",
    ] {
        assert!(kinds.iter().any(|k| k == kind), "{kind}: {kinds:?}");
    }
    // Nothing open any more.
    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "close_detour",
        &json!({"outcome": "done"}),
    );
    assert!(is_error);
    assert_eq!(text, "no open detour to close");
}

#[test]
fn nested_detour_returns_to_the_outer_one() {
    let env = claimed_setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    call_as(
        &mut mcp,
        &env,
        "s1",
        "start_detour",
        &json!({"reason": "build is broken", "resume_cue": "rerun the login test"}),
    );
    let (_, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "start_detour",
        &json!({"reason": "lockfile conflict", "resume_cue": "rebuild after cargo update"}),
    );
    assert_eq!(
        text,
        "Detour D2 opened: lockfile conflict. Suspended D1 build is broken — on return: rebuild after cargo update. If you stop before returning, leave a handoff_note."
    );
    let (_, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "close_detour",
        &json!({"outcome": "abandoned"}),
    );
    assert_eq!(
        text,
        "Detour closed. Return to D1 build is broken. On return: rebuild after cargo update."
    );
}

#[test]
fn closing_the_outer_detour_first_still_returns_to_the_task() {
    let env = claimed_setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    for (reason, cue) in [
        ("build is broken", "rerun the login test"),
        ("lockfile conflict", "rebuild after cargo update"),
    ] {
        call_as(
            &mut mcp,
            &env,
            "s1",
            "start_detour",
            &json!({"reason": reason, "resume_cue": cue}),
        );
    }
    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "close_detour",
        &json!({"outcome": "abandoned", "detour": "D1"}),
    );
    assert!(!is_error, "{text}");
    assert_eq!(text, "Detour D1 closed. Continue with your current work.");
    // D1 is gone, so closing D2 goes straight back to the task, with the task's cue.
    let (_, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "close_detour",
        &json!({"outcome": "done"}),
    );
    assert_eq!(
        text,
        "Detour closed. Return to 1.1 Login form. On return: rerun the login test."
    );
    let store = Store::open(&env.db_path()).unwrap();
    assert_eq!(
        bridge_screen_core::tangent::suspended(store.conn(), "s1").unwrap(),
        vec![],
        "nothing is left suspended"
    );
    let s = session::get(store.conn(), "s1").unwrap().unwrap();
    assert_eq!(s.current_detour, None);
}

#[test]
fn close_with_nothing_open_is_a_tool_error() {
    let env = claimed_setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "close_detour",
        &json!({"outcome": "done"}),
    );
    assert!(is_error);
    assert_eq!(text, "no open detour to close");
    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "close_detour",
        &json!({"outcome": "finished"}),
    );
    assert!(is_error);
    assert!(
        text.starts_with("invalid arguments: unknown variant `finished`"),
        "{text}"
    );
}

#[test]
fn resume_someone_elses_item_is_a_tool_error() {
    let env = claimed_setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    call_as(
        &mut mcp,
        &env,
        "s1",
        "start_detour",
        &json!({"reason": "build is broken", "resume_cue": "rerun the login test"}),
    );
    let store = Store::open(&env.db_path()).unwrap();
    let item = bridge_screen_core::tangent::suspended(store.conn(), "s1").unwrap()[0]
        .item_id
        .clone();
    drop(store);
    let (is_error, text) = call_as(&mut mcp, &env, "s2", "resume", &json!({"item_id": item}));
    assert!(is_error);
    assert_eq!(text, format!("no suspended item {item} in this session"));
    // Its own session can, by item id or by the task id it points at.
    let (is_error, text) = call_as(&mut mcp, &env, "s1", "resume", &json!({"item_id": "1.1"}));
    assert!(!is_error, "{text}");
    assert_eq!(
        text,
        "Resumed 1.1 Login form. On return: rerun the login test."
    );
    let (is_error, _) = call_as(&mut mcp, &env, "s1", "resume", &json!({"item_id": "1.1"}));
    assert!(is_error, "an item is resumed once");
}

#[test]
fn promote_creates_and_claims_a_task() {
    let env = claimed_setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    call_as(
        &mut mcp,
        &env,
        "s1",
        "start_detour",
        &json!({"reason": "rate limiting", "resume_cue": "finish the login form submit"}),
    );
    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "promote_detour",
        &json!({"parent": "1", "title": "Rate-limit login", "estimate_min": 40}),
    );
    assert!(!is_error, "{text}");
    assert_eq!(text, "Detour D1 is now task 1.3.");
    assert_eq!(active_task(&env, "s1").as_deref(), Some("1.3"));
    let store = Store::open(&env.db_path()).unwrap();
    let repo = session::get(store.conn(), "s1").unwrap().unwrap().repo_id;
    let node = get_node(store.conn(), &repo, "1.3").unwrap().unwrap();
    assert_eq!(
        (node.title.as_str(), node.estimate_min),
        ("Rate-limit login", Some(40))
    );
    let d = bridge_screen_core::tangent::open_detours(store.conn(), "s1").unwrap();
    assert!(d.is_empty(), "a promoted detour is closed");
    let all: Vec<_> = bridge_screen_core::store::read_all(store.conn(), None).unwrap();
    let promoted = all.iter().find(|e| e.kind == "detour_promoted").unwrap();
    assert_eq!(promoted.data["task_id"], "1.3");
    let added = all
        .iter()
        .find(|e| e.kind == "node_added" && e.data["id"] == "1.3")
        .unwrap();
    assert_eq!(added.data["reason"], "promoted from detour");
    // The task it blocked is still waiting to be resumed.
    let items = bridge_screen_core::tangent::suspended(store.conn(), "s1").unwrap();
    assert_eq!(items[0].ref_id, "1.1");
}

#[test]
fn empty_or_oversized_cue_is_rejected() {
    let env = claimed_setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    for cue in ["", "   ", &"x".repeat(201)] {
        let (is_error, text) = call_as(
            &mut mcp,
            &env,
            "s1",
            "start_detour",
            &json!({"reason": "r", "resume_cue": cue}),
        );
        assert!(is_error, "{cue}");
        assert_eq!(
            text,
            "resume_cue is required: one line (at most 200 characters) naming the next action and the file or test"
        );
    }
    let store = Store::open(&env.db_path()).unwrap();
    assert_eq!(
        bridge_screen_core::tangent::open_detours(store.conn(), "s1").unwrap(),
        vec![]
    );
}

#[test]
fn disagreement_is_recorded_not_argued() {
    use bridge_screen_core::tangent::{Label, LabelSource};
    let env = Env::new();
    let seed = r#"[
      {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
      {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
      {"op":"add_node","parent":"1","kind":"task","title":"Login form validation",
       "expected_files":["src/login.rs"]}
    ]"#;
    assert!(env.run(&["plan", "edit"], seed).status.success());
    env.hook("session-start", "s1");
    let args = json!({"id": "1.1", "mode": "auto", "confidence": 0.9});
    let mut mcp = Mcp::start(&env, "/nonexistent");
    call_as(&mut mcp, &env, "s1", "claim_task", &args);
    env.hook("user-prompt-submit", "s1");
    for f in ["docker/Dockerfile", ".github/workflows/ci.yml"] {
        let mut p = fixture_json("post-tool-use", "s1", &env.repo_path());
        p["tool_input"]["file_path"] = json!(format!("{}/{f}", env.repo_path()));
        env.hook_raw("post-tool-use", &p.to_string());
    }
    let mut p = fixture_json("stop", "s1", &env.repo_path());
    p["last_assistant_message"] = json!("Rewrote the Dockerfile and the CI workflow caching.");
    let out = env.hook_raw("stop", &p.to_string());
    assert!(out.contains("possible drift"), "{out}");

    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "label_unit",
        &json!({"label": "on_task", "why": "CI must pass before the login form can merge"}),
    );
    assert!(!is_error, "{text}");
    assert_eq!(text, "Labelled unit 1 as on-task.");
    let store = Store::open(&env.db_path()).unwrap();
    let u = &session::units(store.conn(), "s1").unwrap()[0];
    assert_eq!(u.label, Some(Label::OnTask));
    assert_eq!(u.label_source, Some(LabelSource::Agent));
    assert!(u.disagreement && u.flagged);
    let (n, label, source): (i64, String, String) = store
        .conn()
        .query_row(
            "SELECT count(*), max(label), max(source) FROM drift_samples",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (n, label.as_str(), source.as_str()),
        (1, "on_task", "agent")
    );
    drop(store);
    // The agent's word stands: no red, no needs-you, no red note on the next tool call.
    let p = fixture_json("post-tool-use", "s1", &env.repo_path());
    assert_eq!(env.hook_raw("post-tool-use", &p.to_string()), "");
    let store = Store::open(&env.db_path()).unwrap();
    let s = session::get(store.conn(), "s1").unwrap().unwrap();
    assert_eq!(s.nav_state.as_str(), "green");
    let drift_asks: i64 = store
        .conn()
        .query_row(
            "SELECT count(*) FROM events WHERE kind IN ('needs_owner', 'nav_state_changed')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(drift_asks, 0);
    drop(store);

    // All units labelled: a second call corrects the last one.
    let (_, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "label_unit",
        &json!({"label": "detour", "why": "it blocked the merge"}),
    );
    assert_eq!(text, "Labelled unit 1 as detour.");
    let (is_error, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "label_unit",
        &json!({"label": "sideways", "why": "x"}),
    );
    assert!(is_error);
    assert!(
        text.starts_with("invalid arguments: unknown variant `sideways`"),
        "{text}"
    );
}

#[test]
fn status_lists_detours_and_suspended_items() {
    let env = claimed_setup();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    call_as(
        &mut mcp,
        &env,
        "s1",
        "start_detour",
        &json!({"reason": "build is broken", "resume_cue": "rerun the login test"}),
    );
    let (is_error, text) = call_as(&mut mcp, &env, "s1", "status", &json!({}));
    assert!(!is_error, "{text}");
    assert!(
        text.contains("Open detours: D1 build is broken (current)"),
        "{text}"
    );
    assert!(
        text.contains("Suspended: 1.1 Login form — on return: rerun the login test"),
        "{text}"
    );
}

#[test]
fn claim_done_marks_the_task_and_asks_for_a_checkpoint() {
    let env = setup();
    let args = json!({"id": "1.1", "mode": "auto", "confidence": 0.9});
    bind(&env, "s1", "claim_task", &args, "toolu_1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    assert!(!mcp.call("claim_task", &args).0);
    let args = json!({"evidence": "cargo test: 12 passed"});
    bind(&env, "s1", "claim_done", &args, "toolu_2");
    let (is_error, text) = mcp.call("claim_done", &args);
    assert!(!is_error, "{text}");
    assert_eq!(
        text,
        "Marked 1.1 claimed done. Commit a checkpoint before you stop, then leave a handoff_note if work remains in flight."
    );
    let store = Store::open(&env.db_path()).unwrap();
    let node = get_node(
        store.conn(),
        &session::get(store.conn(), "s1").unwrap().unwrap().repo_id,
        "1.1",
    )
    .unwrap()
    .unwrap();
    assert_eq!(node.status, NodeStatus::Claimed);
    let s = session::get(store.conn(), "s1").unwrap().unwrap();
    assert_eq!(s.checkpoint_pending.as_deref(), Some("claimed_done"));

    // Without an active task and without an id there is nothing to mark.
    let args = json!({"evidence": "x"});
    bind(&env, "s2", "claim_done", &args, "toolu_3");
    let (is_error, text) = mcp.call("claim_done", &args);
    assert!(is_error);
    assert_eq!(text, "no active task; give the id of the task you finished");
}

#[test]
fn resolve_leftover_undoes_into_a_stash() {
    let env = setup();
    // s1 leaves an untracked file behind and ends; s3 starts in the same worktree.
    env.hook("user-prompt-submit", "s1");
    let path = env.repo.path().join("scratch.txt");
    std::fs::write(&path, "half a thought\n").unwrap();
    let mut p = fixture_json("post-tool-use", "s1", &env.repo_path());
    p["tool_input"]["file_path"] = json!(path.to_str().unwrap());
    env.hook_raw("post-tool-use", &p.to_string());
    env.hook("session-end", "s1");
    env.hook("session-end", "s2");
    env.hook("session-start", "s3");
    let store = Store::open(&env.db_path()).unwrap();
    let repo_id = session::get(store.conn(), "s3").unwrap().unwrap().repo_id;
    let found = bridge_screen_core::handoff::leftovers(store.conn(), &repo_id, true).unwrap();
    assert_eq!(found.len(), 1);
    let id = found[0].leftover_id.clone();

    let args = json!({"ids": [id], "action": "undo"});
    bind(&env, "s3", "resolve_leftover", &args, "toolu_1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (is_error, text) = mcp.call("resolve_leftover", &args);
    assert!(!is_error, "{text}");
    assert!(text.starts_with("Undid 1 leftover: stashed as "), "{text}");
    assert!(text.contains("git stash apply"), "{text}");
    assert!(!path.exists());
    let snap = env
        .home
        .path()
        .join("snapshots")
        .join(&repo_id)
        .join(format!("{id}.json"));
    assert!(snap.exists(), "{}", snap.display());

    // A bad action is a tool error the agent can read.
    let args = json!({"ids": [id], "action": "delete"});
    bind(&env, "s3", "resolve_leftover", &args, "toolu_2");
    let (is_error, text) = mcp.call("resolve_leftover", &args);
    assert!(is_error);
    assert!(text.starts_with("invalid arguments"), "{text}");
}

#[test]
fn handoff_note_and_readback_round_trip() {
    let env = setup();
    let args = json!({"id": "1.1", "mode": "auto", "confidence": 0.9});
    bind(&env, "s1", "claim_task", &args, "toolu_1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    assert!(!mcp.call("claim_task", &args).0);
    let args = json!({"in_flight": "validation half wired",
                      "if_then": ["if the form test flakes, rerun it once"]});
    bind(&env, "s1", "handoff_note", &args, "toolu_2");
    let (is_error, text) = mcp.call("handoff_note", &args);
    assert!(!is_error, "{text}");
    assert_eq!(text, "Handoff note saved for 1.1 Login form.");
    let args = json!({"in_flight": ""});
    bind(&env, "s1", "handoff_note", &args, "toolu_3");
    let (is_error, text) = mcp.call("handoff_note", &args);
    assert!(is_error);
    assert!(text.starts_with("in_flight is required"), "{text}");

    let args = json!({"goal": "ship", "in_flight": "1.1 login form", "next_action": "wire it"});
    bind(&env, "s1", "handoff_readback", &args, "toolu_4");
    let (is_error, text) = mcp.call("handoff_readback", &args);
    assert!(!is_error, "{text}");
    assert_eq!(text, "Read-back matches the log. Go ahead.");
    let store = Store::open(&env.db_path()).unwrap();
    let s = session::get(store.conn(), "s1").unwrap().unwrap();
    assert_eq!(s.readback, bridge_screen_core::handoff::ReadbackState::Ok);
    let args = json!({"goal": "ship", "in_flight": "the reset page", "next_action": "x"});
    bind(&env, "s1", "handoff_readback", &args, "toolu_5");
    let (_, text) = mcp.call("handoff_readback", &args);
    assert_eq!(
        text,
        "Close, but the log says: active task 1.1 \"Login form\". Carry on from there."
    );
}

#[test]
fn status_shows_checkpoint_read_back_and_leftovers() {
    let env = setup();
    // s1 leaves a file behind and ends; s2 also ends; s3 starts and gets the leftover.
    env.hook("user-prompt-submit", "s1");
    let path = env.repo.path().join("left.txt");
    std::fs::write(&path, "x\n").unwrap();
    let mut p = fixture_json("post-tool-use", "s1", &env.repo_path());
    p["tool_input"]["file_path"] = json!(path.to_str().unwrap());
    env.hook_raw("post-tool-use", &p.to_string());
    env.hook("session-end", "s1");
    env.hook("session-end", "s2");
    env.hook("session-start", "s3");
    let args = json!({"id": "1.1", "mode": "auto", "confidence": 0.9});
    bind(&env, "s3", "claim_task", &args, "toolu_1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    assert!(!mcp.call("claim_task", &args).0);
    let args = json!({"evidence": "done"});
    bind(&env, "s3", "claim_done", &args, "toolu_2");
    assert!(!mcp.call("claim_done", &args).0);
    bind(&env, "s3", "status", &json!({}), "toolu_3");
    let (is_error, text) = mcp.call("status", &json!({}));
    assert!(!is_error, "{text}");
    assert!(
        text.contains("Checkpoint pending (claimed_done): commit as `wip(bridge): <id> <title>` before you stop."),
        "{text}"
    );
    assert!(
        text.contains("Read-back pending: call handoff_readback."),
        "{text}"
    );
    assert!(text.contains("Leftovers (1): "), "{text}");
    assert!(text.contains(" left.txt (no task) → adopt"), "{text}");
}
