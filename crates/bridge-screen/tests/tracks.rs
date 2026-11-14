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

// ---- every reader picks a track (spec 2026-11-14 §2, Task 7) ----

fn context_text(out: &str) -> String {
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn two_track_env() -> Env {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    assert!(
        env.run(
            &[
                "track",
                "start",
                "Docs refresh",
                "--intent",
                "Docs are current"
            ],
            ""
        )
        .status
        .success()
    );
    let ops = r#"[{"op":"add_node","parent":"T2-G","kind":"milestone","title":"Pages","intent":"p"},
                  {"op":"add_node","parent":"T2-1","kind":"task","title":"Readme rewrite","estimate_min":10}]"#;
    assert!(env.run(&["plan", "edit"], ops).status.success());
    env
}

fn prompt(env: &Env, session: &str, text: &str) -> String {
    let mut p = common::fixture_json("user-prompt-submit", session, &env.repo_path());
    p["prompt"] = json!(text);
    env.hook_raw("user-prompt-submit", &p.to_string())
}

#[test]
fn briefing_lists_open_tracks_and_asks() {
    let env = two_track_env();
    let text = context_text(&env.hook("session-start", "s1"));
    assert!(text.contains("Tracks (2 open):"), "{text}");
    assert!(
        text.contains("1 Ship it — 0%") && text.contains("2 Docs refresh — 0%"),
        "{text}"
    );
    assert!(
        text.contains("ask the owner: continue a track, or start a new one (start_track)"),
        "{text}"
    );
    assert!(text.lines().count() <= 40, "{text}");
}

#[test]
fn briefing_unchanged_with_one_track() {
    let (env, _mcp) = setup();
    let text = context_text(&env.hook("session-start", "s2"));
    assert!(!text.contains("Tracks ("), "{text}");
    assert!(text.contains("Goal: Ship it"), "{text}");
}

#[test]
fn briefing_caps_track_list() {
    let env = Env::new();
    assert!(env.run(&["plan", "edit"], SEED).status.success());
    for i in 2..=7 {
        assert!(
            env.run(&["track", "start", &format!("Side {i}")], "")
                .status
                .success()
        );
    }
    let text = context_text(&env.hook("session-start", "s1"));
    assert!(
        text.contains("Tracks (7 open):") && text.contains("(+2 more)"),
        "{text}"
    );
    assert!(text.lines().count() <= 40, "{text}");
}

#[test]
fn claim_reminder_names_tracks() {
    let env = two_track_env();
    env.hook("session-start", "s1");
    let text = context_text(&prompt(&env, "s1", "Rewrite the readme"));
    assert!(
        text.contains("T2-1.1 Readme rewrite [Docs refresh]"),
        "{text}"
    );
}

#[test]
fn claim_reminder_asks_when_nothing_matches_and_two_tracks() {
    let env = two_track_env();
    env.hook("session-start", "s1");
    let text = context_text(&prompt(&env, "s1", "Plant tomatoes in the garden"));
    assert!(text.contains("No plan task matches"), "{text}");
    assert!(
        text.contains("2 open tracks") && text.contains("continue a track or start a new one"),
        "{text}"
    );
    // With one track the reminder reads as before.
    let (one, _mcp) = setup();
    one.hook("session-start", "s2");
    let text = context_text(&prompt(&one, "s2", "Plant tomatoes in the garden"));
    assert!(!text.contains("open tracks"), "{text}");
}

#[test]
fn init_plan_refused_with_open_track() {
    let (env, mut mcp) = setup();
    let (err, text) = call_as(
        &mut mcp,
        &env,
        "s1",
        "init_plan",
        &json!({"goal": {"title": "x", "intent": "y"}, "milestones": [], "tasks": [],
                "owner_said": "yes"}),
    );
    assert!(
        err && text.contains("track 1 'Ship it'") && text.contains("start_track"),
        "{text}"
    );
}

#[test]
fn plan_show_track_flag() {
    let env = two_track_env();
    let all = stdout(&env.run(&["plan", "show"], ""));
    assert!(
        all.contains("Ship it") && all.contains("Docs refresh"),
        "{all}"
    );
    let only = stdout(&env.run(&["plan", "show", "--track", "2"], ""));
    assert!(
        only.contains("Docs refresh") && !only.contains("Ship it"),
        "{only}"
    );
}

#[test]
fn usage_by_track_and_forecast_per_track_run() {
    let env = two_track_env();
    let out = env.run(&["usage", "--by", "track"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    let out = env.run(&["usage", "--track", "2"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("track 2"), "{}", stdout(&out));
    // Several open tracks: each gets its own forecast.
    let f = stdout(&env.run(&["forecast"], ""));
    assert!(
        f.contains("track 1 Ship it") && f.contains("track 2 Docs refresh"),
        "{f}"
    );
    let f2 = stdout(&env.run(&["forecast", "--track", "2"], ""));
    assert!(
        f2.contains("track 2 Docs refresh") && !f2.contains("track 1"),
        "{f2}"
    );
    assert!(!env.run(&["forecast", "--track", "9"], "").status.success());
}

#[test]
fn status_tool_prints_each_track() {
    let env = two_track_env();
    env.hook("session-start", "s1");
    let mut mcp = Mcp::start(&env, "/nonexistent");
    let (err, text) = call_as(&mut mcp, &env, "s1", "status", &json!({}));
    assert!(!err, "{text}");
    assert!(
        text.contains("Track 1 — Goal: Ship it") && text.contains("Track 2 — Goal: Docs refresh"),
        "{text}"
    );
    assert!(text.contains("T2-1.1 Readme rewrite"), "{text}");
}

#[test]
fn status_lists_tracks() {
    let env = two_track_env();
    env.hook("session-start", "s1");
    let text = stdout(&env.run(&["status"], ""));
    assert!(
        text.contains("  track 1 Ship it") && text.contains("  track 2 Docs refresh"),
        "{text}"
    );
    // Hiding one bar says so and the line disappears.
    assert!(env.run(&["hide", "track", "2"], "").status.success());
    let text = stdout(&env.run(&["status"], ""));
    assert!(!text.contains("  track 2 Docs refresh"), "{text}");
    assert!(text.contains("1 track hidden"), "{text}");
}
