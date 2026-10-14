//! Owner actions through `bridge_screen_core::owner::act` — the one path the CLI, the
//! dashboard, `watch` and the desktop card share: the optional Continue reason, the
//! events each action records, and stale or double clicks.

mod common;

use bridge_screen_core::config::Config;
use bridge_screen_core::error::Error;
use bridge_screen_core::handoff::{LeftoverAction, leftovers};
use bridge_screen_core::owner::{ActCtx, DriftAnswer, OwnerAction, act};
use bridge_screen_core::paths::Home;
use bridge_screen_core::session;
use bridge_screen_core::store::{Event, EventDraft, Store, append_in_at, read_all};
use common::{Env, fixture_json, git};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form validation","estimate_min":30,
   "expected_files":["src/login.rs"]},
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

fn repo_id(env: &Env) -> String {
    bridge_screen_core::repo::identify(env.repo.path())
        .unwrap()
        .id
}

fn claim(env: &Env, session: &str, task: &str) {
    use bridge_screen_core::claim::{ClaimMode, ClaimRequest, ClaimTarget, claim};
    let mut s = store(env);
    claim(
        &mut s,
        &ClaimRequest {
            session_id: session.into(),
            repo_id: repo_id(env),
            task: ClaimTarget::Existing(task.into()),
            mode: ClaimMode::Auto,
            confidence: 0.9,
            actor: bridge_screen_core::store::Actor::default(),
        },
    )
    .unwrap();
}

/// A session 40 minutes into a detour from 1.1: far outside tolerance, red.
fn far_off(env: &Env, session: &str) {
    env.hook("session-start", session);
    claim(env, session, "1.1");
    let mut s = store(env);
    let now = Timestamp::now();
    let ago = |min: i64| now - SignedDuration::from_mins(min);
    let script = [
        ("prompt", json!({"chars": 5}), ago(40)),
        (
            "detour_opened",
            json!({"detour_id": format!("D-{session}"), "reason": "CI cache is broken",
                   "blocks": "1.1", "resume_cue": "rerun the login test"}),
            ago(40),
        ),
        ("unit_stopped", json!({"unit_no": 1}), ago(5)),
        (
            "unit_labeled",
            json!({"unit_no": 1, "label": "detour", "source": "declared"}),
            ago(5),
        ),
    ];
    let repo = repo_id(env);
    s.write(|tx| {
        for (kind, data, ts) in script {
            append_in_at(tx, EventDraft::new(&repo, session, kind, data), ts)?;
        }
        Ok(())
    })
    .unwrap();
    drop(s);
    // A tool call records the red state.
    let mut p = fixture_json("post-tool-use", session, &env.repo_path());
    p["tool_name"] = json!("Bash");
    p["tool_input"] = json!({"command": "cargo test"});
    env.hook_raw("post-tool-use", &p.to_string());
}

fn cli(env: &Env, args: &[&str]) -> (bool, String) {
    let out = env.run(args, "");
    let text = if out.status.success() {
        String::from_utf8(out.stdout).unwrap()
    } else {
        String::from_utf8(out.stderr).unwrap()
    };
    (out.status.success(), text)
}

/// `owner::act` as the dashboard calls it (no `-C` directory).
fn act_on(env: &Env, action: &OwnerAction) -> Result<String, Error> {
    let home = Home::at(env.home.path());
    let cfg = Config::default();
    let ctx = ActCtx {
        home: &home,
        cfg: &cfg,
        now: Timestamp::now(),
        dir: None,
    };
    act(&mut store(env), &ctx, action)
}

/// The kinds and data of events appended after `before` (ids and times left out).
fn since(env: &Env, before: usize) -> Vec<(String, Value)> {
    events(env)
        .into_iter()
        .skip(before)
        .map(|e| (e.kind, e.data))
        .collect()
}

fn drift(session: &str, answer: DriftAnswer, reason: Option<&str>) -> OwnerAction {
    OwnerAction::Drift {
        session: session.into(),
        answer,
        parent: None,
        title: None,
        estimate: None,
        reason: reason.map(Into::into),
    }
}

fn context_of(out: &str) -> String {
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn continue_without_a_reason_is_one_click() {
    let env = env();
    far_off(&env, "s1aaaa");
    let before = events(&env).len();
    let reply = act_on(&env, &drift("s1", DriftAnswer::Continue, None)).unwrap();
    assert_eq!(reply, "Told s1aaaa: continue.");
    let new = since(&env, before);
    assert_eq!(
        new[0],
        (
            "owner_action".to_string(),
            json!({"action": "drift", "answer": "continue", "target": "s1aaaa"})
        )
    );
    // Continue acknowledges the away time: back to green.
    assert_eq!(new[1].0, "nav_state_changed");
    assert_eq!(new[1].1["to"], "green");
    assert_eq!(new.len(), 2);
}

#[test]
fn a_valid_reason_is_stored_and_reaches_the_agent() {
    let env = env();
    far_off(&env, "s1aaaa");
    let (ok, text) = cli(
        &env,
        &[
            "drift",
            "s1",
            "continue",
            "--reason",
            "the CI fix unblocks the release",
        ],
    );
    assert!(ok, "{text}");
    assert_eq!(
        text,
        "Told s1aaaa: continue — \"the CI fix unblocks the release\".\n"
    );
    let action = events(&env)
        .into_iter()
        .find(|e| e.kind == "owner_action")
        .unwrap();
    assert_eq!(action.data["reason"], "the CI fix unblocks the release");
    let out = env.hook("user-prompt-submit", "s1aaaa");
    assert_eq!(
        context_of(&out),
        "bridge-screen: the owner says continue: the CI fix unblocks the release."
    );
}

#[test]
fn a_reason_that_says_nothing_is_refused_and_nothing_recorded() {
    let env = env();
    far_off(&env, "s1aaaa");
    let before = events(&env).len();
    for bad in ["ok", "  ", "fine.", "sure go"] {
        match act_on(&env, &drift("s1", DriftAnswer::Continue, Some(bad))) {
            // Blank is no reason at all: that one goes through.
            Ok(_) if bad.trim().is_empty() => {
                assert_eq!(events(&env)[before].kind, "owner_action");
                return;
            }
            Err(Error::Invalid(hint)) => {
                assert!(hint.contains("at least 8 characters"), "{hint}");
                assert_eq!(events(&env).len(), before, "{bad}");
            }
            other => panic!("{bad}: {other:?}"),
        }
    }
    unreachable!("the blank reason is accepted");
}

#[test]
fn each_action_records_what_its_cli_path_recorded() {
    let env = env();
    far_off(&env, "s1aaaa");
    // Stop: the answer, nothing else (the session stays red until it returns).
    let before = events(&env).len();
    act_on(&env, &drift("s1aaaa", DriftAnswer::Stop, None)).unwrap();
    assert_eq!(
        since(&env, before),
        vec![(
            "owner_action".to_string(),
            json!({"action": "drift", "answer": "stop", "target": "s1aaaa"})
        )]
    );
    // Make it a task: the promotion's events, then the answer.
    far_off(&env, "s2bbbb");
    let before = events(&env).len();
    let reply = act_on(
        &env,
        &OwnerAction::Drift {
            session: "s2bbbb".into(),
            answer: DriftAnswer::MakeTask,
            parent: Some("1".into()),
            title: Some("Fix the CI cache".into()),
            estimate: None,
            reason: None,
        },
    )
    .unwrap();
    assert_eq!(reply, "Detour D1 is now task 1.3, claimed by s2bbbb.");
    let kinds: Vec<String> = since(&env, before).into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        kinds,
        vec![
            "node_added",
            "task_released",
            "checkpoint_due",
            "task_claimed",
            "detour_promoted",
            "note_sent",
            "owner_action",
        ]
    );
    // Label: the label (and its owner sample), then the owner action.
    let before = events(&env).len();
    act_on(
        &env,
        &OwnerAction::Label {
            session: "s2bbbb".into(),
            unit_no: 1,
            label: bridge_screen_core::tangent::Label::Drift,
        },
    )
    .unwrap();
    let new = since(&env, before);
    assert_eq!(new[0].0, "unit_labeled");
    assert_eq!(new[0].1["source"], "owner");
    assert_eq!(
        new[1],
        (
            "owner_action".to_string(),
            json!({"action": "label", "answer": "drift", "target": "s2bbbb:1"})
        )
    );
}

/// A dead session's leftover in the repo; returns its id.
fn leftover(env: &Env) -> String {
    env.hook("session-start", "s0dead");
    env.hook("user-prompt-submit", "s0dead");
    let path = env.repo.path().join("src/login.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "x\n").unwrap();
    let mut p = fixture_json("post-tool-use", "s0dead", &env.repo_path());
    p["tool_input"]["file_path"] = json!(path.to_str().unwrap());
    env.hook_raw("post-tool-use", &p.to_string());
    env.hook("stop", "s0dead");
    env.hook("session-end", "s0dead");
    env.hook("session-start", "s3next");
    let open = leftovers(store(env).conn(), &repo_id(env), true).unwrap();
    assert_eq!(open.len(), 1, "{open:?}");
    // Its worktree is free again: the owner may act on it.
    env.hook("session-end", "s3next");
    open[0].leftover_id.clone()
}

#[test]
fn stale_owner_action_records_nothing() {
    let env = env();
    // A leftover resolved by the CLI, then clicked on the dashboard.
    let id = leftover(&env);
    let (ok, text) = cli(&env, &["resolve", &id, "undo"]);
    assert!(ok, "{text}");
    let before = events(&env).len();
    let click = act_on(
        &env,
        &OwnerAction::Resolve {
            repo: None,
            leftover_ids: vec![id.clone()],
            action: LeftoverAction::Adopt,
        },
    );
    assert!(
        matches!(&click, Err(Error::Stale(m)) if m.contains("already resolved by the owner")),
        "{click:?}"
    );
    assert_eq!(events(&env).len(), before);

    // A drift answered twice (a second tab, or the CLI in another terminal).
    far_off(&env, "s1aaaa");
    act_on(&env, &drift("s1", DriftAnswer::Stop, None)).unwrap();
    let before = events(&env).len();
    let again = act_on(&env, &drift("s1", DriftAnswer::Stop, None));
    assert!(
        matches!(&again, Err(Error::Stale(m)) if m.starts_with("already resolved")),
        "{again:?}"
    );
    assert_eq!(events(&env).len(), before);

    // A verification after the task is already L4 at HEAD.
    let (ok, text) = cli(&env, &["verify", "1.2"]);
    assert!(ok, "{text}");
    let before = events(&env).len();
    let twice = act_on(
        &env,
        &OwnerAction::Verify {
            repo: repo_id(&env),
            task: "1.2".into(),
            level: 4,
        },
    );
    assert!(
        matches!(&twice, Err(Error::Stale(m)) if m.contains("already verified L4")),
        "{twice:?}"
    );
    assert_eq!(events(&env).len(), before);
    assert!(
        session::get(store(&env).conn(), "s1aaaa")
            .unwrap()
            .is_some()
    );
    assert_eq!(env.log(), "");
}
