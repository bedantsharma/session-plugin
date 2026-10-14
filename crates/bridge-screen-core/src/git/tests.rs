use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;

/// Run git in `dir` for test setup, with a scrubbed environment; panics on failure.
pub(crate) fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A temp repo with a repo-local identity and one commit containing `a.txt`.
pub(crate) fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init_in(dir.path());
    dir
}

pub(crate) fn init_in(path: &Path) {
    sh(path, &["init", "-q", "-b", "main"]);
    configure(path);
    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    sh(path, &["add", "a.txt"]);
    sh(path, &["commit", "-q", "-m", "init"]);
}

/// Repo-local identity and settings, so the user's global config never matters.
pub(crate) fn configure(path: &Path) {
    for (k, v) in [
        ("user.name", "Test"),
        ("user.email", "t@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        sh(path, &["config", k, v]);
    }
}

fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn file<'a>(st: &'a WorktreeStatus, path: &str) -> &'a FileState {
    st.files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} not in {st:?}"))
}

#[test]
fn status_reads_staged_unstaged_untracked_and_renamed() {
    let r = repo();
    let wt = r.path();
    write(wt, "b.txt", "b\n");
    sh(wt, &["add", "b.txt"]);
    sh(wt, &["commit", "-q", "-m", "b"]);
    let clean = status(wt).unwrap();
    assert_eq!(clean.branch.as_deref(), Some("main"));
    assert_eq!(clean.head, head(wt).unwrap());
    assert_eq!(clean.files, Vec::new());
    assert!(!is_dirty(wt).unwrap());

    write(wt, "a.txt", "two\n"); // unstaged
    write(wt, "new dir/ü n.txt", "x\n"); // untracked, space + unicode
    sh(wt, &["mv", "b.txt", "c.txt"]); // staged rename
    write(wt, "s.txt", "s\n");
    sh(wt, &["add", "s.txt"]); // staged new
    write(wt, "s.txt", "s2\n"); // and unstaged on top
    let st = status(wt).unwrap();
    let fa = file(&st, "a.txt");
    assert!(fa.unstaged && !fa.staged && !fa.untracked);
    let fu = file(&st, "new dir/ü n.txt");
    assert!(fu.untracked && !fu.staged && !fu.unstaged);
    let fc = file(&st, "c.txt");
    assert!(fc.staged && !fc.unstaged);
    assert_eq!(fc.orig_path.as_deref(), Some("b.txt"));
    let fs = file(&st, "s.txt");
    assert!(fs.staged && fs.unstaged);
    assert_eq!(st.files.len(), 4);
    assert!(is_dirty(wt).unwrap());
    assert_eq!(st.dirty_paths().len(), 5, "rename counts both sides");
}

#[test]
fn head_is_none_in_an_empty_repo() {
    let dir = tempfile::tempdir().unwrap();
    sh(dir.path(), &["init", "-q", "-b", "main"]);
    assert_eq!(head(dir.path()).unwrap(), None);
    let st = status(dir.path()).unwrap();
    assert_eq!((st.head, st.branch.as_deref()), (None, Some("main")));
}

#[test]
fn errors_are_clear_not_panics() {
    let dir = tempfile::tempdir().unwrap();
    let err = status(dir.path()).unwrap_err();
    assert!(matches!(err, Error::Git(_) | Error::NotARepo(_)), "{err}");
    let r = repo();
    let err = commit_message(r.path(), "0000000000000000000000000000000000000000").unwrap_err();
    assert!(matches!(err, Error::Git(_)), "{err}");
}

#[test]
fn commit_paths_commits_only_those_paths_with_trailers() {
    let r = repo();
    let wt = r.path();
    write(wt, "a.txt", "changed\n");
    write(wt, "keep me.txt", "k\n");
    write(wt, "other.txt", "o\n");
    sh(wt, &["add", "other.txt"]); // staged but not ours
    let before = head(wt).unwrap().unwrap();
    let sha = commit_paths(
        wt,
        &["a.txt".into(), "keep me.txt".into()],
        "wip(bridge): 1.2 Login form",
        &[
            ("Bridge-Task".into(), "1.2".into()),
            (
                "Bridge-Next".into(),
                "add the test: login_rejects_empty".into(),
            ),
        ],
    )
    .unwrap();
    assert_eq!(head(wt).unwrap().as_deref(), Some(sha.as_str()));
    assert_ne!(sha, before);
    assert_eq!(
        trailers(wt, &sha).unwrap(),
        vec![
            ("Bridge-Task".to_string(), "1.2".to_string()),
            (
                "Bridge-Next".to_string(),
                "add the test: login_rejects_empty".to_string()
            ),
        ]
    );
    assert!(
        commit_message(wt, &sha)
            .unwrap()
            .starts_with("wip(bridge): 1.2 Login form\n")
    );
    let changed = sh(wt, &["show", "--name-only", "--format=", &sha]);
    let mut changed: Vec<&str> = changed.lines().collect();
    changed.sort_unstable();
    assert_eq!(changed, vec!["a.txt", "keep me.txt"]);
    // other.txt is still staged and uncommitted.
    let st = status(wt).unwrap();
    assert_eq!(st.files.len(), 1);
    assert!(file(&st, "other.txt").staged);
}

#[test]
fn head_commit_reads_sha_time_subject_and_trailers() {
    let r = repo();
    let wt = r.path();
    write(wt, "a.txt", "x\n");
    let sha = commit_all(
        wt,
        "wip(bridge): 1.1 X",
        &[("Bridge-Next".into(), "go".into())],
    )
    .unwrap();
    let h = head_commit(wt).unwrap().unwrap();
    assert_eq!(
        (h.sha.as_str(), h.subject.as_str()),
        (sha.as_str(), "wip(bridge): 1.1 X")
    );
    assert_eq!(
        h.trailers,
        vec![("Bridge-Next".to_string(), "go".to_string())]
    );
    assert!((h.time - jiff::Timestamp::now().as_second()).abs() < 60);
    let empty = tempfile::tempdir().unwrap();
    sh(empty.path(), &["init", "-q"]);
    assert_eq!(head_commit(empty.path()).unwrap(), None);
}

#[test]
fn commit_paths_handles_a_deleted_file() {
    let r = repo();
    let wt = r.path();
    std::fs::remove_file(wt.join("a.txt")).unwrap();
    commit_paths(wt, &["a.txt".into()], "drop a", &[]).unwrap();
    assert!(!is_dirty(wt).unwrap());
}

#[test]
fn commit_all_commits_everything() {
    let r = repo();
    let wt = r.path();
    write(wt, "a.txt", "changed\n");
    write(wt, "z.txt", "z\n");
    let sha = commit_all(wt, "all", &[("Bridge-Squashed-From".into(), "x".into())]).unwrap();
    assert!(!is_dirty(wt).unwrap());
    assert_eq!(
        trailers(wt, &sha).unwrap(),
        vec![("Bridge-Squashed-From".to_string(), "x".to_string())]
    );
    assert_eq!(
        commit_message(wt, &sha).unwrap().lines().next(),
        Some("all")
    );
}

#[test]
fn stash_paths_takes_tracked_and_untracked_of_those_paths_only() {
    let r = repo();
    let wt = r.path();
    write(wt, "a.txt", "changed\n");
    write(wt, "dir x/new é.txt", "untracked\n");
    write(wt, "leave.txt", "stay\n");
    let stash = stash_paths(
        wt,
        &["a.txt".into(), "dir x/new é.txt".into()],
        "bridge-screen undo L1",
    )
    .unwrap();
    assert_eq!(stash.sha.len(), 40);
    assert!(stash.message.contains("bridge-screen undo L1"));
    assert_eq!(std::fs::read_to_string(wt.join("a.txt")).unwrap(), "one\n");
    assert!(!wt.join("dir x/new é.txt").exists());
    assert_eq!(
        std::fs::read_to_string(wt.join("leave.txt")).unwrap(),
        "stay\n"
    );
    sh(wt, &["stash", "apply", &stash.sha]);
    assert_eq!(
        std::fs::read_to_string(wt.join("a.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("dir x/new é.txt")).unwrap(),
        "untracked\n"
    );
}

#[test]
fn stash_paths_with_nothing_to_stash_is_an_error() {
    let r = repo();
    let err = stash_paths(r.path(), &["a.txt".into()], "nothing").unwrap_err();
    assert!(matches!(err, Error::Git(_)), "{err}");
}

#[test]
fn diff_names_since_covers_committed_and_uncommitted() {
    let r = repo();
    let wt = r.path();
    let base = head(wt).unwrap().unwrap();
    write(wt, "c1.txt", "c\n");
    sh(wt, &["add", "c1.txt"]);
    sh(wt, &["commit", "-q", "-m", "c1"]);
    write(wt, "a.txt", "dirty\n");
    write(wt, "u.txt", "u\n");
    assert_eq!(
        diff_names_since(wt, &base).unwrap(),
        vec!["a.txt", "c1.txt", "u.txt"]
    );
}

#[test]
fn branch_history_merge_base_and_commits_since() {
    let r = repo();
    let wt = r.path();
    let base = head(wt).unwrap().unwrap();
    sh(wt, &["checkout", "-q", "-b", "feat"]);
    write(wt, "f.txt", "1\n");
    commit_all(
        wt,
        "wip(bridge): 1.1 First",
        &[("Bridge-Task".into(), "1.1".into())],
    )
    .unwrap();
    write(wt, "f.txt", "2\n");
    commit_all(wt, "regular change", &[]).unwrap();
    assert_eq!(
        merge_base(wt, "main").unwrap().as_deref(),
        Some(base.as_str())
    );
    assert_eq!(default_base(wt).unwrap(), "main");
    let commits = commits_since(wt, &base).unwrap();
    let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, vec!["wip(bridge): 1.1 First", "regular change"]);
    assert_eq!(
        commits[0].trailers,
        vec![("Bridge-Task".to_string(), "1.1".to_string())]
    );
    assert_eq!(commits[1].trailers, Vec::new());
    assert_eq!(merge_base(wt, "no-such-branch").unwrap(), None);

    create_ref(wt, "refs/bridge-screen/backup/feat/1", &commits[1].sha).unwrap();
    let tip = &commits[1].sha;
    let one = commit_tree(wt, tip, &base, "one", &[("K".into(), "v".into())]).unwrap();
    assert_eq!(sh(wt, &["rev-parse", &format!("{one}^")]), base);
    assert_eq!(
        sh(wt, &["rev-parse", &format!("{one}^{{tree}}")]),
        sh(wt, &["rev-parse", &format!("{tip}^{{tree}}")])
    );
    assert_eq!(trailers(wt, &one).unwrap(), vec![("K".into(), "v".into())]);
    assert_eq!(
        head(wt).unwrap().as_deref(),
        Some(tip.as_str()),
        "no ref moved"
    );
    // Compare and swap: refused unless the branch is still where we think.
    let branch = format!(
        "refs/heads/{}",
        sh(wt, &["symbolic-ref", "--short", "HEAD"])
    );
    assert!(move_ref(wt, &branch, &one, &base, "test").is_err());
    assert_eq!(head(wt).unwrap().as_deref(), Some(tip.as_str()));
    move_ref(wt, &branch, &one, tip, "test").unwrap();
    assert_eq!(head(wt).unwrap().as_deref(), Some(one.as_str()));
    assert_eq!(
        sh(wt, &["rev-parse", "refs/bridge-screen/backup/feat/1"]),
        *tip
    );
    assert!(!is_dirty(wt).unwrap(), "same tree: nothing to commit");
}

#[test]
fn worktree_list_sees_a_linked_worktree() {
    let r = repo();
    let wt = r.path();
    let linked_parent = tempfile::tempdir().unwrap();
    let linked: PathBuf = linked_parent.path().join("linked tree");
    sh(
        wt,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );
    let list = worktree_list(wt).unwrap();
    assert_eq!(list.len(), 2);
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap();
    assert_eq!(canon(&list[0].path), canon(wt));
    assert_eq!(list[0].branch.as_deref(), Some("main"));
    assert_eq!(canon(&list[1].path), canon(&linked));
    assert_eq!(list[1].branch.as_deref(), Some("side"));
    assert_eq!(list[1].head, head(&linked).unwrap());
    assert_eq!(canon(&toplevel(&linked.join(".")).unwrap()), canon(&linked));
}

/// A bare `origin`, a clone of it (ours) and a second clone (someone else).
fn with_origin() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let seed = root.path().join("seed");
    std::fs::create_dir(&seed).unwrap();
    init_in(&seed);
    let origin = root.path().join("origin.git");
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
    (root, ours, theirs)
}

#[test]
fn upstream_and_force_push_with_lease() {
    let (_root, ours, theirs) = with_origin();
    assert_eq!(upstream(&ours).unwrap().as_deref(), Some("origin/main"));
    assert_eq!(default_base(&ours).unwrap(), "origin/main");
    let remote_before = sh(&ours, &["rev-parse", "origin/main"]);
    // Rewrite our history and force-push with the right expectation: accepted.
    write(&ours, "a.txt", "ours\n");
    commit_all(&ours, "ours", &[]).unwrap();
    sh(&ours, &["commit", "-q", "--amend", "-m", "ours amended"]);
    push_force_with_lease(&ours, &remote_before).unwrap();
    let pushed = head(&ours).unwrap().unwrap();
    assert_eq!(
        sh(&theirs, &["ls-remote", "origin", "refs/heads/main"])
            .split_whitespace()
            .next(),
        Some(pushed.as_str())
    );

    // Someone else pushes; our lease on the old value is refused.
    sh(&theirs, &["pull", "-q", "--ff-only"]);
    write(&theirs, "t.txt", "theirs\n");
    commit_all(&theirs, "theirs", &[]).unwrap();
    sh(&theirs, &["push", "-q", "origin", "main"]);
    write(&ours, "a.txt", "ours again\n");
    commit_all(&ours, "ours again", &[]).unwrap();
    let err = push_force_with_lease(&ours, &pushed).unwrap_err();
    assert!(matches!(err, Error::Git(_)), "{err}");
    let remote_now = sh(&theirs, &["rev-parse", "HEAD"]);
    assert_eq!(
        sh(&ours, &["ls-remote", "origin", "refs/heads/main"])
            .split_whitespace()
            .next(),
        Some(remote_now.as_str())
    );
}

#[test]
fn no_upstream_is_none() {
    let r = repo();
    assert_eq!(upstream(r.path()).unwrap(), None);
}

#[test]
fn pathspecs_are_literal() {
    let r = repo();
    let wt = r.path();
    write(wt, "x[1].rs", "glob-looking\n");
    write(wt, "x1.rs", "matches the glob\n");
    write(wt, "star*.rs", "star\n");
    write(wt, "starry.rs", "matches the star\n");
    stash_paths(wt, &["x[1].rs".into(), "star*.rs".into()], "literal").unwrap();
    assert!(!wt.join("x[1].rs").exists());
    assert!(!wt.join("star*.rs").exists());
    assert!(wt.join("x1.rs").exists(), "a glob match was stashed");
    assert!(wt.join("starry.rs").exists(), "a glob match was stashed");
}

#[test]
fn exists_at_and_grep_at_read_a_commit_not_the_worktree() {
    let r = repo();
    let wt = r.path();
    write(
        wt,
        "src/ü dir/log in.rs",
        "pub fn login_form() {}\nstruct Form;\n",
    );
    sh(wt, &["add", "-A"]);
    sh(wt, &["commit", "-q", "-m", "login"]);
    let head = head(wt).unwrap().unwrap();
    let path = "src/ü dir/log in.rs".to_string();
    assert!(exists_at(wt, &head, &path).unwrap());
    assert!(exists_at(wt, &head, "a.txt").unwrap());
    assert!(!exists_at(wt, &head, "src/missing.rs").unwrap());
    assert!(!exists_at(wt, "HEAD~1", &path).unwrap());
    let paths = std::slice::from_ref(&path);
    assert!(grep_at(wt, &head, "login_form", paths).unwrap());
    assert!(grep_at(wt, &head, "Form", paths).unwrap());
    // Whole words only: `login` is part of `login_form`, `orm` of `Form`.
    assert!(!grep_at(wt, &head, "login", paths).unwrap());
    assert!(!grep_at(wt, &head, "orm", paths).unwrap());
    // Only in the given files.
    assert!(!grep_at(wt, &head, "one", paths).unwrap());
    assert!(grep_at(wt, &head, "one", &["a.txt".to_string()]).unwrap());
    // Uncommitted text doesn't count.
    write(wt, "src/ü dir/log in.rs", "pub fn signup() {}\n");
    assert!(!grep_at(wt, &head, "signup", paths).unwrap());
    assert!(grep_at(wt, "nope", "x", paths).is_err());
}

#[test]
fn changed_between_lists_both_sides_of_renames() {
    let r = repo();
    let wt = r.path();
    let base = head(wt).unwrap().unwrap();
    write(wt, "b.txt", "b\n");
    write(wt, "c d.txt", "c\n");
    sh(wt, &["add", "-A"]);
    sh(wt, &["commit", "-q", "-m", "two"]);
    sh(wt, &["mv", "a.txt", "moved.txt"]);
    sh(wt, &["commit", "-q", "-m", "mv"]);
    let now = head(wt).unwrap().unwrap();
    let mut changed = changed_between(wt, &base, &now).unwrap();
    changed.sort();
    assert_eq!(changed, vec!["a.txt", "b.txt", "c d.txt", "moved.txt"]);
    assert_eq!(
        changed_between(wt, &now, &now).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn history_lists_commits_parents_and_touched_files() {
    let r = repo();
    let wt = r.path();
    let first = head(wt).unwrap().unwrap();
    write(wt, "a b.txt", "x\n");
    write(wt, "a.txt", "two\n");
    sh(wt, &["add", "-A"]);
    sh(wt, &["commit", "-q", "-m", "two"]);
    let second = head(wt).unwrap().unwrap();
    sh(wt, &["mv", "a b.txt", "c.txt"]);
    sh(wt, &["commit", "-q", "-m", "mv"]);
    let third = head(wt).unwrap().unwrap();
    let h = history(wt, "HEAD", 10).unwrap();
    assert_eq!(h.len(), 3);
    assert_eq!(h[0].sha, third);
    assert_eq!(h[0].parents, vec![second.clone()]);
    let mut moved = h[0].files.clone();
    moved.sort();
    assert_eq!(moved, vec!["a b.txt", "c.txt"]);
    let mut touched = h[1].files.clone();
    touched.sort();
    assert_eq!(touched, vec!["a b.txt", "a.txt"]);
    assert_eq!((h[2].sha.as_str(), h[2].parents.len()), (first.as_str(), 0));
    assert_eq!(history(wt, "HEAD", 1).unwrap().len(), 1);
}
