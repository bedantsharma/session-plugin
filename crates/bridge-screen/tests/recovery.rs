//! End classification, crash scan and leftover recovery through the hooks, the MCP
//! `resolve_leftover` tool and the `resolve` command (spec §8.2–8.3).

mod common;

use bridge_screen_core::handoff::{EndKind, LeftoverAction, LeftoverState, leftovers};
use bridge_screen_core::session;
use bridge_screen_core::store::{Event, EventDraft, Store, append_in_at, read_all};
use common::{Env, fixture_json, git};
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form validation","estimate_min":30},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset"}
]"#;

fn env() -> Env {
    let env = Env::new();
    for (k, v) in [
        ("user.name", "Test"),
        ("user.email", "t@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(env.repo.path(), &["config", k, v]);
    }
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    env
}

fn store(env: &Env) -> Store {
    Store::open(&env.db_path()).unwrap()
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

fn claim(env: &Env, session: &str, task: &str) {
    use bridge_screen_core::claim::{ClaimMode, ClaimRequest, ClaimTarget, claim};
    let mut s = store(env);
    let repo_id = session::get(s.conn(), session).unwrap().unwrap().repo_id;
    claim(
        &mut s,
        &ClaimRequest {
            session_id: session.into(),
            repo_id,
            task: ClaimTarget::Existing(task.into()),
            mode: ClaimMode::Auto,
            confidence: 0.9,
            actor: bridge_screen_core::store::Actor::default(),
        },
    )
    .unwrap();
}

/// `session` edits `file` (on disk too) in one unit.
fn edit(env: &Env, session: &str, file: &str, text: &str) {
    env.hook("user-prompt-submit", session);
    let path = env.repo.path().join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    let path = path.to_str().unwrap().to_string();
    hook_with(env, "post-tool-use", session, |p| {
        p["tool_input"]["file_path"] = json!(path);
    });
    env.hook("stop", session);
}

#[test]
fn session_end_is_dirty_or_clean() {
    let env = env();
    env.hook("session-start", "clean");
    env.hook("session-end", "clean");
    let ended = of_kind(&env, "session_ended");
    assert_eq!(ended[0]["end_kind"], "clean");

    env.hook("session-start", "unfinished");
    claim(&env, "unfinished", "1.1");
    env.hook("session-end", "unfinished");
    assert_eq!(of_kind(&env, "session_ended")[1]["end_kind"], "dirty");

    env.hook("session-start", "s1");
    edit(&env, "s1", "src/login.rs", "x\n");
    env.hook("session-end", "s1");
    let ended = &of_kind(&env, "session_ended")[2];
    assert_eq!(ended["end_kind"], "dirty");
    assert_eq!(ended["dirty_files"], json!(["src/login.rs"]));
    let s = session::get(store(&env).conn(), "s1").unwrap().unwrap();
    assert_eq!(s.end_kind, Some(EndKind::Dirty));
    assert_eq!(env.log(), "");
}

#[test]
fn session_start_scans_for_crashes() {
    let env = env();
    env.hook("session-start", "recent");
    env.hook("user-prompt-submit", "recent");
    // A session whose events all happened two hours ago.
    {
        let mut s = store(&env);
        let repo_id = session::get(s.conn(), "recent").unwrap().unwrap().repo_id;
        let then = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(2);
        s.write(|tx| {
            append_in_at(
                tx,
                EventDraft::new(
                    &repo_id,
                    "ancient",
                    "session_started",
                    json!({"cwd": env.repo_path(), "source": "startup"}),
                ),
                then,
            )?;
            append_in_at(
                tx,
                EventDraft::new(&repo_id, "ancient", "prompt", json!({"chars": 1})),
                then,
            )
        })
        .unwrap();
    }
    env.hook("session-start", "new");
    let crashed = events(&env)
        .into_iter()
        .filter(|e| e.kind == "session_crashed")
        .map(|e| e.session_id)
        .collect::<Vec<_>>();
    assert_eq!(crashed, vec!["ancient"]);
    let s = session::get(store(&env).conn(), "ancient")
        .unwrap()
        .unwrap();
    assert_eq!(s.end_kind, Some(EndKind::Crashed));
    assert_eq!(env.log(), "");
}

/// s1 claims 1.1, edits src/login.rs and ends dirty; s2 starts in the same worktree.
fn dead_then_new(env: &Env) -> String {
    env.hook("session-start", "s1");
    claim(env, "s1", "1.1");
    edit(env, "s1", "src/login.rs", "half done\n");
    edit(env, "s1", "notes.txt", "scratch\n");
    env.hook("session-end", "s1");
    env.hook("session-start", "s2");
    let found = leftovers(
        store(env).conn(),
        &session::get(store(env).conn(), "s2")
            .unwrap()
            .unwrap()
            .repo_id,
        true,
    )
    .unwrap();
    assert_eq!(found.len(), 2, "{found:?}");
    let login = found.iter().find(|l| l.path == "src/login.rs").unwrap();
    assert_eq!(login.task_id.as_deref(), Some("1.1"));
    assert_eq!(login.suggested, LeftoverAction::Resume);
    assert_eq!(login.state, LeftoverState::Open);
    login.leftover_id.clone()
}

#[test]
fn session_start_finds_leftovers_and_cli_resolves_them() {
    let env = env();
    let id = dead_then_new(&env);
    let out = env.run(&["resolve", &id, "adopt"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("Adopted 1 leftover: committed "), "{text}");
    let show = std::process::Command::new("git")
        .arg("-C")
        .arg(env.repo.path())
        .args(["show", "--name-only", "--format=%s", "HEAD"])
        .output()
        .unwrap();
    let show = String::from_utf8(show.stdout).unwrap();
    assert!(
        show.starts_with("wip(bridge): adopt leftovers from s1\n"),
        "{show}"
    );
    assert!(
        show.contains("src/login.rs") && !show.contains("notes.txt"),
        "{show}"
    );
    // Already resolved by the owner: a second try is refused.
    let again = env.run(&["resolve", &id, "undo"], "");
    assert!(!again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("already resolved by the owner"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(env.log(), "");
}

#[test]
fn cli_resolve_rejects_a_bad_action() {
    let env = env();
    let id = dead_then_new(&env);
    let out = env.run(&["resolve", &id, "delete"], "");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("adopt, resume or undo"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_crashed_session_running_only_bash_comes_back_on_its_next_tool_call() {
    let env = env();
    env.hook("session-start", "other");
    // `slow` last spoke two hours ago (a long build, say): the next start marks it crashed.
    {
        let mut s = store(&env);
        let repo_id = session::get(s.conn(), "other").unwrap().unwrap().repo_id;
        let then = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(2);
        s.write(|tx| {
            append_in_at(
                tx,
                EventDraft::new(
                    &repo_id,
                    "slow",
                    "session_started",
                    json!({"cwd": env.repo_path(), "source": "startup"}),
                ),
                then,
            )?;
            append_in_at(
                tx,
                EventDraft::new(&repo_id, "slow", "prompt", json!({"chars": 1})),
                then,
            )
        })
        .unwrap();
    }
    env.hook("session-start", "other2");
    let get = || session::get(store(&env).conn(), "slow").unwrap().unwrap();
    assert_eq!(get().end_kind, Some(EndKind::Crashed));
    // The build finishes: a Bash call, no edit.
    hook_with(&env, "post-tool-use", "slow", |p| {
        p["tool_name"] = json!("Bash");
        p["tool_input"] = json!({"command": "cargo build"});
    });
    let s = get();
    assert_eq!((s.end_kind, s.ended_at), (None, None));
    assert_ne!(s.state, session::SessionState::Ended);
    assert_eq!(env.log(), "");
}
