//! Tracks through the binary (spec 2026-11-14 §2): starting, finishing, reopening, and the
//! owner-only rules around them.

mod common;

use bridge_screen_core::plan::track;
use bridge_screen_core::session;
use bridge_screen_core::store::Store;
use common::{Env, Mcp, call_as};
use serde_json::{Value, json};

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship it","intent":"The product ships"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Core","intent":"Core works"},
  {"op":"add_node","parent":"1","kind":"task","title":"Schema","estimate_min":30}
]"#;

fn setup() -> (Env, Mcp) {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    env.hook("session-start", "s1");
    let mcp = Mcp::start(&env, "/nonexistent");
    (env, mcp)
}

fn repo(env: &Env) -> String {
    let store = Store::open(&env.db_path()).unwrap();
    session::get(store.conn(), "s1").unwrap().unwrap().repo_id
}

fn open_tracks(env: &Env) -> Vec<(u32, String)> {
    let store = Store::open(&env.db_path()).unwrap();
    track::open_tracks(store.conn(), &repo(env))
        .unwrap()
        .into_iter()
        .map(|t| (t.no, t.title))
        .collect()
}

fn start(mcp: &mut Mcp, env: &Env, args: &Value) -> (bool, String) {
    call_as(mcp, env, "s1", "start_track", args)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn start_track_needs_owner_words_when_open() {
    let (env, mut mcp) = setup();
    let (err, text) = start(
        &mut mcp,
        &env,
        &json!({"title": "Docs refresh", "intent": "Docs are current"}),
    );
    assert!(err, "{text}");
    assert!(
        text.contains("track 1 'Ship it'") && text.contains("Ask the owner"),
        "{text}"
    );
    assert_eq!(open_tracks(&env).len(), 1);
    let (err, text) = start(
        &mut mcp,
        &env,
        &json!({"title": "Docs refresh", "intent": "Docs are current",
                "owner_said": "start a new one for the docs"}),
    );
    assert!(!err, "{text}");
    assert!(
        text.contains("track 2 'Docs refresh' (goal T2-G)") && text.contains("T2-1.1"),
        "{text}"
    );
    assert_eq!(open_tracks(&env).len(), 2);
}

#[test]
fn start_track_refused_for_blank_words() {
    let (env, mut mcp) = setup();
    let (err, text) = start(
        &mut mcp,
        &env,
        &json!({"title": "Docs", "intent": "Docs are current", "owner_said": "   "}),
    );
    assert!(err && text.contains("Ask the owner"), "{text}");
    assert_eq!(open_tracks(&env).len(), 1);
}

#[test]
fn start_track_free_when_none_open() {
    let env = Env::new();
    env.hook("session-start", "s1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (err, text) = start(
        &mut mcp,
        &env,
        &json!({"title": "First", "intent": "Something real"}),
    );
    assert!(!err, "{text}");
    assert!(text.contains("track 1 'First' (goal G)"), "{text}");
    // Finish it (empty tracks have nothing open), then a second needs no words either.
    assert!(env.run(&["track", "close", "1"], "").status.success());
    let (err, text) = start(
        &mut mcp,
        &env,
        &json!({"title": "Second", "intent": "Something else"}),
    );
    assert!(!err, "{text}");
    assert!(text.contains("track 2 'Second' (goal T2-G)"), "{text}");
}

#[test]
fn agent_cannot_start_track_with_plan_edit() {
    let (env, mut mcp) = setup();
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "plan_edit",
        &json!({"ops": [{"op": "add_node", "kind": "goal", "title": "Sneaky", "intent": "why"}],
                "reason": "new work"}),
    );
    assert!(err, "{text}");
    assert!(
        text.contains("start_track") && text.contains("track 1 'Ship it'"),
        "{text}"
    );
    assert_eq!(open_tracks(&env).len(), 1);
}

#[test]
fn cli_track_start_creates_goal() {
    let (env, _mcp) = setup();
    let out = env.run(
        &[
            "track",
            "start",
            "Docs refresh",
            "--intent",
            "Docs are current",
        ],
        "",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("Started track 2 'Docs refresh' (goal T2-G)"),
        "{}",
        stdout(&out)
    );
    let list = stdout(&env.run(&["track", "list"], ""));
    assert!(
        list.contains("1  Ship it") && list.contains("2  Docs refresh"),
        "{list}"
    );
    // The track's first milestone and task take the prefixed ids.
    let ops = r#"[{"op":"add_node","parent":"T2-G","kind":"milestone","title":"Pages","intent":"p"},
                  {"op":"add_node","parent":"T2-1","kind":"task","title":"Readme","estimate_min":10}]"#;
    let out = env.run(&["plan", "edit"], ops);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn close_track_refused_with_open_tasks_names_them() {
    let (env, _mcp) = setup();
    let out = env.run(&["track", "close", "1"], "");
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(
        text.contains("can't finish track 'Ship it' yet")
            && text.contains("1 of 1 tasks aren't done")
            && text.contains("1.1 Schema")
            && text.contains("remove it first")
            && text.contains("nothing was changed"),
        "{text}"
    );
    assert_eq!(open_tracks(&env).len(), 1);
}

#[test]
fn close_then_reopen_and_closed_track_leaves_open_list() {
    let env = Env::new();
    assert!(
        env.run(&["track", "start", "Empty one"], "")
            .status
            .success()
    );
    env.hook("session-start", "s1");
    assert_eq!(open_tracks(&env), vec![(1, "Empty one".to_string())]);
    let out = env.run(&["track", "close", "1"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(open_tracks(&env).len(), 0);
    assert!(stdout(&env.run(&["track", "list"], "")).contains("(finished)"));
    // Closing twice is a stale click.
    assert!(!env.run(&["track", "close", "1"], "").status.success());
    assert!(env.run(&["track", "reopen", "1"], "").status.success());
    assert_eq!(open_tracks(&env).len(), 1);
    assert!(!env.run(&["track", "reopen", "1"], "").status.success());
}

#[test]
fn close_run_needs_track_when_two_open() {
    let (env, _mcp) = setup();
    assert!(env.run(&["track", "start", "Docs"], "").status.success());
    let out = env.run(&["run", "close"], "");
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(
        text.contains("several tracks") && text.contains("track 2 'Docs'"),
        "{text}"
    );
    // With the track named, the usual refusal about unfinished tasks comes back.
    let out = env.run(&["run", "close", "--track", "1"], "");
    assert!(
        stderr(&out).contains("can't close this run yet"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn card_cannot_finish_or_start_tracks() {
    use bridge_screen_core::owner::OwnerAction;
    for a in [
        OwnerAction::CloseTrack {
            repo: "r".into(),
            track: 1,
        },
        OwnerAction::ReopenTrack {
            repo: "r".into(),
            track: 1,
        },
        OwnerAction::StartTrack {
            repo: "r".into(),
            title: "x".into(),
            intent: None,
        },
    ] {
        assert!(!a.allowed_on_card(), "{a:?}");
    }
    assert!(
        OwnerAction::CloseRun {
            repo: "r".into(),
            track: None
        }
        .allowed_on_card()
    );
}
