use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;
use crate::git::tests::{configure, init_in, sh};
use crate::store::{EventDraft, Store};

fn write(dir: &Path, path: &str, text: &str) {
    std::fs::write(dir.join(path), text).unwrap();
}

fn commit(dir: &Path, file: &str, message: &[&str]) {
    write(dir, file, &format!("{message:?}\n"));
    sh(dir, &["add", "-A"]);
    let mut args = vec!["commit", "-q"];
    for m in message {
        args.push("-m");
        args.push(m);
    }
    sh(dir, &args);
}

/// A plan with tasks 1.1 and 1.2.
fn plan() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    for (id, parent, kind, title) in [
        ("G", None, "goal", "Ship"),
        ("1", Some("G"), "milestone", "Accounts"),
        ("1.1", Some("1"), "task", "Login form"),
        ("1.2", Some("1"), "task", "Password reset"),
    ] {
        s.append(EventDraft::new(
            "r",
            "owner",
            "node_added",
            json!({"id": id, "parent_id": parent, "kind": kind, "title": title,
                   "intent": "why", "expected_files": []}),
        ))
        .unwrap();
    }
    s
}

/// A bare origin, our clone on branch `feat` (pushed) with WIP and regular commits,
/// and a second clone. Returns (root, ours, theirs).
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let seed = root.path().join("seed");
    std::fs::create_dir(&seed).unwrap();
    init_in(&seed);
    sh(
        root.path(),
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_str().unwrap(),
            "origin.git",
        ],
    );
    let origin = root.path().join("origin.git");
    let ours = root.path().join("ours");
    let theirs = root.path().join("theirs");
    for dir in [&ours, &theirs] {
        sh(
            root.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                dir.to_str().unwrap(),
            ],
        );
        configure(dir);
    }
    sh(&ours, &["checkout", "-q", "-b", "feat"]);
    commit(
        &ours,
        "a.rs",
        &[
            "wip(bridge): 1.1 Login form",
            "Bridge-Task: 1.1\nBridge-Next: x",
        ],
    );
    commit(&ours, "b.rs", &["tidy the helpers"]);
    commit(
        &ours,
        "c.rs",
        &["wip(bridge): 1.2 Password reset", "Bridge-Task: 1.2"],
    );
    (root, ours, theirs)
}

fn head(dir: &Path) -> String {
    sh(dir, &["rev-parse", "HEAD"])
}

#[test]
fn squash_collapses_wip_and_regular_commits_with_a_backup() {
    let s = plan();
    let (_root, ours, _) = setup();
    let old = head(&ours);
    let base = sh(&ours, &["rev-parse", "main"]);
    let done = squash(&ours, &Options::default(), s.conn(), "r").unwrap();
    let Outcome::Squashed(sq) = done else {
        panic!("{done:?}")
    };
    assert_eq!(sq.branch, "feat");
    assert_eq!(sq.old_shas.len(), 3);
    assert_eq!(sq.old_shas[2], old);
    assert_eq!(sq.push, None, "never pushed: nothing to push");
    assert_eq!(head(&ours), sq.new_sha);
    assert_eq!(sh(&ours, &["rev-parse", "HEAD~1"]), base);
    assert_eq!(sh(&ours, &["rev-parse", &sq.backup_ref]), old);
    assert!(sq.backup_ref.starts_with("refs/bridge-screen/backup/feat/"));
    let message = crate::git::commit_message(&ours, &sq.new_sha).unwrap();
    assert_eq!(
        message.trim_end(),
        format!(
            "feat: 3 commits squashed\n\n1.1 Login form\n1.2 Password reset\ntidy the helpers\n\nBridge-Squashed-From: {}",
            sq.backup_ref
        )
    );
    // Same tree as before.
    assert_eq!(sh(&ours, &["diff", &old, "HEAD", "--stat"]), "");
}

#[test]
fn squash_refuses_unsafe_states() {
    let s = plan();
    let check = |dir: &Path, opts: &Options, want: Refusal| {
        let before = head(dir);
        let refs = sh(dir, &["for-each-ref"]);
        match squash(dir, opts, s.conn(), "r").unwrap() {
            Outcome::Refused(r) => assert_eq!(r, want),
            other => panic!("{other:?}"),
        }
        assert_eq!(head(dir), before, "history untouched");
        assert_eq!(sh(dir, &["for-each-ref"]), refs, "no backup ref");
    };
    // Dirty.
    let (_r1, ours, _) = setup();
    write(&ours, "a.rs", "dirty\n");
    check(&ours, &Options::default(), Refusal::Dirty);
    // No WIP commits.
    let (_r2, ours, _) = setup();
    sh(&ours, &["checkout", "-q", "-b", "plain", "main"]);
    commit(&ours, "p.rs", &["plain work"]);
    check(&ours, &Options::default(), Refusal::NoWip);
    // Detached HEAD.
    let (_r3, ours, _) = setup();
    sh(&ours, &["checkout", "-q", "--detach"]);
    check(&ours, &Options::default(), Refusal::Detached);
    // The remote branch moved since our last fetch.
    let (_r4, ours, theirs) = setup();
    sh(&ours, &["push", "-q", "-u", "origin", "feat"]);
    sh(&theirs, &["fetch", "-q", "origin"]);
    sh(&theirs, &["checkout", "-q", "-b", "feat", "origin/feat"]);
    commit(&theirs, "t.rs", &["their change"]);
    sh(&theirs, &["push", "-q", "origin", "feat"]);
    check(
        &ours,
        &Options::default(),
        Refusal::RemoteMoved("origin/feat".into()),
    );
    // An unknown base.
    let (_r5, ours, _) = setup();
    let opts = Options {
        onto: Some("nope".into()),
        dry_run: false,
    };
    check(&ours, &opts, Refusal::NoBase("nope".into()));
    assert!(Refusal::Dirty.to_string().contains("uncommitted"));
}

#[test]
fn dry_run_changes_nothing() {
    let s = plan();
    let (_root, ours, _) = setup();
    let before = head(&ours);
    let refs = sh(&ours, &["for-each-ref"]);
    let opts = Options {
        onto: Some("main".into()),
        dry_run: true,
    };
    let Outcome::DryRun(p) = squash(&ours, &opts, s.conn(), "r").unwrap() else {
        panic!()
    };
    assert_eq!(p.commits.len(), 3);
    assert_eq!(p.wips, 2);
    assert!(p.message.starts_with("feat: 3 commits squashed\n"));
    assert_eq!(head(&ours), before);
    assert_eq!(sh(&ours, &["for-each-ref"]), refs);
}

#[test]
fn pushed_branch_is_force_pushed_with_lease() {
    let s = plan();
    let (_root, ours, theirs) = setup();
    sh(&ours, &["push", "-q", "-u", "origin", "feat"]);
    let Outcome::Squashed(sq) = squash(&ours, &Options::default(), s.conn(), "r").unwrap() else {
        panic!()
    };
    assert_eq!(sq.push, Some(Ok(())));
    let remote = sh(&theirs, &["ls-remote", "origin", "refs/heads/feat"]);
    assert!(remote.starts_with(&sq.new_sha), "{remote}");
}

#[test]
fn pr_commands_and_bases() {
    assert!(is_pr_command("gh pr create --fill"));
    assert!(is_pr_command("git push && gh  pr create -B dev"));
    assert!(!is_pr_command("gh pr list"));
    assert!(!is_pr_command("echo gh pr"));
    assert_eq!(pr_base("gh pr create -B dev").as_deref(), Some("dev"));
    assert_eq!(
        pr_base("gh pr create --base=\"rel\"").as_deref(),
        Some("rel")
    );
    assert_eq!(
        pr_base("gh pr create --base main --fill").as_deref(),
        Some("main")
    );
    assert_eq!(pr_base("gh pr create --fill"), None);
}

#[test]
fn squash_runs_no_commit_hooks_and_leaves_index_and_worktree_alone() {
    use std::os::unix::fs::PermissionsExt;
    let s = plan();
    let (root, ours, _) = setup();
    // Commit hooks that would fail (and leave a mark) if they ran.
    let hooks = root.path().join("hooks");
    std::fs::create_dir(&hooks).unwrap();
    let mark = root.path().join("hook-ran");
    for name in [
        "pre-commit",
        "commit-msg",
        "prepare-commit-msg",
        "post-commit",
    ] {
        let path = hooks.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", mark.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    sh(
        &ours,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    let old = head(&ours);
    let index = sh(&ours, &["ls-files", "--stage"]);
    let Outcome::Squashed(sq) = squash(&ours, &Options::default(), s.conn(), "r").unwrap() else {
        panic!()
    };
    assert!(!mark.exists(), "a commit hook ran");
    assert_eq!(head(&ours), sq.new_sha);
    assert_eq!(
        sh(&ours, &["rev-parse", "HEAD^{tree}"]),
        sh(&ours, &["rev-parse", &format!("{old}^{{tree}}")])
    );
    assert_eq!(sh(&ours, &["status", "--porcelain"]), "");
    assert_eq!(sh(&ours, &["ls-files", "--stage"]), index);
    assert_eq!(sh(&ours, &["rev-parse", &sq.backup_ref]), old);
    assert_eq!(sh(&ours, &["symbolic-ref", "--short", "HEAD"]), "feat");
}
