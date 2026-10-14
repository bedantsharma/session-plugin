//! `bridge-screen squash` and the automatic squash before `gh pr create` (spec §8.1).

mod common;

use std::path::Path;

use bridge_screen_core::store::{Store, read_all};
use common::{Env, fixture_json, git};
use serde_json::{Value, json};

fn out(dir: &Path, args: &[&str]) -> String {
    let o = common::git_cmd(dir).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}");
    String::from_utf8(o.stdout).unwrap().trim().to_string()
}

fn commit(dir: &Path, file: &str, message: &[&str]) {
    std::fs::write(dir.join(file), format!("{message:?}\n")).unwrap();
    git(dir, &["add", "-A"]);
    let mut args = vec!["commit", "-q"];
    for m in message {
        args.push("-m");
        args.push(m);
    }
    git(dir, &args);
}

/// The env's repo with a bare `origin`, identity config, and branch `feat` holding two
/// WIP commits and a regular one. Returns the origin's temp dir (keep it alive).
fn setup(env: &Env) -> tempfile::TempDir {
    let repo = env.repo.path();
    for (k, v) in [
        ("user.name", "Test"),
        ("user.email", "t@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(repo, &["config", k, v]);
    }
    let origin = tempfile::tempdir().unwrap();
    git(origin.path(), &["init", "-q", "--bare"]);
    git(
        repo,
        &["remote", "add", "origin", origin.path().to_str().unwrap()],
    );
    git(repo, &["push", "-q", "-u", "origin", "main"]);
    git(repo, &["remote", "set-head", "origin", "main"]);
    git(repo, &["checkout", "-q", "-b", "feat"]);
    commit(
        repo,
        "a.rs",
        &["wip(bridge): 1.1 Login", "Bridge-Task: 1.1"],
    );
    commit(repo, "b.rs", &["tidy"]);
    commit(
        repo,
        "c.rs",
        &["wip(bridge): 1.1 Login", "Bridge-Task: 1.1"],
    );
    origin
}

fn kinds(env: &Env) -> Vec<(String, Value)> {
    let store = Store::open(&env.db_path()).unwrap();
    read_all(store.conn(), None)
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.data))
        .collect()
}

fn of_kind(env: &Env, kind: &str) -> Vec<Value> {
    kinds(env)
        .into_iter()
        .filter(|(k, _)| k == kind)
        .map(|(_, d)| d)
        .collect()
}

fn context_of(out: &str) -> String {
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

fn pr(env: &Env, session: &str, command: &str) -> String {
    let mut p = fixture_json("pre-tool-use", session, &env.repo_path());
    p["tool_name"] = json!("Bash");
    p["tool_input"] = json!({"command": command});
    env.hook_raw("pre-tool-use", &p.to_string())
}

#[test]
fn cli_squash_collapses_the_branch() {
    let env = Env::new();
    let _origin = setup(&env);
    git(env.repo.path(), &["push", "-q", "-u", "origin", "feat"]);
    let old = out(env.repo.path(), &["rev-parse", "HEAD"]);
    let o = env.run(&["squash"], "");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8(o.stdout).unwrap();
    let new = out(env.repo.path(), &["rev-parse", "HEAD"]);
    assert!(
        text.starts_with(&format!("Squashed 3 commits on feat into {}.", &new[..8])),
        "{text}"
    );
    assert!(
        text.contains("Backup: refs/bridge-screen/backup/feat/"),
        "{text}"
    );
    assert!(text.contains("Pushed"), "{text}");
    let backup = text
        .split("Backup: ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .trim_end_matches('.');
    assert_eq!(out(env.repo.path(), &["rev-parse", backup]), old);
    let squashed = of_kind(&env, "squashed");
    assert_eq!(squashed.len(), 1);
    assert_eq!(squashed[0]["new_sha"], new.as_str());
    assert_eq!(squashed[0]["old_shas"].as_array().unwrap().len(), 3);
}

#[test]
fn cli_dry_run_and_refusal_change_nothing() {
    let env = Env::new();
    let _origin = setup(&env);
    let old = out(env.repo.path(), &["rev-parse", "HEAD"]);
    let o = env.run(&["squash", "--dry-run", "--onto", "main"], "");
    assert!(o.status.success());
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(
        text.starts_with("Would squash 3 commits on feat onto "),
        "{text}"
    );
    assert!(text.contains("feat: 3 commits squashed"), "{text}");
    assert_eq!(out(env.repo.path(), &["rev-parse", "HEAD"]), old);

    std::fs::write(env.repo.path().join("a.rs"), "dirty\n").unwrap();
    let o = env.run(&["squash"], "");
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("uncommitted changes"), "{err}");
    assert!(err.contains("history unchanged"), "{err}");
    assert_eq!(out(env.repo.path(), &["rev-parse", "HEAD"]), old);
    assert_eq!(of_kind(&env, "squashed"), Vec::<Value>::new());
}

#[test]
fn gh_pr_create_squashes_first_and_proceeds() {
    let env = Env::new();
    let _origin = setup(&env);
    env.hook("session-start", "s1");
    let o = pr(&env, "s1", "gh pr create --fill --base main");
    let text = context_of(&o);
    assert!(
        text.starts_with("bridge-screen: squashed 3 commits into one before the PR"),
        "{text}"
    );
    assert_eq!(
        out(env.repo.path(), &["rev-list", "--count", "main..HEAD"]),
        "1"
    );
    assert_eq!(of_kind(&env, "squashed").len(), 1);
    // Nothing left to squash: silent.
    assert_eq!(pr(&env, "s1", "gh pr create --fill"), "");
    assert_eq!(env.log(), "");
}

#[test]
fn failed_squash_before_a_pr_says_so_and_never_blocks() {
    let env = Env::new();
    let _origin = setup(&env);
    env.hook("session-start", "s1");
    std::fs::write(env.repo.path().join("b.rs"), "dirty\n").unwrap();
    let old = out(env.repo.path(), &["rev-parse", "HEAD"]);
    let o = pr(&env, "s1", "gh pr create --title x --body y");
    assert_eq!(
        context_of(&o),
        "bridge-screen: WIP commits weren't squashed (the worktree has uncommitted changes; commit or stash them first); the PR will show them. Run bridge-screen squash later if you want them collapsed."
    );
    assert_eq!(out(env.repo.path(), &["rev-parse", "HEAD"]), old);
    assert_eq!(
        of_kind(&env, "squash_failed"),
        vec![json!({"reason": "the worktree has uncommitted changes; commit or stash them first"})]
    );
    assert_eq!(
        of_kind(&env, "needs_owner"),
        vec![json!({"kind": "squash"})]
    );
    // The GitHub MCP tool counts too.
    let mut p = fixture_json("pre-tool-use", "s1", &env.repo_path());
    p["tool_name"] = json!("mcp__github__create_pull_request");
    p["tool_input"] = json!({"base": "main", "head": "feat", "title": "x"});
    let o = env.hook_raw("pre-tool-use", &p.to_string());
    assert!(context_of(&o).starts_with("bridge-screen: WIP commits weren't squashed"));
    assert_eq!(env.log(), "");
}
