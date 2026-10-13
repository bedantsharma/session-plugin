//! Helpers shared by the binary's integration tests: a temp home, a temp git repo,
//! and running `bridge-screen` against them. Nothing here touches the real `~/.claude`
//! or `~/.bridge-screen`.

// Each test crate uses a different subset of these helpers.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const BIN: &str = env!("CARGO_BIN_EXE_bridge-screen");

pub fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
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
