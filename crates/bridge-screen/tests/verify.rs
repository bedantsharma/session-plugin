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

/// `setup`, with `src/login.rs` committed and 1.1 claimed done at L1.
fn at_l1() -> (Env, Mcp) {
    let (env, mut mcp) = setup();
    write(&env, "src/login.rs", "pub fn validate_login() {}\n");
    edited(&env, "s1", "src/login.rs");
    commit(&env, "login");
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "tests pass", "files": ["src/login.rs"]}),
    );
    assert!(!err, "{text}");
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
    (env, mcp)
}

fn verify_call(env: &Env, mcp: &mut Mcp, args: &Value) -> (bool, String) {
    call_as(mcp, env, "s1", "verify", args)
}

fn l2() -> Value {
    json!({"level": 2, "evidence": "cargo test", "exit_code": 0, "summary": "3 passed"})
}

fn set_test_command(env: &Env, cmd: &str) {
    let out = env.run(&["config", "set", "test-command", cmd], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn verify_refuses_uncommitted_work() {
    let (env, mut mcp) = at_l1();
    write(
        &env,
        "src/login.rs",
        "pub fn validate_login() { todo!() }\n",
    );
    let before = events(&env).len();
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(err);
    assert_eq!(
        text,
        "commit a checkpoint first; verifications are tied to a commit (uncommitted: src/login.rs)"
    );
    // Only the hook's binding of the call: nothing recorded.
    assert_eq!(events(&env).len(), before);
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
    commit(&env, "finish login");
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(!err, "{text}");
    let top = rows(&env, "1.1").pop().unwrap();
    assert_eq!((top.level, top.sha.clone()), (2, head(&env)));
    assert_eq!(top.files, vec!["src/login.rs"]);
    assert_eq!(status(&env, "1.1"), NodeStatus::L2);
}

#[test]
fn l2_without_a_test_command_is_recorded_as_reported() {
    let (env, mut mcp) = at_l1();
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(!err, "{text}");
    assert_eq!(
        text,
        "1.1 verified L2 (as reported; no test command to re-run). Ask the owner to set a test command: bridge-screen config set test-command '<cmd>'. Project 75% done."
    );
    let top = rows(&env, "1.1").pop().unwrap();
    assert_eq!(
        (top.by, top.rerun),
        (verify::By::Agent, Some(verify::Rerun::NotConfigured))
    );
    assert_eq!(top.evidence, "cargo test · 3 passed");
}

#[test]
fn l2_reruns_the_owners_test_command() {
    let (env, mut mcp) = at_l1();
    set_test_command(&env, "test -f src/login.rs && echo '3 passed'");
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(!err, "{text}");
    assert!(
        text.starts_with("1.1 verified L2 (re-run passed in "),
        "{text}"
    );
    assert!(text.ends_with("Project 75% done."), "{text}");
    let top = rows(&env, "1.1").pop().unwrap();
    assert_eq!(
        (top.by, top.rerun),
        (verify::By::Rerun, Some(verify::Rerun::Passed))
    );
    assert!(top.evidence.contains("re-run ok in"), "{}", top.evidence);
}

#[test]
fn a_failed_rerun_sets_failed_until_the_task_is_claimed_again() {
    let (env, mut mcp) = at_l1();
    set_test_command(&env, "echo 'test login ... FAILED'; exit 1");
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(!err, "{text}");
    assert_eq!(
        text,
        "The re-run failed (exit 1): test login ... FAILED. 1.1 is back to failed; fix it and verify again."
    );
    assert_eq!(status(&env, "1.1"), NodeStatus::Failed);
    let failed = of_kind(&env, "verification_failed");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["rerun"], "failed");
    assert_eq!(failed[0]["evidence"], "test login ... FAILED");
    let (err, _) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_task",
        &json!({"id": "1.1", "mode": "auto", "confidence": 0.9}),
    );
    assert!(!err);
    assert_eq!(status(&env, "1.1"), NodeStatus::Active);
    // Not claimed done any more: verify waits for claim_done.
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(err);
    assert_eq!(
        text,
        "1.1 isn't claimed done (active); call claim_done first"
    );
}

#[test]
fn a_failing_exit_code_is_refused() {
    let (env, mut mcp) = at_l1();
    let (err, text) = verify_call(
        &env,
        &mut mcp,
        &json!({"level": 2, "evidence": "cargo test", "exit_code": 101}),
    );
    assert!(err);
    assert_eq!(
        text,
        "Exit code 101 means the tests failed — fix them, commit, and verify again."
    );
    let (err, text) = verify_call(
        &env,
        &mut mcp,
        &json!({"level": 2, "evidence": "cargo test"}),
    );
    assert!(err);
    assert!(text.starts_with("exit_code is required"), "{text}");
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
}

#[test]
fn l3_needs_l1_and_l4_needs_the_owners_words() {
    let (env, mut mcp) = setup();
    let (err, _) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "done"}),
    );
    assert!(!err);
    let l3 = json!({"level": 3, "evidence": "logged in through the browser"});
    let (err, text) = verify_call(&env, &mut mcp, &l3);
    assert!(err);
    assert!(text.starts_with("1.1 has no L1 yet"), "{text}");
    let (err, text) = verify_call(
        &env,
        &mut mcp,
        &json!({"level": 4, "evidence": "asked the owner"}),
    );
    assert!(err);
    assert!(text.starts_with("owner_said is required"), "{text}");
    let (err, text) = verify_call(
        &env,
        &mut mcp,
        &json!({"level": 4, "evidence": "asked", "owner_said": "yes, login works for me"}),
    );
    assert!(!err, "{text}");
    assert_eq!(status(&env, "1.1"), NodeStatus::L4);
    let top = rows(&env, "1.1").pop().unwrap();
    assert_eq!(top.by, verify::By::OwnerViaAgent);
    assert!(top.evidence.contains("yes, login works for me"));

    let (env, mut mcp) = at_l1();
    let (err, text) = verify_call(&env, &mut mcp, &l3);
    assert!(!err, "{text}");
    assert!(text.starts_with("1.1 verified L3"), "{text}");
    assert_eq!(status(&env, "1.1"), NodeStatus::L3);
}

#[test]
fn no_tool_can_set_the_test_command() {
    let env = Env::new();
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let resp = mcp.request("tools/list", &json!({}));
    let tools = resp["result"]["tools"].to_string();
    assert!(tools.contains("\"verify\""), "{tools}");
    for word in ["test_command", "test-command", "config_changed"] {
        assert!(!tools.contains(word), "{word} in {tools}");
    }
}

#[test]
fn owner_verifies_or_marks_broken_from_the_cli() {
    let (env, _mcp) = at_l1();
    let out = env.run(&["verify", "1.1"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let short: String = head(&env).chars().take(7).collect();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("Verified 1.1 Login form L4 at {short}.")
    );
    assert_eq!(status(&env, "1.1"), NodeStatus::L4);
    let top = rows(&env, "1.1").pop().unwrap();
    assert_eq!(
        (top.by, top.session_id.as_str()),
        (verify::By::Owner, "owner")
    );
    let out = env.run(&["verify", "1.1", "--broken", "the reset link 404s"], "");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "Marked 1.1 Login form broken; it's back to failed."
    );
    assert_eq!(status(&env, "1.1"), NodeStatus::Failed);
    let actions = of_kind(&env, "owner_action");
    assert_eq!(
        actions,
        vec![
            json!({"action": "verify", "answer": "works", "target": "1.1"}),
            json!({"action": "verify", "answer": "broken", "target": "1.1"}),
        ]
    );
    for bad in [["verify", "1"], ["verify", "9.9"]] {
        let out = env.run(&bad, "");
        assert!(!out.status.success(), "{bad:?}");
    }
}

#[test]
fn test_settings_round_trip_through_the_cli() {
    let env = Env::new();
    set_test_command(&env, "cargo test -q");
    let out = env.run(&["config", "set", "test-timeout", "300"], "");
    assert!(out.status.success());
    let shown = String::from_utf8_lossy(&env.run(&["config"], "").stdout).into_owned();
    assert!(shown.contains("verify_timeout_s = 120"), "{shown}");
    assert!(shown.contains(": cargo test -q"), "{shown}");
    assert!(shown.contains(": 300 s"), "{shown}");
    let changes = of_kind_any(&env, "config_changed");
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["new"], "cargo test -q");
    assert_eq!(changes[1]["new"], 300);
    let bad = env.run(&["config", "set", "test-timeout", "soon"], "");
    assert!(!bad.status.success());
    let out = env.run(&["config", "unset", "test-command"], "");
    assert!(out.status.success());
    let shown = String::from_utf8_lossy(&env.run(&["config"], "").stdout).into_owned();
    assert!(!shown.contains("cargo test -q"), "{shown}");
    assert!(shown.contains(": 300 s"), "{shown}");
    let changes = of_kind_any(&env, "config_changed");
    assert_eq!(changes[2]["new"], Value::Null);
    assert_eq!(changes[2]["old"], "cargo test -q");
}

/// Events of a kind, from a store that may have no sessions.
fn of_kind_any(env: &Env, kind: &str) -> Vec<Value> {
    events(env)
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.data)
        .collect()
}

/// Start a new session (a SessionStart in the repo); its briefing.
fn start(env: &Env, session: &str) -> String {
    context(&env.hook("session-start", session))
}

fn stale_events(env: &Env) -> Vec<Value> {
    of_kind(env, "verification_stale")
}

fn at_l2() -> (Env, Mcp) {
    let (env, mut mcp) = at_l1();
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(!err, "{text}");
    assert_eq!(status(&env, "1.1"), NodeStatus::L2);
    (env, mcp)
}

#[test]
fn stale_drops_once_per_change() {
    let (env, _mcp) = at_l2();
    let l2_id = rows(&env, "1.1").pop().unwrap().verif_id;
    // No new commits: nothing changes, however often a session starts.
    start(&env, "s2");
    start(&env, "s3");
    assert_eq!(stale_events(&env), Vec::<Value>::new());
    // A commit that touches only an unrelated file drops nothing.
    write(&env, "src/other.rs", "x\n");
    commit(&env, "other");
    start(&env, "s4");
    assert_eq!(stale_events(&env), Vec::<Value>::new());
    assert_eq!(status(&env, "1.1"), NodeStatus::L2);
    // A commit that touches a depended-on file drops each verification one level.
    write(
        &env,
        "src/login.rs",
        "pub fn validate_login() { /* v2 */ }\n",
    );
    commit(&env, "login v2");
    let briefing = start(&env, "s5");
    let drops: Vec<Value> = stale_events(&env)
        .into_iter()
        .filter(|e| e["verif_id"] == l2_id)
        .collect();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        (&drops[0]["from_level"], &drops[0]["to_level"]),
        (&json!(2), &json!(1))
    );
    assert_eq!(drops[0]["changed"], json!(["src/login.rs"]));
    assert_eq!(drops[0]["head_sha"], head(&env));
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
    // The L1 row dropped to 0; the dropped L2 row still holds L1, so no re-check.
    let levels: Vec<(i64, verify::VerifState)> = rows(&env, "1.1")
        .iter()
        .map(|v| (v.level, v.state))
        .collect();
    assert_eq!(
        levels,
        vec![
            (0, verify::VerifState::Stale),
            (1, verify::VerifState::Stale)
        ]
    );
    assert!(
        briefing.contains("1.1 Login form (stale: src/login.rs changed @"),
        "{briefing}"
    );
    // Starting again at the same HEAD drops nothing more.
    let before = stale_events(&env).len();
    start(&env, "s6");
    start(&env, "s7");
    assert_eq!(stale_events(&env).len(), before);
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
    assert_eq!(env.log(), "");
}

#[test]
fn verifications_on_another_branch_are_left_alone() {
    let (env, _mcp) = setup();
    git(env.repo.path(), &["checkout", "-q", "-b", "other"]);
    write(&env, "x.rs", "one\n");
    commit(&env, "x on other");
    let other = head(&env);
    git(env.repo.path(), &["checkout", "-q", "main"]);
    {
        let mut s = store(&env);
        let repo = sess(&env, "s1").repo_id;
        s.append(
            verify::verified_draft(
                &repo,
                "s1",
                "1.2",
                2,
                &other,
                vec!["x.rs".into()],
                "ok",
                verify::By::Agent,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    }
    write(&env, "x.rs", "two\n");
    commit(&env, "x on main");
    start(&env, "s2");
    assert_eq!(stale_events(&env), Vec::<Value>::new());
    assert_eq!(status(&env, "1.2"), NodeStatus::L2);
}

#[test]
fn a_deleted_file_leaves_the_task_claimed_and_listed() {
    let (env, _mcp) = at_l1();
    git(env.repo.path(), &["rm", "-q", "src/login.rs"]);
    commit(&env, "drop login");
    let briefing = start(&env, "s2");
    assert_eq!(status(&env, "1.1"), NodeStatus::Claimed);
    assert!(
        briefing.contains("1.1 Login form (stale: src/login.rs changed @"),
        "{briefing}"
    );
    assert_eq!(env.log(), "");
}

#[test]
fn squash_keeps_verifications() {
    let (env, mut mcp) = setup();
    git(env.repo.path(), &["checkout", "-q", "-b", "feat"]);
    write(&env, "src/login.rs", "pub fn validate_login() {}\n");
    edited(&env, "s1", "src/login.rs");
    commit(&env, "wip(bridge): 1.1 Login form");
    write(&env, "README.md", "notes\n");
    commit(&env, "docs: notes");
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "tests pass", "files": ["src/login.rs"]}),
    );
    assert!(!err, "{text}");
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(!err, "{text}");
    let wip_head = head(&env);
    let out = env.run(&["squash", "--onto", "main"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let new_sha = head(&env);
    assert_ne!(new_sha, wip_head);
    // Every commit of the range is in old_shas, not only the WIP one.
    let squashed = of_kind(&env, "squashed");
    assert_eq!(squashed[0]["old_shas"].as_array().unwrap().len(), 2);
    start(&env, "s2");
    assert_eq!(stale_events(&env), Vec::<Value>::new());
    assert_eq!(status(&env, "1.1"), NodeStatus::L2);
    for v in rows(&env, "1.1") {
        assert_eq!(
            (v.sha.as_str(), v.stale_checked_sha.as_str()),
            (new_sha.as_str(), new_sha.as_str())
        );
    }
    // The re-pointed verification is still watched: a later change drops it.
    write(
        &env,
        "src/login.rs",
        "pub fn validate_login() { /* v2 */ }\n",
    );
    commit(&env, "login v2");
    start(&env, "s3");
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
    assert_eq!(env.log(), "");
}

/// Verified at the first WIP commit, changed again by the second, then squashed by
/// `squash` (CLI) or before `gh pr create` (hook): the change still drops it.
fn change_inside_the_squashed_range(squash: impl FnOnce(&Env)) {
    let (env, mut mcp) = setup();
    git(env.repo.path(), &["checkout", "-q", "-b", "feat"]);
    write(&env, "src/login.rs", "pub fn validate_login() {}\n");
    edited(&env, "s1", "src/login.rs");
    commit(&env, "wip(bridge): 1.1 Login form");
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "claim_done",
        &json!({"evidence": "tests pass", "files": ["src/login.rs"]}),
    );
    assert!(!err, "{text}");
    let (err, text) = verify_call(&env, &mut mcp, &l2());
    assert!(!err, "{text}");
    write(
        &env,
        "src/login.rs",
        "pub fn validate_login() { /* v2 */ }\n",
    );
    commit(&env, "wip(bridge): 1.1 Login form, more");
    squash(&env);
    assert_eq!(of_kind(&env, "squashed").len(), 1);
    start(&env, "s2");
    assert_eq!(
        status(&env, "1.1"),
        NodeStatus::L1,
        "{:?}",
        rows(&env, "1.1")
    );
    assert_ne!(stale_events(&env), Vec::<Value>::new());
    assert_eq!(env.log(), "");
}

#[test]
fn a_change_inside_the_squashed_range_still_drops_the_verification() {
    change_inside_the_squashed_range(|env| {
        let out = env.run(&["squash", "--onto", "main"], "");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
    change_inside_the_squashed_range(|env| {
        hook_with(env, "pre-tool-use", "s1", |p| {
            p["tool_name"] = json!("Bash");
            p["tool_input"] = json!({"command": "gh pr create --fill --base main"});
        });
    });
}

#[test]
fn an_l1_that_drops_to_nothing_is_rechecked_at_head() {
    let (env, _mcp) = at_l1();
    write(
        &env,
        "src/login.rs",
        "pub fn validate_login() { /* v2 */ }\n",
    );
    commit(&env, "login v2");
    start(&env, "s2");
    assert_eq!(status(&env, "1.1"), NodeStatus::L1);
    let rechecked = rows(&env, "1.1").pop().unwrap();
    assert_eq!(
        (rechecked.level, rechecked.by, rechecked.sha.clone()),
        (1, verify::By::Auto, head(&env))
    );
    assert_eq!(
        verify::stale_note(store(&env).conn(), &sess(&env, "s1").repo_id, "1.1").unwrap(),
        None
    );
}

fn prompt(env: &Env, session: &str) -> String {
    context(&env.hook("user-prompt-submit", session))
}

const NUDGE: &str = "bridge-screen: 1.1 is claimed done but not verified (L1). Run the tests and call verify(1.1, level 2).";

fn status_text(env: &Env) -> String {
    String::from_utf8(env.run(&["status"], "").stdout).unwrap()
}

#[test]
fn unverified_work_leads_the_next_briefing_only() {
    let (env, _mcp) = at_l1();
    // The session that claimed it isn't briefed about its own claim.
    let own = start(&env, "s1");
    assert!(!own.contains("Verify first"), "{own}");
    let briefing = start(&env, "s2");
    assert!(
        briefing.contains("Verify first (1): 1.1 Login form"),
        "{briefing}"
    );
    assert_eq!(
        of_kind(&env, "verify_nudged"),
        vec![json!({"task_id": "1.1", "level": 1})]
    );
}

#[test]
fn the_prompt_nudge_repeats_only_after_the_window_and_escalates() {
    let (env, _mcp) = at_l1();
    // The claim was in unit 1: the next prompt carries the nudge.
    assert_eq!(prompt(&env, "s1"), NUDGE);
    assert!(!status_text(&env).contains("needs you: verify"));
    for _ in 0..3 {
        assert_eq!(prompt(&env, "s1"), "");
    }
    // The second nudge, ignored like the first: the owner is asked.
    assert_eq!(prompt(&env, "s1"), NUDGE);
    let owner = of_kind(&env, "needs_owner");
    assert_eq!(owner, vec![json!({"kind": "verify", "task_id": "1.1"})]);
    let text = status_text(&env);
    assert!(
        text.contains(
            "needs you: verify 1.1 Login form — bridge-screen verify 1.1 (or --broken \"…\")"
        ),
        "{text}"
    );
    // A later nudge doesn't ask the owner twice.
    for _ in 0..4 {
        prompt(&env, "s1");
    }
    assert_eq!(of_kind(&env, "needs_owner").len(), 1);
    // L2 answers it.
    let out = env.run(&["verify", "1.1", "--level", "2"], "");
    assert!(out.status.success());
    assert!(!status_text(&env).contains("needs you: verify"));
    assert_eq!(prompt(&env, "s1"), "");
    assert_eq!(env.log(), "");
}

#[test]
fn dependency_ready_goes_once_to_the_waiting_session_only() {
    let (env, mut mcp) = setup();
    let deps = r#"[{"op":"add_dep","id":"1.1","depends_on":"1.2"}]"#;
    assert!(env.run(&["plan", "edit"], deps).status.success());
    env.hook("session-start", "s2");
    let (err, _) = call_as(
        &mut mcp,
        &env,
        "s2",
        "claim_task",
        &json!({"id": "1.2", "mode": "auto", "confidence": 0.9}),
    );
    assert!(!err);
    env.hook("user-prompt-submit", "s2");
    write(&env, "src/reset.rs", "pub fn reset() {}\n");
    edited(&env, "s2", "src/reset.rs");
    commit(&env, "reset");
    for (tool, args) in [
        (
            "claim_done",
            json!({"evidence": "done", "files": ["src/reset.rs"]}),
        ),
        ("verify", l2()),
    ] {
        let (err, text) = call_as(&mut mcp, &env, "s2", tool, &args);
        assert!(!err, "{text}");
    }
    let short: String = head(&env).chars().take(7).collect();
    // s1 works on 1.1, which waits for 1.2.
    assert_eq!(
        prompt(&env, "s1"),
        format!(
            "bridge-screen: 1.2 Password reset is done on `main` @{short}. Merge or rebase to pick it up."
        )
    );
    for _ in 0..5 {
        assert_eq!(prompt(&env, "s1"), "");
    }
    // The verifier itself never hears about its own work.
    let notes: Vec<Value> = of_kind(&env, "note_sent")
        .into_iter()
        .filter(|n| n["kind"] == "dependency_ready")
        .collect();
    assert_eq!(notes.len(), 1);
    // Re-verified at a new commit by the owner: the state changed, so it goes out again.
    write(&env, "src/reset.rs", "pub fn reset() { /* v2 */ }\n");
    commit(&env, "reset v2");
    assert!(env.run(&["verify", "1.2"], "").status.success());
    let short: String = head(&env).chars().take(7).collect();
    assert_eq!(
        prompt(&env, "s1"),
        format!(
            "bridge-screen: 1.2 Password reset is done @{short}. Merge or rebase to pick it up."
        )
    );
}

#[test]
fn dependency_ready_outranks_the_verify_nudge() {
    let (env, _mcp) = at_l1();
    let deps = r#"[{"op":"add_dep","id":"1.1","depends_on":"1.2"}]"#;
    assert!(env.run(&["plan", "edit"], deps).status.success());
    assert!(env.run(&["verify", "1.2"], "").status.success());
    assert!(prompt(&env, "s1").contains("1.2 Password reset is done"));
    // One note per unit: the nudge waits for the next prompt.
    assert_eq!(prompt(&env, "s1"), NUDGE);
}

#[test]
fn status_and_plan_show_the_ladder() {
    let (env, mut mcp) = at_l1();
    let text = status_text(&env);
    assert!(
        text.contains("goal: Ship  · done 0% · claimed 75%"),
        "{text}"
    );
    assert!(text.contains("· verify pending: 1.1"), "{text}");
    let agent = ok_call(&env, &mut mcp, "status", &json!({}));
    assert!(
        agent.contains("Verification: L1 (code exists) — next: run tests, verify level 2"),
        "{agent}"
    );
    assert!(
        agent.contains("Unverified: 1.1 (L1) — verify them"),
        "{agent}"
    );
    let (err, _) = verify_call(&env, &mut mcp, &l2());
    assert!(!err);
    let text = status_text(&env);
    assert!(text.contains("· done 75% · claimed 0%"), "{text}");
    assert!(!text.contains("verify pending"), "{text}");
    // Stale and owner markers in the plan.
    write(
        &env,
        "src/login.rs",
        "pub fn validate_login() { /* v2 */ }\n",
    );
    commit(&env, "login v2");
    start(&env, "s2");
    let plan = String::from_utf8(env.run(&["plan", "show"], "").stdout).unwrap();
    assert!(plan.contains("1.1  Login form  [l1] stale 30m"), "{plan}");
    assert!(env.run(&["verify", "1.1"], "").status.success());
    let plan = String::from_utf8(env.run(&["plan", "show"], "").stdout).unwrap();
    assert!(plan.contains("1.1  Login form  [l4] ✓owner 30m"), "{plan}");
}

fn ok_call(env: &Env, mcp: &mut Mcp, tool: &str, args: &Value) -> String {
    let (err, text) = call_as(mcp, env, "s1", tool, args);
    assert!(!err, "{text}");
    text
}
