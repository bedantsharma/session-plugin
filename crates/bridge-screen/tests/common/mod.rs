//! Helpers shared by the binary's integration tests: a temp home, a temp git repo,
//! and running `bridge-screen` against them. Nothing here touches the real `~/.claude`
//! or `~/.bridge-screen`.

// Each test crate uses a different subset of these helpers.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

pub const BIN: &str = env!("CARGO_BIN_EXE_bridge-screen");

/// Variables that would point git somewhere other than `-C <dir>`. A git hook (pre-push in a
/// linked worktree) exports `GIT_DIR`, which would turn a temp repo's setup into commits on
/// the real repository.
pub const GIT_SCRUBBED: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// `git -C <dir>` with the environment scrubbed of [`GIT_SCRUBBED`].
pub fn git_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for var in GIT_SCRUBBED {
        cmd.env_remove(var);
    }
    cmd
}

pub fn git(dir: &Path, args: &[&str]) {
    let ok = git_cmd(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(ok.success(), "git {args:?} failed");
}

pub struct Env {
    pub home: tempfile::TempDir,
    pub repo: tempfile::TempDir,
    /// Extra environment for every run (e.g. a PATH with a fake `git`).
    pub vars: Vec<(String, String)>,
}

impl Env {
    pub fn new() -> Self {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]);
        git(
            repo.path(),
            &["commit", "-q", "--allow-empty", "-m", "init"],
        );
        Self {
            home: tempfile::tempdir().unwrap(),
            repo,
            vars: Vec::new(),
        }
    }

    /// The owner said no to project pages here, so SessionStart briefings carry no offer
    /// (for tests that look at the exact briefing text).
    pub fn decline_pages(&self) {
        let id = bridge_screen_core::repo::identify(self.repo.path())
            .unwrap()
            .id;
        let home = bridge_screen_core::paths::Home::at(self.home.path());
        bridge_screen_core::pages::decline(&home, &id, "no thanks").unwrap();
    }

    pub fn repo_path(&self) -> String {
        self.repo.path().to_str().unwrap().to_string()
    }

    pub fn db_path(&self) -> PathBuf {
        self.home.path().join("bridge-screen.db")
    }

    /// Run the binary with `args`, feeding `stdin`, in `cwd`.
    pub fn run_in(&self, args: &[&str], stdin: &str, cwd: &Path) -> Output {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .current_dir(cwd)
            .env("BRIDGE_SCREEN_HOME", self.home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &self.vars {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    pub fn run(&self, args: &[&str], stdin: &str) -> Output {
        self.run_in(args, stdin, self.repo.path())
    }

    /// Run `bridge-screen hook <event>` with a raw payload; assert it exits 0.
    pub fn hook_raw(&self, event: &str, payload: &str) -> String {
        let out = self.run(&["hook", event], payload);
        assert_eq!(
            out.status.code(),
            Some(0),
            "hook {event} exited non-zero: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Run a hook with the fixture for `event`, for `session` in this env's repo.
    pub fn hook(&self, event: &str, session: &str) -> String {
        self.hook_raw(event, &fixture(event, session, &self.repo_path()))
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.home.path().join("hook-errors.log")).unwrap_or_default()
    }
}

pub const EVENTS: &[&str] = &[
    "session-start",
    "user-prompt-submit",
    "pre-tool-use",
    "post-tool-use",
    "stop",
    "subagent-stop",
    "pre-compact",
    "notification",
    "session-end",
];

/// A fixture payload with its placeholders filled in.
pub fn fixture(event: &str, session: &str, cwd: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hooks")
        .join(format!("{event}.json"));
    std::fs::read_to_string(path)
        .unwrap()
        .replace("SESSION", session)
        .replace("CWD", cwd)
        .replace("PROMPT", &format!("{session}-prompt"))
}

/// A fixture as JSON, so a test can change fields before sending it.
pub fn fixture_json(event: &str, session: &str, cwd: &str) -> serde_json::Value {
    serde_json::from_str(&fixture(event, session, cwd)).unwrap()
}

pub const PREFIX: &str = "mcp__plugin_bridge-screen_bridge-screen__";

/// `bridge-screen mcp` over stdio, the way Claude Code talks to it.
pub struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: i64,
}

impl Mcp {
    /// Start the server and complete the MCP handshake.
    pub fn start(env: &Env, project_dir: &str) -> Self {
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

    pub fn send(&mut self, msg: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    pub fn request(&mut self, method: &str, params: &Value) -> Value {
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
    pub fn call(&mut self, tool: &str, args: &Value) -> (bool, String) {
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

/// What the PreToolUse hook sees just before Claude Code calls our tool.
pub fn bind(env: &Env, session: &str, tool: &str, args: &Value, tool_use_id: &str) {
    let mut p = fixture_json("pre-tool-use", session, &env.repo_path());
    p["tool_name"] = json!(format!("{PREFIX}{tool}"));
    p["tool_input"] = args.clone();
    p["tool_use_id"] = json!(tool_use_id);
    assert_eq!(env.hook_raw("pre-tool-use", &p.to_string()), "");
}

/// Bind `args` to `session` through the hook, then call the tool.
pub fn call_as(
    mcp: &mut Mcp,
    env: &Env,
    session: &str,
    tool: &str,
    args: &Value,
) -> (bool, String) {
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
