//! Dependencies across repos (spec §9.1): plan links to a task in another repo, and
//! ad-hoc waits (`waiting_on`) that clear when their target is done.

mod common;

use std::path::{Path, PathBuf};

use common::{Env, Mcp, call_as, fixture, git};
use serde_json::{Value, json};

const SEED_WEB: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Web","intent":"People use it"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Pages","intent":"Pages work"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login page","estimate_min":30}
]"#;

const SEED_API: &str = r#"[
  {"op":"add_node","kind":"goal","title":"API","intent":"Clients call it"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Auth","intent":"Sign in works"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login endpoint","estimate_min":30}
]"#;

/// Two repos sharing one bridge-screen home: `env.repo` (web) and `api`.
struct Two {
    env: Env,
    _api_dir: tempfile::TempDir,
    api: PathBuf,
    mcp: Mcp,
}

fn configure(dir: &Path) {
    for (k, v) in [
        ("user.name", "Test"),
        ("user.email", "t@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(dir, &["config", k, v]);
    }
}

impl Two {
    fn new() -> Self {
        let env = Env::new();
        configure(env.repo.path());
        let api_dir = tempfile::tempdir().unwrap();
        let api = api_dir.path().join("api");
        std::fs::create_dir_all(&api).unwrap();
        git(&api, &["init", "-q"]);
        configure(&api);
        git(&api, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let api = std::fs::canonicalize(api).unwrap();
        assert!(env.run(&["plan", "edit"], SEED_WEB).status.success());
        let out = env.run_in(&["plan", "edit"], SEED_API, &api);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        env.hook("session-start", "w1");
        let two = Self {
            mcp: Mcp::start(&env, "/nonexistent"),
            env,
            _api_dir: api_dir,
            api,
        };
        two.api_hook("session-start", "a1");
        two
    }

    fn api_hook(&self, event: &str, session: &str) -> String {
        let payload = fixture(event, session, self.api.to_str().unwrap());
        let out = self.env.run_in(&["hook", event], &payload, &self.api);
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }

    fn call(&mut self, session: &str, tool: &str, args: &Value) -> (bool, String) {
        call_as(&mut self.mcp, &self.env, session, tool, args)
    }

    fn claim(&mut self, session: &str, id: &str) {
        let (err, text) = self.call(
            session,
            "claim_task",
            &json!({"id": id, "mode": "confirmed", "confidence": 1}),
        );
        assert!(!err, "{text}");
    }

    /// The api session finishes its 1.1 and verifies it at L2; returns the short sha.
    fn finish_api_task(&mut self) -> String {
        std::fs::create_dir_all(self.api.join("src")).unwrap();
        std::fs::write(self.api.join("src/login.rs"), "pub fn login() {}\n").unwrap();
        git(&self.api, &["add", "-A"]);
        git(&self.api, &["commit", "-q", "-m", "login"]);
        for (tool, args) in [
            (
                "claim_done",
                json!({"evidence": "done", "files": ["src/login.rs"]}),
            ),
            (
                "verify",
                json!({"level": 2, "evidence": "cargo test", "exit_code": 0, "summary": "3 passed"}),
            ),
        ] {
            let (err, text) = self.call("a1", tool, &args);
            assert!(!err, "{tool}: {text}");
        }
        let out = common::git_cmd(&self.api)
            .args(["rev-parse", "--short=7", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn prompt_web(&self, session: &str) -> String {
        context(&self.env.hook("user-prompt-submit", session))
    }
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
fn external_dep_ready_note() {
    let mut t = Two::new();
    let link = r#"[{"op":"add_dep","id":"1.1","depends_on":"1.1","repo":"api"}]"#;
    let out = t
        .env
        .run(&["plan", "edit", "--reason", "needs the endpoint"], link);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    t.claim("w1", "1.1");
    t.prompt_web("w1");
    t.claim("a1", "1.1");
    t.api_hook("user-prompt-submit", "a1");
    // Before: the briefing says it is waiting.
    let brief = context(&t.env.hook_raw("session-start", &{
        let mut p: Value =
            serde_json::from_str(&fixture("session-start", "w1", &t.env.repo_path())).unwrap();
        p["source"] = json!("resume");
        p.to_string()
    }));
    assert!(brief.contains("Dependencies: api:1.1 waiting"), "{brief}");
    let short = t.finish_api_task();
    assert_eq!(
        t.prompt_web("w1"),
        format!(
            "bridge-screen: api:1.1 Login endpoint is done on `main` @{short}. Merge or rebase to pick it up."
        )
    );
    for _ in 0..3 {
        assert_eq!(t.prompt_web("w1"), "");
    }
    let brief = context(&t.env.hook_raw("session-start", &{
        let mut p: Value =
            serde_json::from_str(&fixture("session-start", "w1", &t.env.repo_path())).unwrap();
        p["source"] = json!("resume");
        p.to_string()
    }));
    assert!(brief.contains("Dependencies: api:1.1 ready"), "{brief}");
}

fn waits_dump(env: &Env) -> Vec<String> {
    let store = bridge_screen_core::store::Store::open(&env.db_path()).unwrap();
    let mut stmt = store
        .conn()
        .prepare(
            "SELECT wait_id||'|'||session_id||'|'||target_text||'|'||coalesce(cleared_why,'open')
                    ||'|'||coalesce(cleared_sha,'')||'|'||coalesce(cleared_at,'')
             FROM waits ORDER BY wait_id",
        )
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn wait_clears_once_and_notes_once() {
    let mut t = Two::new();
    t.claim("w1", "1.1");
    t.prompt_web("w1");
    // A waiter in the other repo (web) and one in the same repo (api).
    let (err, text) = t.call(
        "w1",
        "waiting_on",
        &json!({"target": "api:1.1", "reason": "the page calls it"}),
    );
    assert!(!err, "{text}");
    assert_eq!(
        text,
        "Waiting on api:1.1 Login endpoint (W1). You'll get a note here when it's done. If your task will always need it, ask the owner whether to make it a plan link (promote_wait)."
    );
    t.api_hook("session-start", "a2");
    let (err, text) = t.call(
        "a2",
        "waiting_on",
        &json!({"target": "1.1", "reason": "docs need it"}),
    );
    assert!(!err, "{text}");
    // Waiting shows in the agent's status.
    let (_, st) = t.call("w1", "status", &json!({}));
    assert!(
        st.contains("Waiting on: W1 api:1.1 Login endpoint (since 0m)"),
        "{st}"
    );

    t.claim("a1", "1.1");
    t.api_hook("user-prompt-submit", "a1");
    let short = t.finish_api_task();
    let dump = waits_dump(&t.env);
    assert_eq!(dump.len(), 2);
    assert!(dump.iter().all(|w| w.contains("|target_done|")), "{dump:?}");
    // The web session hears once; the plan-link note doesn't duplicate it (no link).
    assert_eq!(
        t.prompt_web("w1"),
        format!(
            "bridge-screen: api:1.1 Login endpoint is done on `main` @{short}. Merge or rebase to pick it up."
        )
    );
    for _ in 0..3 {
        assert_eq!(t.prompt_web("w1"), "");
    }
    let a2 = context(&t.api_hook("user-prompt-submit", "a2"));
    assert!(a2.contains("1.1 Login endpoint is done"), "{a2}");
    // A later change that drops the verification as stale never reopens the wait.
    std::fs::write(t.api.join("src/login.rs"), "pub fn login() { /* v2 */ }\n").unwrap();
    git(&t.api, &["commit", "-qam", "v2"]);
    t.api_hook("session-start", "a3");
    assert_eq!(waits_dump(&t.env), dump);
    // Rebuild gives the same waits.
    assert!(t.env.run(&["rebuild"], "").status.success());
    assert_eq!(waits_dump(&t.env), dump);
}

#[test]
fn wait_tools_contract() {
    let mut t = Two::new();
    t.claim("w1", "1.1");
    let (err, text) = t.call(
        "w1",
        "waiting_on",
        &json!({"target": "Sam's API key", "reason": "for staging"}),
    );
    assert!(!err, "{text}");
    assert_eq!(
        text,
        "Waiting on Sam's API key (W1). Clear it with clear_wait when it arrives."
    );
    // Bad input is a tool error the agent can read.
    let (err, text) = t.call("w1", "waiting_on", &json!({"target": "1.1"}));
    assert!(err, "{text}");
    let (err, text) = t.call(
        "w1",
        "waiting_on",
        &json!({"target": "nope:2", "reason": "x"}),
    );
    assert!(err);
    assert_eq!(text, "no repo named nope");
    // Another session's wait can't be cleared or promoted by it.
    let (err, text) = t.call("a1", "clear_wait", &json!({"wait_id": "W1"}));
    assert!(err);
    assert!(text.contains("another session's wait"), "{text}");
    let (err, text) = t.call("w1", "promote_wait", &json!({"wait_id": "W1"}));
    assert!(err);
    assert!(
        text.contains("only a task can become a plan link"),
        "{text}"
    );
    let (err, text) = t.call("w1", "clear_wait", &json!({"wait_id": "W1"}));
    assert!(!err, "{text}");
    assert_eq!(text, "Cleared W1: no longer waiting on Sam's API key.");
    let (err, text) = t.call("w1", "clear_wait", &json!({"wait_id": "W1"}));
    assert!(err);
    assert_eq!(text, "W1 is already cleared");
    // A task wait promoted by the agent becomes a plan link.
    let (err, _) = t.call(
        "w1",
        "waiting_on",
        &json!({"target": "api:1.1", "reason": "the page calls it"}),
    );
    assert!(!err);
    let (err, text) = t.call("w1", "promote_wait", &json!({"wait_id": "W2"}));
    assert!(!err, "{text}");
    assert_eq!(text, "1.1 now depends on api:1.1; the wait W2 is closed.");
}

#[test]
fn owner_lists_clears_and_promotes_waits() {
    let mut t = Two::new();
    t.claim("w1", "1.1");
    for target in ["api:1.1", "the design doc"] {
        let (err, text) = t.call(
            "w1",
            "waiting_on",
            &json!({"target": target, "reason": "needed"}),
        );
        assert!(!err, "{text}");
    }
    let list = String::from_utf8(t.env.run(&["wait", "list"], "").stdout).unwrap();
    assert!(
        list.contains("W1  w1  waiting on api:1.1 Login endpoint — needed (open for 0m)"),
        "{list}"
    );
    let out = t.env.run(&["wait", "clear", "w2"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "Cleared W2: w1 no longer waits on the design doc.\n"
    );
    // A second click is stale and records nothing.
    let out = t.env.run(&["wait", "clear", "W2"], "");
    assert!(!out.status.success());
    let out = t.env.run(&["wait", "promote", "W1"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "1.1 now depends on api:1.1; the wait W1 is closed.\n"
    );
    let list = String::from_utf8(t.env.run(&["wait", "list"], "").stdout).unwrap();
    assert!(
        list.contains("(now a plan link)") && list.contains("(cleared by hand)"),
        "{list}"
    );
}

#[test]
fn a_link_and_a_wait_on_the_same_task_note_once() {
    let mut t = Two::new();
    let link = r#"[{"op":"add_dep","id":"1.1","depends_on":"1.1","repo":"api"}]"#;
    assert!(
        t.env
            .run(&["plan", "edit", "--reason", "needs it"], link)
            .status
            .success()
    );
    t.claim("w1", "1.1");
    t.prompt_web("w1");
    let (err, text) = t.call(
        "w1",
        "waiting_on",
        &json!({"target": "api:1.1", "reason": "the page calls it"}),
    );
    assert!(!err, "{text}");
    // Already a plan link: no offer to make one.
    assert!(!text.contains("promote_wait"), "{text}");
    t.claim("a1", "1.1");
    t.api_hook("user-prompt-submit", "a1");
    let short = t.finish_api_task();
    let first = t.prompt_web("w1");
    assert_eq!(
        first,
        format!(
            "bridge-screen: api:1.1 Login endpoint is done on `main` @{short}. Merge or rebase to pick it up."
        )
    );
    for _ in 0..3 {
        assert_eq!(t.prompt_web("w1"), "", "the same news twice");
    }
}

#[test]
fn two_waits_on_the_same_task_note_once() {
    let mut t = Two::new();
    t.claim("w1", "1.1");
    t.prompt_web("w1");
    for why in ["the page calls it", "and the docs"] {
        let (err, text) = t.call(
            "w1",
            "waiting_on",
            &json!({"target": "api:1.1", "reason": why}),
        );
        assert!(!err, "{text}");
    }
    t.claim("a1", "1.1");
    t.api_hook("user-prompt-submit", "a1");
    let short = t.finish_api_task();
    assert_eq!(
        t.prompt_web("w1"),
        format!(
            "bridge-screen: api:1.1 Login endpoint is done on `main` @{short}. Merge or rebase to pick it up."
        )
    );
    for _ in 0..3 {
        assert_eq!(t.prompt_web("w1"), "", "the same news twice");
    }
}
