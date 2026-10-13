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

    // The next unit is its own question: asked again, not labelled on the spot.
    for f in ["src/login.rs", "scripts/x.sh"] {
        let path = format!("{}/{f}", env.repo_path());
        hook_with(&env, "post-tool-use", "s1", |p| {
            p["tool_input"]["file_path"] = json!(path);
        });
    }
    assert_eq!(
        context_of(&stop(&env, "s1", MIDDLE, false), "Stop"),
        LABEL_REQUEST
    );
    assert_eq!(units(&env, "s1")[1].label, None);
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
fn every_flagged_unit_is_asked_about() {
    let env = setup();
    let docker = ["docker/Dockerfile", ".github/workflows/ci.yml"];
    assert_eq!(
        context_of(&unit(&env, "s1", &docker, DOCKER), "Stop"),
        DRIFT_REQUEST
    );
    // The owner settles unit 1, so nothing else competes for unit 2's one note.
    assert!(
        env.run(&["label", "s1", "1", "on-task"], "")
            .status
            .success()
    );
    // Within the 3-unit repeat window, the next flagged unit is still asked, not
    // silently labelled drift.
    let infra = ["infra/main.tf", "deploy/values.yaml"];
    assert_eq!(
        context_of(&unit(&env, "s1", &infra, DOCKER), "Stop"),
        DRIFT_REQUEST
    );
    let u = &units(&env, "s1")[1];
    assert!(u.flagged && u.label.is_none(), "{u:?}");
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

mod tolerance {
    use super::*;
    use bridge_screen_core::store::{EventDraft, append_in_at};
    use jiff::{SignedDuration, Timestamp};

    /// `s1` has been on detour "CI cache is broken" for 35 minutes (task 1.1, 30-minute
    /// estimate: tolerance 15 minutes), as of now.
    fn far_off(env: &Env, session: &str) {
        env.hook("session-start", session);
        claim(env, session, "1.1");
        let mut store = Store::open(&env.db_path()).unwrap();
        let repo = session::get(store.conn(), session)
            .unwrap()
            .unwrap()
            .repo_id;
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
            (
                "item_suspended",
                json!({"item_id": format!("I-{session}"), "kind": "task", "ref_id": "1.1",
                       "resume_cue": "rerun the login test", "by_detour": format!("D-{session}")}),
                ago(40),
            ),
            ("unit_stopped", json!({"unit_no": 1}), ago(5)),
            (
                "unit_labeled",
                json!({"unit_no": 1, "label": "detour", "source": "declared"}),
                ago(5),
            ),
        ];
        store
            .write(|tx| {
                for (kind, data, ts) in script {
                    append_in_at(tx, EventDraft::new(&repo, session, kind, data), ts)?;
                }
                Ok(())
            })
            .unwrap();
    }

    fn bash(env: &Env, session: &str) -> String {
        hook_with(env, "post-tool-use", session, |p| {
            p["tool_name"] = json!("Bash");
            p["tool_input"] = json!({"command": "cargo test"});
        })
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

    fn nav_state(env: &Env, session: &str) -> String {
        let store = Store::open(&env.db_path()).unwrap();
        session::get(store.conn(), session)
            .unwrap()
            .unwrap()
            .nav_state
            .as_str()
            .to_string()
    }

    const RED: &str = "bridge-screen: this detour is well past tolerance; the owner has been flagged. Keep going if it's essential, or close it and return to 1.1 Login form validation.";

    #[test]
    fn red_flags_the_owner_and_is_listed_first() {
        let env = Env::new();
        assert!(env.run(&["plan", "edit"], SEED).status.success());
        far_off(&env, "s1aaaa");
        env.hook("session-start", "s2bbbb");
        let out = bash(&env, "s1aaaa");
        assert_eq!(context_of(&out, "PostToolUse"), RED);
        assert_eq!(nav_state(&env, "s1aaaa"), "red");
        let flagged = events(&env)
            .into_iter()
            .find(|e| e.kind == "needs_owner")
            .unwrap();
        assert_eq!(flagged.data["kind"], "drift");
        // Rate-limited: the next tool call says nothing.
        assert_eq!(bash(&env, "s1aaaa"), "");
        let (_, status) = cli(&env, &["status"]);
        let lines: Vec<&str> = status.lines().collect();
        assert!(lines[2].starts_with("  S s1aaaa  needs you"), "{status}");
        assert!(lines[3].starts_with("  S s2bbbb  idle"), "{status}");
    }

    #[test]
    fn continue_drops_back_to_green_and_is_told_once() {
        let env = Env::new();
        assert!(env.run(&["plan", "edit"], SEED).status.success());
        far_off(&env, "s1aaaa");
        bash(&env, "s1aaaa");
        let (ok, text) = cli(&env, &["drift", "s1", "continue"]);
        assert!(ok, "{text}");
        assert_eq!(text, "Told s1aaaa: continue.\n");
        assert_eq!(nav_state(&env, "s1aaaa"), "green");
        // The answer reaches the agent at its next prompt, once.
        let out = env.hook("user-prompt-submit", "s1aaaa");
        assert_eq!(
            context_of(&out, "UserPromptSubmit"),
            "bridge-screen: the owner says continue."
        );
        assert_eq!(bash(&env, "s1aaaa"), "");
        assert_eq!(env.hook("user-prompt-submit", "s1aaaa"), "");
    }

    #[test]
    fn stop_reaches_the_agent_once() {
        let env = Env::new();
        assert!(env.run(&["plan", "edit"], SEED).status.success());
        far_off(&env, "s1aaaa");
        let (ok, text) = cli(&env, &["drift", "s1a", "stop"]);
        assert!(ok, "{text}");
        let out = env.hook("user-prompt-submit", "s1aaaa");
        assert_eq!(
            context_of(&out, "UserPromptSubmit"),
            "bridge-screen: the owner says stop the detour and return to 1.1 Login form validation."
        );
        assert_eq!(env.hook("user-prompt-submit", "s1aaaa"), "");
        let replies = events(&env)
            .into_iter()
            .filter(|e| e.kind == "note_sent" && e.data["kind"] == "owner_reply")
            .count();
        assert_eq!(replies, 1);
    }

    #[test]
    fn the_owner_reply_outranks_the_red_note() {
        let env = Env::new();
        assert!(env.run(&["plan", "edit"], SEED).status.success());
        far_off(&env, "s1aaaa");
        let (ok, text) = cli(&env, &["drift", "s1", "stop"]);
        assert!(ok, "{text}");
        assert_eq!(nav_state(&env, "s1aaaa"), "red", "stop doesn't acknowledge");
        assert_eq!(
            context_of(&bash(&env, "s1aaaa"), "PostToolUse"),
            "bridge-screen: the owner says stop the detour and return to 1.1 Login form validation."
        );
    }

    #[test]
    fn make_task_promotes_the_detour() {
        let env = Env::new();
        assert!(env.run(&["plan", "edit"], SEED).status.success());
        far_off(&env, "s1aaaa");
        let (ok, text) = cli(&env, &["drift", "s1", "make-task"]);
        assert!(!ok);
        assert!(text.contains("--title"), "{text}");
        let (ok, text) = cli(
            &env,
            &[
                "drift",
                "s1",
                "make-task",
                "--parent",
                "1",
                "--title",
                "Fix the CI cache",
            ],
        );
        assert!(ok, "{text}");
        assert_eq!(text, "Detour D1 is now task 1.3, claimed by s1aaaa.\n");
        let store = Store::open(&env.db_path()).unwrap();
        let s = session::get(store.conn(), "s1aaaa").unwrap().unwrap();
        assert_eq!(s.active_task.as_deref(), Some("1.3"));
        assert_eq!(s.current_detour, None);
    }

    #[test]
    fn label_override_creates_an_owner_sample() {
        let env = setup();
        unit(&env, "s1", &["src/login.rs"], ON_TASK);
        let (ok, text) = cli(&env, &["label", "s1", "1", "drift"]);
        assert!(ok, "{text}");
        assert_eq!(text, "Labelled unit 1 of s1 as drift.\n");
        let u = &units(&env, "s1")[0];
        assert_eq!(
            (u.label, u.label_source),
            (Some(Label::Drift), Some(LabelSource::Owner))
        );
        assert!(u.disagreement, "the heuristic said on-task");
        let store = Store::open(&env.db_path()).unwrap();
        let (n, source): (i64, String) = store
            .conn()
            .query_row("SELECT count(*), max(source) FROM drift_samples", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((n, source.as_str()), (1, "owner"));
        let (ok, text) = cli(&env, &["label", "s1", "7", "drift"]);
        assert!(!ok);
        assert!(
            text.contains("no finished unit 7 in this session"),
            "{text}"
        );
    }

    #[test]
    fn session_prefix_must_be_unique() {
        let env = Env::new();
        env.hook("session-start", "abc111");
        env.hook("session-start", "abc222");
        let (ok, text) = cli(&env, &["drift", "abc", "continue"]);
        assert!(!ok);
        assert!(text.contains("ambiguous"), "{text}");
        let (ok, text) = cli(&env, &["drift", "zzz", "continue"]);
        assert!(!ok);
        assert!(text.contains("no live session starts with zzz"), "{text}");
    }

    #[test]
    fn a_drift_answer_needs_a_session_outside_tolerance() {
        let env = Env::new();
        env.hook("session-start", "abc111");
        for answer in ["continue", "stop"] {
            let (ok, text) = cli(&env, &["drift", "abc", answer]);
            assert!(!ok, "{text}");
            assert!(
                text.contains("abc111 is inside its tolerance band; there is no drift to answer"),
                "{text}"
            );
        }
        assert!(
            !kinds(&env).contains(&"owner_action".to_string()),
            "nothing is recorded"
        );
        assert_eq!(env.hook("user-prompt-submit", "abc111"), "");
    }
}

mod tuning {
    use super::*;
    use bridge_screen_core::store::EventDraft;

    fn repo_id(env: &Env) -> String {
        let store = Store::open(&env.db_path()).unwrap();
        session::get(store.conn(), "s1").unwrap().unwrap().repo_id
    }

    #[test]
    fn tuned_values_drive_the_stop_flow() {
        let env = setup();
        let mut store = Store::open(&env.db_path()).unwrap();
        store
            .append(EventDraft::new(
                repo_id(&env),
                "owner",
                "config_changed",
                json!({"scope": "repo", "key": "drift_lower", "old": 0.3, "new": 0.1,
                       "samples": 20}),
            ))
            .unwrap();
        drop(store);
        // Scores about 0.12: on-task by default, but above the tuned lower cut-off.
        let out = unit(&env, "s1", &["src/login.rs"], ON_TASK);
        assert_eq!(context_of(&out, "Stop"), LABEL_REQUEST);
        let status = String::from_utf8(env.run(&["status"], "").stdout).unwrap();
        assert!(
            status.contains("goal: Ship  · tuned drift .10–.70, tolerance 20%"),
            "{status}"
        );
    }

    #[test]
    fn session_start_runs_tuning_when_due() {
        let env = setup();
        let repo = repo_id(&env);
        let store = Store::open(&env.db_path()).unwrap();
        for _ in 0..20 {
            store
                .conn()
                .execute(
                    "INSERT INTO drift_samples (repo_id, unit_ref, score, label, source, ts)
                     VALUES (?1, 's1:1', 0.49, 'on_task', 'agent', 't')",
                    [&repo],
                )
                .unwrap();
        }
        drop(store);
        env.hook("session-start", "s2");
        let changed: Vec<Value> = events(&env)
            .into_iter()
            .filter(|e| e.kind == "config_changed")
            .map(|e| e.data)
            .collect();
        assert_eq!(changed.len(), 1, "{changed:?}");
        assert_eq!(changed[0]["key"], "drift_lower");
        assert_eq!(changed[0]["new"], 0.35);
        // Not again until 20 more samples arrive.
        env.hook("session-start", "s3");
        assert_eq!(
            events(&env)
                .iter()
                .filter(|e| e.kind == "config_changed")
                .count(),
            1
        );
    }
}
