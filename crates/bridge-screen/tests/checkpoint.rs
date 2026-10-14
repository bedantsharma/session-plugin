//! Checkpoint enforcement through the hooks (spec §8.1): boundaries make a checkpoint due,
//! Stop asks for the commit once, commits are detected, and misses are flagged.

mod common;

use bridge_screen_core::checkpoint::{self, ClaimDoneRequest};
use bridge_screen_core::session::{self, Session};
use bridge_screen_core::store::{Actor, Event, Store, read_all};
use bridge_screen_core::tangent::{self, Label, LabelSource};
use common::{Env, fixture_json, git};
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form validation","estimate_min":30,
   "expected_files":["src/login.rs"]},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset"}
]"#;

const MIDDLE: &str = "Refactored helper scripts.";

const ASK: &str = "bridge-screen: checkpoint before stopping — commit as `wip(bridge): 1.1 Login form validation` with trailers Bridge-Task: 1.1, Bridge-Next: <one-line next action>, Bridge-Session: s1.";

fn setup() -> Env {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    env.hook("session-start", "s1");
    claim(&env, "s1", "1.1");
    env
}

fn store(env: &Env) -> Store {
    Store::open(&env.db_path()).unwrap()
}

fn claim(env: &Env, session: &str, task: &str) {
    use bridge_screen_core::claim::{ClaimMode, ClaimRequest, ClaimTarget, claim};
    let mut s = store(env);
    let repo_id = sess(env, session).repo_id;
    claim(
        &mut s,
        &ClaimRequest {
            session_id: session.into(),
            repo_id,
            task: ClaimTarget::Existing(task.into()),
            mode: ClaimMode::Auto,
            confidence: 0.9,
            actor: Actor::default(),
        },
    )
    .unwrap();
}

fn claim_done(env: &Env, session: &str) {
    checkpoint::claim_done(
        &mut store(env),
        &ClaimDoneRequest {
            session_id: session.into(),
            task_id: None,
            evidence: "tests pass".into(),
            files: Vec::new(),
            symbols: Vec::new(),
            actor: Actor::default(),
        },
    )
    .unwrap();
}

fn sess(env: &Env, id: &str) -> Session {
    session::get(store(env).conn(), id).unwrap().unwrap()
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

fn hook_with(env: &Env, event: &str, session: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut payload = fixture_json(event, session, &env.repo_path());
    edit(&mut payload);
    env.hook_raw(event, &payload.to_string())
}

/// A prompt and edits to `files` (written to disk, so the worktree is dirty).
fn work(env: &Env, session: &str, files: &[&str]) {
    env.hook("user-prompt-submit", session);
    for f in files {
        let path = env.repo.path().join(f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "changed\n").unwrap();
        let path = path.to_str().unwrap().to_string();
        hook_with(env, "post-tool-use", session, |p| {
            p["tool_input"]["file_path"] = json!(path);
        });
    }
}

fn stop(env: &Env, session: &str, active: bool) -> String {
    hook_with(env, "stop", session, |p| {
        p["last_assistant_message"] = json!(MIDDLE);
        p["stop_hook_active"] = json!(active);
    })
}

fn bash(env: &Env, session: &str, command: &str) -> String {
    hook_with(env, "post-tool-use", session, |p| {
        p["tool_name"] = json!("Bash");
        p["tool_input"] = json!({"command": command});
        p["tool_response"] = json!({"stdout": "", "stderr": ""});
    })
}

fn context_of(out: &str, event: &str) -> String {
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], event);
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
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

#[test]
fn dirty_stop_after_claim_done_asks_for_the_checkpoint_before_a_label() {
    let env = setup();
    work(&env, "s1", &["src/login.rs", "tools/gen.py"]);
    claim_done(&env, "s1");
    assert_eq!(
        sess(&env, "s1").checkpoint_pending.as_deref(),
        Some("claimed_done")
    );
    // This unit would get a label request on its own; the checkpoint outranks it.
    assert_eq!(context_of(&stop(&env, "s1", false), "Stop"), ASK);
    let notes = of_kind(&env, "note_sent");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["kind"], "checkpoint");
    assert_eq!(notes[0]["channel"], "stop");
    let u = &session::units(store(&env).conn(), "s1").unwrap()[0];
    assert_eq!(
        (u.label, u.label_source),
        (Some(Label::OnTask), Some(LabelSource::Heuristic))
    );
    assert_eq!(sess(&env, "s1").checkpoint_asked_unit, Some(1));
    assert_eq!(env.log(), "");
}

#[test]
fn trailer_commit_is_recorded_and_clears_the_checkpoint() {
    let env = setup();
    work(&env, "s1", &["src/login.rs"]);
    claim_done(&env, "s1");
    assert_eq!(context_of(&stop(&env, "s1", false), "Stop"), ASK);
    git(env.repo.path(), &["add", "-A"]);
    git(
        env.repo.path(),
        &[
            "commit",
            "-q",
            "-m",
            "wip(bridge): 1.1 Login form validation",
            "-m",
            "Bridge-Task: 1.1\nBridge-Next: add the empty-password test\nBridge-Session: s1",
        ],
    );
    assert_eq!(
        bash(
            &env,
            "s1",
            "git add -A && git commit -m 'wip(bridge): 1.1 …'"
        ),
        ""
    );
    let cps = of_kind(&env, "checkpoint");
    assert_eq!(cps.len(), 1);
    assert_eq!(cps[0]["sha"], head(&env));
    assert_eq!(cps[0]["task_id"], "1.1");
    assert_eq!(cps[0]["next_action"], "add the empty-password test");
    let s = sess(&env, "s1");
    assert_eq!(
        (s.checkpoint_pending, s.last_checkpoint),
        (None, Some(head(&env)))
    );
    // The continuation turn's Stop has nothing more to ask.
    assert_eq!(stop(&env, "s1", true), "");
    assert_eq!(of_kind(&env, "checkpoint_missed"), Vec::<Value>::new());
    assert_eq!(env.log(), "");
}

#[test]
fn plain_commit_while_pending_also_clears_it() {
    let env = setup();
    work(&env, "s1", &["src/login.rs"]);
    claim_done(&env, "s1");
    git(env.repo.path(), &["add", "-A"]);
    // Commit times are whole seconds: a commit in the boundary's own second can't be told
    // from one made just before it, so this one is clearly later.
    let later = (jiff::Timestamp::now() + jiff::SignedDuration::from_secs(2)).as_second();
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(env.repo.path())
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["commit", "-q", "-m", "login validation"])
        .env("GIT_COMMITTER_DATE", format!("@{later} +0000"))
        .status()
        .unwrap();
    assert!(ok.success());
    bash(&env, "s1", "git commit -am 'login validation'");
    let cps = of_kind(&env, "checkpoint");
    assert_eq!(cps.len(), 1);
    assert_eq!(cps[0]["next_action"], Value::Null);
    assert_eq!(sess(&env, "s1").checkpoint_pending, None);
    // Stop has no checkpoint to ask for (the unit's own label request may still go out).
    assert!(!stop(&env, "s1", false).contains("checkpoint"));
}

#[test]
fn ignored_checkpoint_is_flagged_not_repeated() {
    let env = setup();
    work(&env, "s1", &["src/login.rs"]);
    claim_done(&env, "s1");
    assert_eq!(context_of(&stop(&env, "s1", false), "Stop"), ASK);
    // The agent stops again without committing: no second request, one miss.
    assert_eq!(stop(&env, "s1", true), "");
    assert_eq!(stop(&env, "s1", false), "");
    assert_eq!(
        of_kind(&env, "checkpoint_missed"),
        vec![json!({"unit_no": 1})]
    );
    let notes = of_kind(&env, "note_sent");
    assert_eq!(notes.len(), 1, "{notes:?}");
    let s = sess(&env, "s1");
    assert_eq!(s.checkpoint_missed, 1);
    assert_eq!(s.checkpoint_pending.as_deref(), Some("claimed_done"));
    assert_eq!(env.log(), "");
}

#[test]
fn stop_hook_active_never_asks() {
    let env = setup();
    work(&env, "s1", &["src/login.rs"]);
    claim_done(&env, "s1");
    assert_eq!(stop(&env, "s1", true), "");
    assert_eq!(
        of_kind(&env, "checkpoint_missed"),
        vec![json!({"unit_no": 1})]
    );
}

#[test]
fn clean_worktree_clears_pending_silently() {
    let env = setup();
    env.hook("user-prompt-submit", "s1");
    claim_done(&env, "s1");
    assert!(!stop(&env, "s1", false).contains("checkpoint"));
    let cps = of_kind(&env, "checkpoint");
    assert_eq!(
        cps,
        vec![json!({"sha": head(&env), "task_id": "1.1", "next_action": null})]
    );
    assert_eq!(sess(&env, "s1").checkpoint_pending, None);
}

#[test]
fn detour_close_and_task_switch_make_a_checkpoint_due() {
    let env = setup();
    env.hook("user-prompt-submit", "s1");
    let started = tangent::StartRequest {
        session_id: "s1".into(),
        reason: "CI cache is broken".into(),
        resume_cue: "rerun the login test".into(),
        blocks: None,
        actor: Actor::default(),
    };
    tangent::start_detour(&mut store(&env), &started).unwrap();
    assert_eq!(of_kind(&env, "checkpoint_due"), Vec::<Value>::new());
    tangent::close_detour(
        &mut store(&env),
        &tangent::CloseRequest {
            session_id: "s1".into(),
            detour: None,
            outcome: tangent::DetourOutcome::Done,
            note: None,
            actor: Actor::default(),
        },
    )
    .unwrap();
    assert_eq!(
        of_kind(&env, "checkpoint_due"),
        vec![json!({"reason": "detour_closed"})]
    );

    env.hook("session-start", "s2");
    claim(&env, "s2", "1.2");
    assert_eq!(sess(&env, "s2").checkpoint_pending, None, "first claim");
    claim(&env, "s2", "1.1");
    assert_eq!(
        sess(&env, "s2").checkpoint_pending.as_deref(),
        Some("task_switched")
    );
}

#[cfg(unix)]
#[test]
fn no_pending_checkpoint_means_no_git() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut env = setup();
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
    env.hook("user-prompt-submit", "s1");
    env.hook("post-tool-use", "s1");
    bash(&env, "s1", "cargo test");
    env.hook("stop", "s1");
    assert_eq!(env.log(), "");
}
