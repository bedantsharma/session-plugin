//! Plan files: imported at SessionStart and by `bridge-screen import` (spec §12.1).

mod common;

use std::path::PathBuf;

use bridge_screen_core::plan::load_tree;
use bridge_screen_core::store::{Store, read_all};
use common::Env;
use serde_json::Value;

const PLAN: &str = "# Login Implementation Plan\n\n**Goal:** People can sign in.\n\n\
                    ### Task 1: Endpoint\n\n**Files:**\n- Create: `src/login.rs`\n\n\
                    - [x] **Step 1: Write the test**\n- [ ] **Step 2: Implement**\n\n\
                    ### Task 2: Form\n\n- [ ] **Step 1: Render**\n";

fn plans_dir(env: &Env) -> PathBuf {
    let d = env.repo.path().join("docs/superpowers/plans");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn context(out: &str) -> String {
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

fn titles(env: &Env) -> Vec<String> {
    let store = Store::open(&env.db_path()).unwrap();
    let repo: String = store
        .conn()
        .query_row("SELECT id FROM repos", [], |r| r.get(0))
        .unwrap();
    load_tree(store.conn(), &repo, false)
        .unwrap()
        .into_iter()
        .map(|n| format!("{} {}", n.id, n.title))
        .collect()
}

fn kinds(env: &Env) -> Vec<String> {
    let store = Store::open(&env.db_path()).unwrap();
    read_all(store.conn(), None)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

#[test]
fn session_start_imports_a_plan_file() {
    let env = Env::new();
    let file = plans_dir(&env).join("2026-10-21-login.md");
    std::fs::write(&file, PLAN).unwrap();
    let text = context(&env.hook("session-start", "s1"));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[1], "Goal: Login — People can sign in.");
    assert_eq!(
        lines[2],
        "Imported 2026-10-21-login: added 5 tasks and steps; 1 ticked step recorded as claimed (not checked)"
    );
    assert_eq!(
        titles(&env),
        vec![
            "G Login",
            "1 Endpoint",
            "1.1 Write the test",
            "1.2 Implement",
            "2 Form",
            "2.1 Render"
        ]
    );
    let k = kinds(&env);
    assert!(k.contains(&"claimed_done".to_string()));
    assert!(!k.contains(&"verified".to_string()));
    assert!(!k.contains(&"checkpoint_due".to_string()));
    // A second session: nothing new to import, and the line is gone.
    let again = context(&env.hook("session-start", "s2"));
    assert!(!again.contains("Imported"), "{again}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), PLAN);
    assert_eq!(env.log(), "");
}

#[test]
fn session_start_survives_bad_plans() {
    let env = Env::new();
    let dir = plans_dir(&env);
    std::fs::write(dir.join("empty.md"), "").unwrap();
    std::fs::write(dir.join("notes.md"), "# Notes\n- [ ] not a task\n").unwrap();
    std::fs::write(dir.join("bin.md"), [0xff_u8, 0xfe, 0, 7]).unwrap();
    std::fs::write(dir.join("dup.md"), "### Task 1: A\n### Task 1: B\n").unwrap();
    std::fs::write(dir.join("huge.md"), "y".repeat(5 << 20)).unwrap();
    let text = context(&env.hook("session-start", "s1"));
    assert!(text.starts_with("bridge-screen briefing"), "{text}");
    // Only dup.md had a task.
    assert_eq!(titles(&env), vec!["G dup", "1 A"]);

    // An unreadable folder: the briefing still goes out, the problem goes to the log.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let text = context(&env.hook("session-start", "s2"));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(text.starts_with("bridge-screen briefing"), "{text}");
        assert!(env.log().contains("plan import"), "{}", env.log());
    }
}

#[test]
fn cli_import_dry_run_then_for_real() {
    let env = Env::new();
    let file = plans_dir(&env).join("login.md");
    std::fs::write(&file, PLAN).unwrap();
    let out = env.run(&["import", "--dry-run"], "");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("Dry run: nothing was written."), "{text}");
    assert!(text.contains("  - would add task: Endpoint"), "{text}");
    assert_eq!(titles(&env), Vec::<String>::new());

    let out = env.run(&["import"], "");
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        text,
        "Imported login: added 5 tasks and steps; 1 ticked step recorded as claimed (not checked)\n"
    );
    let out = env.run(&["import"], "");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "No new or changed plan files.\n"
    );
    // One file by hand, linked into an existing milestone.
    std::fs::write(
        plans_dir(&env).join("extra.md"),
        "# Extra\n\n### Task 1: Form\n- [ ] More\n",
    )
    .unwrap();
    let ops =
        r#"[{"op":"add_node","parent":"G","kind":"milestone","title":"Later","intent":"after"}]"#;
    assert!(env.run(&["plan", "edit"], ops).status.success());
    let out = env.run(
        &["import", "docs/superpowers/plans/extra.md", "--into", "3"],
        "",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        titles(&env).contains(&"3.1 Form".to_string()),
        "{:?}",
        titles(&env)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), PLAN);
}
