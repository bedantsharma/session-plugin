use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// `git -C <dir>` without inherited variables (a git hook's `GIT_DIR` would otherwise point
/// the temp repo's setup at the real repository).
fn git_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_PREFIX",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

struct Env {
    home: tempfile::TempDir,
    repo: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let repo = tempfile::tempdir().unwrap();
        let ok = git_cmd(repo.path()).args(["init", "-q"]).status().unwrap();
        assert!(ok.success());
        Self {
            home: tempfile::tempdir().unwrap(),
            repo,
        }
    }

    fn run(&self, args: &[&str], stdin: Option<&str>, cwd: &Path) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bridge-screen"))
            .args(args)
            .current_dir(cwd)
            .env("BRIDGE_SCREEN_HOME", self.home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        child.wait_with_output().unwrap()
    }

    fn ok(&self, args: &[&str], stdin: Option<&str>) -> String {
        let out = self.run(args, stdin, self.repo.path());
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Core","intent":"Events first"},
  {"op":"add_node","parent":"1","kind":"task","title":"Schema","estimate_min":30}
]"#;

#[test]
fn version_flag_prints_version() {
    let env = Env::new();
    assert!(env.ok(&["--version"], None).starts_with("bridge-screen "));
}

#[test]
fn install_creates_home_db_and_config() {
    let env = Env::new();
    // --no-plugin: never let a test reach the real `claude` CLI.
    let out = env.ok(&["install", "--no-plugin"], None);
    assert!(env.home.path().join("bridge-screen.db").is_file());
    assert!(env.home.path().join("config.toml").is_file());
    assert!(out.contains("config.toml (created)"));
    assert!(
        env.ok(&["install", "--no-plugin"], None)
            .contains("config.toml (kept existing)")
    );
}

#[test]
fn empty_plan_explains_next_step() {
    let env = Env::new();
    let out = env.ok(&["plan", "show"], None);
    assert!(out.starts_with("No plan yet for "));
    assert!(out.contains("bridge-screen plan edit"));
}

#[test]
fn edit_then_show() {
    let env = Env::new();
    let out = env.ok(&["plan", "edit", "--reason", "initial plan"], Some(SEED));
    assert_eq!(out, "applied 3 operation(s)\nadded G\nadded 1\nadded 1.1\n");
    let shown = env.ok(&["plan", "show"], None);
    assert_eq!(
        shown,
        "G  Ship  [todo]\n   ↳ Owner sees progress\n  1  Core  [todo]\n     ↳ Events first\n    1.1  Schema  [todo] 30m\n"
    );
}

#[test]
fn edit_reads_file_and_dir_flag() {
    let env = Env::new();
    let file = env.home.path().join("ops.json");
    std::fs::write(&file, SEED).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let repo = env.repo.path().to_str().unwrap();
    let out = env.run(
        &["plan", "edit", "-C", repo, "--file", file.to_str().unwrap()],
        None,
        elsewhere.path(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(env.ok(&["plan", "show"], None).contains("1.1  Schema"));
}

#[test]
fn invalid_batch_changes_nothing() {
    let env = Env::new();
    env.ok(&["plan", "edit"], Some(SEED));
    let before = env.ok(&["plan", "show"], None);
    let bad = r#"[{"op":"add_node","parent":"1","kind":"task","title":"Fine"},
                  {"op":"add_node","parent":"1.1","kind":"milestone","title":"Bad","intent":"x"}]"#;
    let out = env.run(&["plan", "edit"], Some(bad), env.repo.path());
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with("error: op 1: a milestone cannot go under task"),
        "{err}"
    );
    assert_eq!(env.ok(&["plan", "show"], None), before);
}

#[test]
fn malformed_json_is_explained() {
    let env = Env::new();
    let out = env.run(&["plan", "edit"], Some("{not json"), env.repo.path());
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("JSON array of plan operations"));
}

#[test]
fn outside_git_repo_reports_clearly() {
    let env = Env::new();
    let plain = tempfile::tempdir().unwrap();
    let out = env.run(&["plan", "show"], None, plain.path());
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a git repository"));
}

#[test]
fn rebuild_reports_event_count() {
    let env = Env::new();
    env.ok(&["plan", "edit"], Some(SEED));
    assert_eq!(env.ok(&["rebuild"], None), "rebuilt views from 3 events\n");
}

#[test]
fn config_prints_effective_values() {
    let env = Env::new();
    std::fs::write(env.home.path().join("config.toml"), "points_minutes = 20\n").unwrap();
    let out = env.ok(&["config"], None);
    assert!(out.contains("points_minutes = 20"));
    assert!(out.contains("crash_timeout_min = 30"));
}

#[test]
fn linked_worktree_shows_the_same_plan() {
    let env = Env::new();
    let git = |args: &[&str]| {
        let ok = git_cmd(env.repo.path())
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(ok.success(), "git {args:?} failed");
    };
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    let parent = tempfile::tempdir().unwrap();
    let wt = parent.path().join("wt");
    git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "feature",
        wt.to_str().unwrap(),
    ]);

    env.ok(&["plan", "edit"], Some(SEED));
    let out = env.run(&["plan", "show"], None, &wt);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        env.ok(&["plan", "show"], None)
    );
}

#[test]
fn empty_batch_is_refused() {
    let env = Env::new();
    let out = env.run(&["plan", "edit"], Some("[]"), env.repo.path());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "error: no operations given\n"
    );
}

#[test]
fn replan_needs_a_reason() {
    let env = Env::new();
    env.ok(&["plan", "edit"], Some(SEED));
    let drop_task = r#"[{"op":"remove_node","id":"1.1"}]"#;
    let out = env.run(
        &["plan", "edit", "--replan"],
        Some(drop_task),
        env.repo.path(),
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("a replan needs a reason"));
    env.ok(
        &["plan", "edit", "--replan", "--reason", "scope cut"],
        Some(drop_task),
    );
    assert!(!env.ok(&["plan", "show"], None).contains("Schema"));
    assert!(
        env.ok(&["plan", "show", "--all"], None)
            .contains("1.1  Schema  [dropped]")
    );
    assert_eq!(env.ok(&["rebuild"], None), "rebuilt views from 5 events\n");
}

#[test]
fn config_shows_and_resets_drift_tuning() {
    use bridge_screen_core::store::{EventDraft, Store};
    let env = Env::new();
    let repo = bridge_screen_core::repo::identify(env.repo.path()).unwrap();
    {
        std::fs::create_dir_all(env.home.path()).unwrap();
        let mut s = Store::open(&env.home.path().join("bridge-screen.db")).unwrap();
        s.register_repo(&repo).unwrap();
        s.append(EventDraft::new(
            &repo.id,
            "owner",
            "config_changed",
            serde_json::json!({"scope": "repo", "key": "drift_lower", "old": 0.3, "new": 0.4,
                               "samples": 20}),
        ))
        .unwrap();
    }
    let out = env.ok(&["config"], None);
    assert!(
        out.contains(&format!(
            "# tuned for {}: drift_lower = 0.4, drift_upper = 0.7, tolerance_pct = 20",
            repo.name
        )),
        "{out}"
    );
    assert!(out.contains("drift_lower = 0.3"), "{out}");
    assert_eq!(
        env.ok(&["config", "reset-drift"], None),
        format!(
            "Reset drift tuning for {}: drift_lower 0.4 → 0.3.\n",
            repo.name
        )
    );
    assert_eq!(
        env.ok(&["config", "reset-drift"], None),
        format!(
            "Drift tuning for {} is already at the defaults.\n",
            repo.name
        )
    );
    assert!(!env.ok(&["config"], None).contains("# tuned"));
}

/// A second worktree of the env's repo (the repo needs a commit first).
fn second_worktree(env: &Env) -> tempfile::TempDir {
    let ok = git_cmd(env.repo.path())
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ])
        .status()
        .unwrap();
    assert!(ok.success());
    let dir = tempfile::tempdir().unwrap();
    let ok = git_cmd(env.repo.path())
        .args(["worktree", "add", "-q", "-b", "other"])
        .arg(dir.path().join("wt"))
        .status()
        .unwrap();
    assert!(ok.success());
    dir
}

#[test]
fn cli_set_up_creates_pages_and_says_where() {
    let env = Env::new();
    let out = env.ok(&["pages", "set-up"], None);
    assert!(out.contains("Project pages are set up"), "{out}");
    assert!(out.contains("nothing was written into the repo"), "{out}");
    let folder = env.ok(&["pages", "path"], None);
    let folder = std::path::PathBuf::from(folder.trim());
    assert!(out.contains(folder.to_str().unwrap()), "{out}");
    assert!(folder.starts_with(env.home.path()));
    for page in [
        "intent.md",
        "project-map.md",
        "current-state.md",
        "decision-log.md",
        "glossary.md",
    ] {
        assert!(folder.join(page).is_file(), "{page}");
    }
    // Nothing inside the repo.
    let in_repo: Vec<_> = std::fs::read_dir(env.repo.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n != ".git")
        .collect();
    assert!(in_repo.is_empty(), "{in_repo:?}");
    let again = env.ok(&["pages", "set-up"], None);
    assert!(again.contains("already set up"), "{again}");
}

#[test]
fn cli_decline_then_set_up_works() {
    let env = Env::new();
    let out = env.ok(&["pages", "decline"], None);
    assert!(out.contains("won't offer"), "{out}");
    let shown = env.ok(&["pages", "show"], None);
    assert!(shown.contains("chose not to"), "{shown}");
    let out = env.ok(&["pages", "set-up"], None);
    assert!(out.contains("Project pages are set up"), "{out}");
    // Declining once they exist is refused.
    let refused = env.run(&["pages", "decline"], None, env.repo.path());
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("already set up"));
}

#[test]
fn cli_show_lists_five_pages() {
    let env = Env::new();
    let before = env.ok(&["pages", "show"], None);
    assert!(before.contains("not set up"), "{before}");
    assert!(before.contains("bridge-screen pages set-up"), "{before}");
    env.ok(&["pages", "set-up"], None);
    let shown = env.ok(&["pages", "show"], None);
    for title in [
        "What this project is for",
        "How it fits together",
        "Where things stand",
        "Why we chose what we chose",
        "Words we use",
    ] {
        assert!(shown.contains(title), "{shown}");
    }
    assert!(shown.contains(env.home.path().to_str().unwrap()), "{shown}");
    let today = &jiff::Timestamp::now().to_string()[..10];
    assert!(shown.contains(today), "{shown}");
}

#[test]
fn cli_show_prints_each_pages_text() {
    let env = Env::new();
    env.ok(&["pages", "set-up"], None);
    let id = bridge_screen_core::repo::identify(env.repo.path())
        .unwrap()
        .id;
    let home = bridge_screen_core::paths::Home::at(env.home.path());
    bridge_screen_core::pages::write(
        &home,
        &id,
        bridge_screen_core::pages::Page::CurrentState,
        "Login works; payments are not started yet.",
        bridge_screen_core::pages::WriteMode::Replace,
        None,
    )
    .unwrap();
    let shown = env.ok(&["pages", "show"], None);
    assert!(
        shown.contains("Login works; payments are not started yet."),
        "{shown}"
    );
}

#[test]
fn cli_path_prints_folder_for_any_worktree() {
    let env = Env::new();
    let wt = second_worktree(&env);
    let main = env.ok(&["pages", "path"], None);
    let other = env.run(&["pages", "path"], None, &wt.path().join("wt"));
    assert!(other.status.success());
    assert_eq!(String::from_utf8(other.stdout).unwrap(), main);
    // -C works too.
    let via_c = env.ok(
        &[
            "pages",
            "path",
            "-C",
            wt.path().join("wt").to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(via_c, main);
}
