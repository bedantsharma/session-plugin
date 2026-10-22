//! Project pages end to end: the folder survives `rebuild`, the SessionStart briefing offers
//! and points at the pages (spec 2026-10-22).

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use bridge_screen_core::pages::{self, Page, WriteMode};
use bridge_screen_core::paths::Home;
use bridge_screen_core::repo;
use common::Env;

/// Every file under `dir` with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        out.insert(
            path.file_name().unwrap().to_string_lossy().into_owned(),
            std::fs::read(&path).unwrap(),
        );
    }
    out
}

#[test]
fn rebuild_leaves_pages_alone() {
    let env = Env::new();
    let id = repo::identify(env.repo.path()).unwrap().id;
    let home = Home::at(env.home.path());
    pages::set_up(&home, &id, "yes").unwrap();
    pages::write(
        &home,
        &id,
        Page::DecisionLog,
        "## #1 something",
        WriteMode::Append,
        None,
    )
    .unwrap();
    let before = snapshot(&home.lead_dir(&id));
    assert!(before.len() >= 5);
    assert!(env.run(&["rebuild"], "").status.success());
    assert_eq!(snapshot(&home.lead_dir(&id)), before);
}

const PLAN: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Accounts","intent":"People can sign in"},
  {"op":"add_node","parent":"1","kind":"task","title":"Login form validation"}
]"#;

const OFFER_MARK: &str = "no project pages yet";

fn with_plan() -> Env {
    let env = Env::new();
    let out = env.run(&["plan", "edit"], PLAN);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    env
}

fn repo_id(env: &Env) -> String {
    repo::identify(env.repo.path()).unwrap().id
}

fn home_of(env: &Env) -> Home {
    Home::at(env.home.path())
}

/// The SessionStart context text, or "" when the hook said nothing.
fn start_text(env: &Env, session: &str) -> String {
    context(&env.hook("session-start", session))
}

fn start_text_with(env: &Env, session: &str, source: &str) -> String {
    let mut p = common::fixture_json("session-start", session, &env.repo_path());
    p["source"] = serde_json::json!(source);
    context(&env.hook_raw("session-start", &p.to_string()))
}

fn context(out: &str) -> String {
    if out.trim().is_empty() {
        return String::new();
    }
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn offered_marker(env: &Env) -> Option<String> {
    std::fs::read_to_string(home_of(env).lead_dir(&repo_id(env)).join(".offered")).ok()
}

#[test]
fn offer_is_made_up_to_three_times() {
    let env = with_plan();
    for (n, session) in ["s1", "s2", "s3"].into_iter().enumerate() {
        let text = start_text(&env, session);
        assert!(text.contains(OFFER_MARK), "session {session}: {text}");
        assert_eq!(offered_marker(&env), Some((n + 1).to_string()));
    }
    let fourth = start_text(&env, "s4");
    assert!(!fourth.contains(OFFER_MARK), "{fourth}");
    assert_eq!(offered_marker(&env), Some("3".to_string()));
}

#[test]
fn offer_stops_after_decline_or_setup() {
    let env = with_plan();
    let (id, home) = (repo_id(&env), home_of(&env));
    assert!(start_text(&env, "s1").contains(OFFER_MARK));
    pages::decline(&home, &id, "no thanks").unwrap();
    assert!(!start_text(&env, "s2").contains("roject pages"));

    let env = with_plan();
    let (id, home) = (repo_id(&env), home_of(&env));
    pages::set_up(&home, &id, "yes").unwrap();
    let text = start_text(&env, "s1");
    assert!(!text.contains(OFFER_MARK));
    assert!(text.lines().any(|l| l.starts_with("Project pages: read")));
}

#[test]
fn offer_yields_to_onboarding_note() {
    let env = Env::new();
    let text = start_text(&env, "s1");
    assert!(text.contains("No plan yet"), "{text}");
    assert!(!text.contains(OFFER_MARK));
    assert_eq!(offered_marker(&env), None);
}

#[test]
fn pointer_shows_last_updated_date() {
    let env = with_plan();
    let (id, home) = (repo_id(&env), home_of(&env));
    pages::set_up(&home, &id, "yes").unwrap();
    let text = start_text(&env, "s1");
    let line = text
        .lines()
        .find(|l| l.starts_with("Project pages:"))
        .unwrap();
    let today = jiff::Timestamp::now().to_string();
    assert!(line.contains(&today[..10]), "{line}");
}

#[test]
fn unreadable_pages_folder_does_not_fail_hook() {
    let env = with_plan();
    let (id, home) = (repo_id(&env), home_of(&env));
    std::fs::create_dir_all(home.lead_dir(&id).parent().unwrap()).unwrap();
    std::fs::write(home.lead_dir(&id), "a file where the folder should be").unwrap();
    // env.hook asserts exit code 0.
    let text = start_text(&env, "s1");
    assert!(text.contains("Goal: Ship"), "{text}");
    assert!(!text.contains("roject pages"), "{text}");
}

#[test]
fn briefing_with_pages_note_stays_within_40_lines() {
    let env = with_plan();
    let text = start_text(&env, "s1");
    assert!(text.contains(OFFER_MARK));
    assert!(text.lines().count() <= 40, "{text}");
    pages::set_up(&home_of(&env), &repo_id(&env), "yes").unwrap();
    let text = start_text(&env, "s2");
    assert!(text.lines().count() <= 40, "{text}");
}

#[test]
fn compaction_rebriefing_has_pointer_only() {
    let env = with_plan();
    start_text(&env, "s1");
    let marker = offered_marker(&env);
    let compact = start_text_with(&env, "s1", "compact");
    assert!(!compact.contains(OFFER_MARK), "{compact}");
    assert_eq!(offered_marker(&env), marker, "no offer counted");
    pages::set_up(&home_of(&env), &repo_id(&env), "yes").unwrap();
    let compact = start_text_with(&env, "s1", "compact");
    assert!(
        compact
            .lines()
            .any(|l| l.starts_with("Project pages: read")),
        "{compact}"
    );
}
