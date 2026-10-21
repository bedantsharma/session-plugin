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
