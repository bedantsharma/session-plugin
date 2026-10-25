//! A half-built repo: `bridge-screen init` (evidence, read-only) and the `init_plan` tool
//! (spec §12.2), plus the estimate ask at a task's first claim (spec §12.1).

mod common;

use bridge_screen_core::plan::{NodeSource, NodeStatus, get_node, load_tree};
use bridge_screen_core::session;
use bridge_screen_core::store::Store;
use common::{Env, Mcp, call_as, git, git_cmd};
use serde_json::{Value, json};

fn status_porcelain(env: &Env) -> String {
    let out = git_cmd(env.repo.path())
        .args(["status", "--porcelain", "--ignored"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

fn half_built(env: &Env) {
    let root = env.repo.path();
    std::fs::write(
        root.join("README.md"),
        "# Widget\n\nA tool that makes widgets.\nFast ones.\n\n## Usage\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("docs/guide")).unwrap();
    std::fs::write(root.join("docs/guide/intro.md"), "hi\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn login() {}\n// TODO: tidy\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"w\"\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "feat: widgets"]);
    git(
        root,
        &["commit", "-q", "--allow-empty", "-m", "fix: faster"],
    );
}

#[test]
fn evidence_reads_without_writing() {
    let env = Env::new();
    half_built(&env);
    let before = status_porcelain(&env);
    let out = env.run(&["init", "--goal", "Ship widgets"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("README: Widget — A tool that makes widgets. Fast ones."),
        "{text}"
    );
    assert!(text.contains("History: 3 commits"), "{text}");
    assert!(text.contains("  - fix: faster"), "{text}");
    assert!(text.contains("  - docs/guide/intro.md"), "{text}");
    assert!(text.contains("TODO/FIXME: 1\n  - src/a.rs: 1"), "{text}");
    assert!(
        text.contains("Tests: probably `cargo test --workspace` (a guess"),
        "{text}"
    );
    assert!(text.contains("(\"Ship widgets\")"), "{text}");
    assert!(text.contains("4. Call init_plan"), "{text}");
    assert_eq!(
        status_porcelain(&env),
        before,
        "init must not touch the repo"
    );
}

#[test]
fn init_says_import_first_and_stops_on_an_existing_plan() {
    let env = Env::new();
    let plans = env.repo.path().join("docs/superpowers/plans");
    std::fs::create_dir_all(&plans).unwrap();
    std::fs::write(plans.join("p.md"), "### Task 1: A\n").unwrap();
    let text = String::from_utf8(env.run(&["init"], "").stdout).unwrap();
    assert!(
        text.contains("Plan files: 1 (1 not imported yet)"),
        "{text}"
    );
    assert!(text.contains("First: run `bridge-screen import`"), "{text}");
    // Nothing in the file says what is finished, so the owner is asked first.
    let asked = String::from_utf8(env.run(&["import"], "").stdout).unwrap();
    assert!(asked.contains("Found 1 plan file"), "{asked}");
    assert!(env.run(&["import", "--fresh"], "").status.success());
    let text = String::from_utf8(env.run(&["init"], "").stdout).unwrap();
    assert!(
        text.contains("already has a plan; nothing to set up"),
        "{text}"
    );
}

fn draft() -> Value {
    json!({
        "goal": {"title": "Widgets", "intent": "People get widgets. Fast."},
        "milestones": [{
            "title": "Core", "intent": "The engine works.",
            "tasks": [
                {"title": "Login", "estimate_min": 30, "expected_files": ["src/a.rs"],
                 "done": {"evidence": "in the first commit", "files": ["src/a.rs"], "symbols": ["login"]}},
                {"title": "Logout", "done": {"evidence": "said so", "files": ["src/gone.rs"]}},
                {"title": "Profile"}
            ]
        }],
        "tasks": [{"title": "Docs"}],
        "owner_said": "yes, that's the plan"
    })
}

fn repo_id(env: &Env) -> String {
    let store = Store::open(&env.db_path()).unwrap();
    session::get(store.conn(), "s1").unwrap().unwrap().repo_id
}

#[test]
fn init_plan_builds_the_tree_and_l1() {
    let env = Env::new();
    half_built(&env);
    env.hook("session-start", "s1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (err, text) = call_as(&mut mcp, &env, "s1", "init_plan", &draft());
    assert!(!err, "{text}");
    assert_eq!(
        text,
        "Plan created: 1 milestone, 4 tasks; 2 look done (checked: 1 exist at HEAD)."
    );
    let store = Store::open(&env.db_path()).unwrap();
    let repo = repo_id(&env);
    let st = |id: &str| get_node(store.conn(), &repo, id).unwrap().unwrap().status;
    assert_eq!(st("1.1"), NodeStatus::L1);
    assert_eq!(st("1.2"), NodeStatus::Claimed);
    assert_eq!(st("1.3"), NodeStatus::Todo);
    assert_eq!(st("2"), NodeStatus::Todo);
    let tree = load_tree(store.conn(), &repo, false).unwrap();
    assert!(
        tree.iter().all(|n| n.source == NodeSource::Init),
        "{tree:?}"
    );
    assert_eq!(
        get_node(store.conn(), &repo, "1.1")
            .unwrap()
            .unwrap()
            .estimate_min,
        Some(30)
    );
    // Nothing above L1, and no checkpoint asked for.
    let kinds: Vec<String> = bridge_screen_core::store::read_all(store.conn(), None)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(!kinds.contains(&"checkpoint_due".to_string()), "{kinds:?}");
    assert!(!kinds.contains(&"decision_asked".to_string()), "{kinds:?}");
}

#[test]
fn init_plan_refuses_an_existing_plan() {
    let env = Env::new();
    env.hook("session-start", "s1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    assert!(!call_as(&mut mcp, &env, "s1", "init_plan", &draft()).0);
    let (err, text) = call_as(&mut mcp, &env, "s1", "init_plan", &draft());
    assert!(err);
    assert_eq!(text, "this repo already has a plan; use plan_edit");
}

#[test]
fn init_plan_needs_owner_words() {
    let env = Env::new();
    env.hook("session-start", "s1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let mut d = draft();
    d["owner_said"] = json!("  ");
    let (err, text) = call_as(&mut mcp, &env, "s1", "init_plan", &d);
    assert!(err);
    assert!(text.starts_with("owner_said is required"), "{text}");
    d.as_object_mut().unwrap().remove("owner_said");
    let (err, text) = call_as(&mut mcp, &env, "s1", "init_plan", &d);
    assert!(err);
    assert!(text.contains("owner_said"), "{text}");
    d["owner_said"] = json!("x".repeat(201));
    assert!(call_as(&mut mcp, &env, "s1", "init_plan", &d).0);
    let store = Store::open(&env.db_path()).unwrap();
    assert_eq!(
        load_tree(store.conn(), &repo_id(&env), true).unwrap().len(),
        0
    );
}

const ASK: &str =
    "\nIt has no estimate yet: set one in agent-minutes with plan_edit (set_estimate).";

#[test]
fn claim_without_estimate_asks_once() {
    let env = Env::new();
    env.hook("session-start", "s1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    assert!(!call_as(&mut mcp, &env, "s1", "init_plan", &draft()).0);
    let claim = |id: &str| json!({"id": id, "mode": "confirmed", "confidence": 1});
    let (_, text) = call_as(&mut mcp, &env, "s1", "claim_task", &claim("1.3"));
    assert_eq!(text, format!("Claimed 1.3 Profile.{ASK}"));
    let (_, text) = call_as(&mut mcp, &env, "s1", "claim_task", &claim("2"));
    assert!(text.ends_with(ASK), "{text}");
    // Back to 1.3: asked already.
    let (_, text) = call_as(&mut mcp, &env, "s1", "claim_task", &claim("1.3"));
    assert_eq!(text, "Claimed 1.3 Profile. Released 2.");
    // A task with an estimate is never asked about.
    let (_, text) = call_as(&mut mcp, &env, "s1", "claim_task", &claim("1.1"));
    assert!(!text.contains("estimate"), "{text}");
}
