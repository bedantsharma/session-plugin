//! The verification ladder through the binary (spec §8.5): L1 at HEAD from `claim_done`
//! and at checkpoint commits, the `verify` tool with its test re-run, owner verification,
//! staleness at SessionStart and squash re-pointing.

mod common;

use bridge_screen_core::plan::{NodeStatus, get_node};
use bridge_screen_core::session::{self, Session};
use bridge_screen_core::store::{Event, Store, read_all};
use bridge_screen_core::verify::{self, Verification};
use common::{Env, Mcp, call_as, fixture_json, git};
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form","estimate_min":30},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset","estimate_min":10}
]"#;

fn store(env: &Env) -> Store {
    Store::open(&env.db_path()).unwrap()
}

fn sess(env: &Env, id: &str) -> Session {
    session::get(store(env).conn(), id).unwrap().unwrap()
}

fn status(env: &Env, task: &str) -> NodeStatus {
    let s = store(env);
    let repo = sess(env, "s1").repo_id;
    get_node(s.conn(), &repo, task).unwrap().unwrap().status
}

fn rows(env: &Env, task: &str) -> Vec<Verification> {
    let repo = sess(env, "s1").repo_id;
    verify::verifications(store(env).conn(), &repo, task).unwrap()
}

fn events(env: &Env) -> Vec<Event> {
    read_all(store(env).conn(), None).unwrap()
}

fn of_kind(env: &Env, kind: &str) -> Vec<Value> {
    events(env)
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.data)
        .collect()
}

fn head(env: &Env) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(env.repo.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn write(env: &Env, path: &str, text: &str) {
    let p = env.repo.path().join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn commit(env: &Env, msg: &str) {
    git(env.repo.path(), &["add", "-A"]);
    git(env.repo.path(), &["commit", "-q", "-m", msg]);
}

/// A repo with the plan, session s1 started and holding 1.1, and the MCP server.
fn setup() -> (Env, Mcp) {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    env.hook("session-start", "s1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_task",
        &json!({"id": "1.1", "mode": "auto", "confidence": 0.9}),
    );
    assert!(!err, "{text}");
    env.hook("user-prompt-submit", "s1");
    (env, mcp)
}

fn hook_with(env: &Env, event: &str, session: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut payload = fixture_json(event, session, &env.repo_path());
    edit(&mut payload);
    env.hook_raw(event, &payload.to_string())
}

/// PostToolUse for an Edit of `path` (already written to disk).
fn edited(env: &Env, session: &str, path: &str) {
    let full = env.repo.path().join(path).to_str().unwrap().to_string();
    hook_with(env, "post-tool-use", session, |p| {
        p["tool_input"]["file_path"] = json!(full);
    });
}

fn bash(env: &Env, session: &str, command: &str) -> String {
    hook_with(env, "post-tool-use", session, |p| {
        p["tool_name"] = json!("Bash");
        p["tool_input"] = json!({"command": command});
    })
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

#[test]
fn claim_done_with_committed_files_reaches_l1_at_head() {
    let (env, mut mcp) = setup();
    write(&env, "src/login form.rs", "pub fn validate_login() {}\n");
    commit(&env, "login");
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "cargo test: 3 passed", "files": ["./src/login form.rs"],
                "symbols": ["validate_login"]}),
    );
    assert!(!err, "{text}");
    let short: String = head(&env).chars().take(7).collect();
    assert!(
        text.starts_with(&format!(
            "Marked 1.1 claimed done. L1: the code exists at {short}."
        )),
        "{text}"
    );
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
    let v = &rows(&env, "1.1")[0];
    assert_eq!(
        (v.level, v.sha.as_str(), v.by),
        (1, head(&env).as_str(), verify::By::Auto)
    );
    assert_eq!(v.files, vec!["src/login form.rs"]);
}

#[test]
fn claim_before_the_commit_reaches_l1_at_the_checkpoint() {
    let (env, mut mcp) = setup();
    write(&env, "src/login.rs", "pub fn validate_login() {}\n");
    edited(&env, "s1", "src/login.rs");
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "tests pass", "files": ["src/login.rs"]}),
    );
    assert!(!err, "{text}");
    assert!(
        text.contains("L1 is checked when you commit the checkpoint."),
        "{text}"
    );
    assert_eq!(status(&env, "1.1"), NodeStatus::Claimed);
    git(env.repo.path(), &["add", "-A"]);
    git(
        env.repo.path(),
        &[
            "commit",
            "-q",
            "-m",
            "wip(bridge): 1.1 Login form",
            "-m",
            "Bridge-Task: 1.1\nBridge-Next: reset flow",
        ],
    );
    assert_eq!(context(&bash(&env, "s1", "git commit -m wip")), "");
    assert_eq!(of_kind(&env, "checkpoint").len(), 1);
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
    assert_eq!(rows(&env, "1.1")[0].sha, head(&env));
    assert_eq!(env.log(), "");
}

#[test]
fn missing_files_and_symbols_keep_the_claim_and_are_named() {
    let (env, mut mcp) = setup();
    write(&env, "src/login.rs", "pub fn validate_login() {}\n");
    commit(&env, "login");
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "done", "files": ["src/login.rs", "src/gone.rs"],
                "symbols": ["validate_login", "reset_password"]}),
    );
    assert!(!err, "{text}");
    let short: String = head(&env).chars().take(7).collect();
    assert!(
        text.contains(&format!(
            "L1 check: src/gone.rs, symbol reset_password not found at {short}. Fix the evidence with claim_done again."
        )),
        "{text}"
    );
    assert_eq!(status(&env, "1.1"), NodeStatus::Claimed);
    assert_eq!(rows(&env, "1.1"), Vec::new());

    // At the next checkpoint commit the miss is reported in the commit's context.
    write(&env, "src/other.rs", "x\n");
    git(env.repo.path(), &["add", "-A"]);
    git(
        env.repo.path(),
        &[
            "commit",
            "-q",
            "-m",
            "wip(bridge): 1.1 Login form",
            "-m",
            "Bridge-Task: 1.1",
        ],
    );
    let short: String = head(&env).chars().take(7).collect();
    assert_eq!(
        context(&bash(&env, "s1", "git commit -m wip")),
        format!(
            "bridge-screen: L1 check — src/gone.rs, symbol reset_password not found at {short}. Fix the evidence with claim_done."
        )
    );
    assert_eq!(status(&env, "1.1"), NodeStatus::Claimed);
}

#[test]
fn without_files_the_units_files_are_the_evidence() {
    let (env, mut mcp) = setup();
    write(&env, "src/login.rs", "fn a() {}\n");
    edited(&env, "s1", "src/login.rs");
    write(&env, "src/scratch.rs", "x\n");
    edited(&env, "s1", "src/scratch.rs");
    // Deleted before the claim: a unit file that no longer exists isn't evidence.
    std::fs::remove_file(env.repo.path().join("src/scratch.rs")).unwrap();
    commit(&env, "login");
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "done"}),
    );
    assert!(!err, "{text}");
    assert!(text.contains("L1: the code exists at"), "{text}");
    assert_eq!(rows(&env, "1.1")[0].files, vec!["src/login.rs"]);
}

#[cfg(unix)]
#[test]
fn a_claimed_task_alone_spawns_no_git_on_tool_use() {
    use std::os::unix::fs::PermissionsExt as _;
    let (mut env, mut mcp) = setup();
    let (err, _) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "done", "files": ["src/x.rs"]}),
    );
    assert!(!err);
    assert_eq!(status(&env, "1.1"), NodeStatus::Claimed);
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
    env.hook("post-tool-use", "s1");
    bash(&env, "s1", "cargo test");
    env.hook("user-prompt-submit", "s1");
    assert_eq!(env.log(), "");
}
