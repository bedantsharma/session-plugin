//! The navigator through the hooks: labelling units at Stop, label requests, tolerance
//! warnings and owner answers.

mod common;

use bridge_screen_core::session::{self, Unit};
use bridge_screen_core::store::{Event, Store, read_all};
use bridge_screen_core::tangent::{Label, LabelSource};
use common::{Env, fixture_json};
use serde_json::{Value, json};

pub const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form validation","estimate_min":30,
   "expected_files":["src/login.rs"]},
  {"op":"add_node","parent":"1","kind":"task","title":"Password reset"}
]"#;

const ON_TASK: &str = "Added validation to the login form and a unit test.";
const DOCKER: &str = "Rewrote the Dockerfile and the CI workflow caching.";
const MIDDLE: &str = "Refactored helper scripts.";

const LABEL_REQUEST: &str =
    "bridge-screen: label your last unit — on-task, detour or drift — with label_unit (one call).";
const DRIFT_REQUEST: &str = "bridge-screen: possible drift in your last unit. Label it — on-task, detour or drift — with label_unit (one call).";

/// A repo with a plan and session `s1` holding 1.1.
fn setup() -> Env {
    let env = Env::new();
    let out = env.run(&["plan", "edit"], SEED);
    assert!(out.status.success());
    env.hook("session-start", "s1");
    claim(&env, "s1", "1.1");
    env
}

fn claim(env: &Env, session: &str, task: &str) {
    use bridge_screen_core::claim::{ClaimMode, ClaimRequest, ClaimTarget, claim};
    let mut store = Store::open(&env.db_path()).unwrap();
    let repo_id = session::get(store.conn(), session)
        .unwrap()
        .unwrap()
        .repo_id;
    claim(
        &mut store,
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

fn hook_with(env: &Env, event: &str, session: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut payload = fixture_json(event, session, &env.repo_path());
    edit(&mut payload);
    env.hook_raw(event, &payload.to_string())
}

/// One unit: a prompt, edits to `files`, then Stop with `message`. Returns Stop's output.
fn unit(env: &Env, session: &str, files: &[&str], message: &str) -> String {
    env.hook("user-prompt-submit", session);
    for f in files {
        let path = format!("{}/{f}", env.repo_path());
        hook_with(env, "post-tool-use", session, |p| {
            p["tool_input"]["file_path"] = json!(path);
        });
    }
    stop(env, session, message, false)
}

fn stop(env: &Env, session: &str, message: &str, active: bool) -> String {
    hook_with(env, "stop", session, |p| {
        p["last_assistant_message"] = json!(message);
        p["stop_hook_active"] = json!(active);
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

fn units(env: &Env, session: &str) -> Vec<Unit> {
    let store = Store::open(&env.db_path()).unwrap();
    session::units(store.conn(), session).unwrap()
}

fn events(env: &Env) -> Vec<Event> {
    let store = Store::open(&env.db_path()).unwrap();
    read_all(store.conn(), None).unwrap()
}

fn kinds(env: &Env) -> Vec<String> {
    events(env).into_iter().map(|e| e.kind).collect()
}

#[test]
fn low_score_labels_silently() {
    let env = setup();
    assert_eq!(unit(&env, "s1", &["src/login.rs"], ON_TASK), "");
    let u = &units(&env, "s1")[0];
    assert_eq!(u.label, Some(Label::OnTask));
    assert_eq!(u.label_source, Some(LabelSource::Heuristic));
    assert!(u.drift_score.unwrap() < 0.3, "{u:?}");
    assert!(!kinds(&env).contains(&"note_sent".to_string()));
    assert_eq!(env.log(), "");
}

#[test]
fn declared_detour_wins() {
    let env = setup();
    env.hook("user-prompt-submit", "s1");
    {
        let mut store = Store::open(&env.db_path()).unwrap();
        bridge_screen_core::tangent::start_detour(
            &mut store,
            &bridge_screen_core::tangent::StartRequest {
                session_id: "s1".into(),
                reason: "CI cache is broken".into(),
                resume_cue: "rerun the login test".into(),
                blocks: None,
                actor: bridge_screen_core::store::Actor::default(),
            },
        )
        .unwrap();
    }
    let path = format!("{}/docker/Dockerfile", env.repo_path());
    hook_with(&env, "post-tool-use", "s1", |p| {
        p["tool_input"]["file_path"] = json!(path);
    });
    assert_eq!(stop(&env, "s1", DOCKER, false), "");
    let u = &units(&env, "s1")[0];
    assert_eq!(u.label, Some(Label::Detour));
    assert_eq!(u.label_source, Some(LabelSource::Declared));
    assert!(!u.flagged);
    assert!(!kinds(&env).contains(&"drift_flagged".to_string()));
}

#[test]
fn label_request_is_sent_once_per_unit() {
    let env = setup();
    let out = unit(&env, "s1", &["src/login.rs", "tools/gen.py"], MIDDLE);
    assert_eq!(context_of(&out, "Stop"), LABEL_REQUEST);
    let u = &units(&env, "s1")[0];
    assert_eq!(u.label, None, "waits for the agent's label");
    let score = u.drift_score.unwrap();
    assert!((0.3..=0.7).contains(&score), "{score}");

    // The continuation turn ends with another Stop: nothing more, flag or no flag.
    assert_eq!(stop(&env, "s1", MIDDLE, false), "");
    assert_eq!(stop(&env, "s1", MIDDLE, true), "");
    let notes = kinds(&env).iter().filter(|k| *k == "note_sent").count();
    assert_eq!(notes, 1);

    // No answer before the next prompt: the unit is labelled on-task by the heuristic.
    env.hook("user-prompt-submit", "s1");
    let u = &units(&env, "s1")[0];
    assert_eq!(
        (u.label, u.label_source),
        (Some(Label::OnTask), Some(LabelSource::Heuristic))
    );

    // Within three units the same request isn't repeated: labelled on the spot.
    for f in ["src/login.rs", "scripts/x.sh"] {
        let path = format!("{}/{f}", env.repo_path());
        hook_with(&env, "post-tool-use", "s1", |p| {
            p["tool_input"]["file_path"] = json!(path);
        });
    }
    assert_eq!(stop(&env, "s1", MIDDLE, false), "");
    let u = &units(&env, "s1")[1];
    assert_eq!(
        (u.label, u.label_source),
        (Some(Label::OnTask), Some(LabelSource::Heuristic))
    );
}

#[test]
fn stop_hook_active_never_continues() {
    let env = setup();
    env.hook("user-prompt-submit", "s1");
    for f in ["src/login.rs", "tools/gen.py"] {
        let path = format!("{}/{f}", env.repo_path());
        hook_with(&env, "post-tool-use", "s1", |p| {
            p["tool_input"]["file_path"] = json!(path);
        });
    }
    assert_eq!(stop(&env, "s1", MIDDLE, true), "");
    let u = &units(&env, "s1")[0];
    assert_eq!(
        (u.label, u.label_source),
        (Some(Label::OnTask), Some(LabelSource::Heuristic))
    );
    assert!(!kinds(&env).contains(&"note_sent".to_string()));
}

#[test]
fn high_score_is_flagged_and_asked_about() {
    let env = setup();
    let out = unit(
        &env,
        "s1",
        &["docker/Dockerfile", ".github/workflows/ci.yml"],
        DOCKER,
    );
    assert_eq!(context_of(&out, "Stop"), DRIFT_REQUEST);
    let u = &units(&env, "s1")[0];
    assert!(u.flagged && u.drift_score.unwrap() > 0.7, "{u:?}");
    assert_eq!(u.label, None);
    let flagged = events(&env)
        .into_iter()
        .find(|e| e.kind == "drift_flagged")
        .unwrap();
    assert_eq!(flagged.data["unit_no"], 1);
}

#[test]
fn summary_text_is_not_stored() {
    let env = setup();
    let long = format!(
        "{} MARKERWORDBEYOND300 {}",
        "word ".repeat(80),
        "more ".repeat(100)
    );
    unit(&env, "s1", &["src/login.rs"], &long);
    let stored: String = events(&env).iter().map(|e| e.data.to_string()).collect();
    assert!(!stored.contains("MARKERWORDBEYOND300"));
    let summary = events(&env)
        .into_iter()
        .find(|e| e.kind == "unit_stopped")
        .unwrap()
        .data["summary"]
        .as_str()
        .unwrap()
        .chars()
        .count();
    assert_eq!(summary, 300);
}
