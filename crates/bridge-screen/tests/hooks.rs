//! `bridge-screen hook <event>`: the binary run the way Claude Code runs it.

mod common;

use common::{EVENTS, Env};

#[test]
fn each_event_is_accepted() {
    let env = Env::new();
    for event in EVENTS {
        let out = env.hook(event, "s-accept");
        assert_eq!(out, "", "{event} printed something");
    }
    assert_eq!(env.log(), "", "unexpected hook errors");
}

#[test]
fn hook_never_fails_the_session() {
    // (a) stdin that isn't JSON
    let env = Env::new();
    assert_eq!(env.hook_raw("user-prompt-submit", "not json"), "");
    let log = env.log();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("user-prompt-submit"), "{log}");

    // (b) BRIDGE_SCREEN_HOME is a file, not a folder: nowhere to log, still exit 0
    let env = Env::new();
    let file = env.home.path().join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    let mut env_b = Env::new();
    env_b.vars.push((
        "BRIDGE_SCREEN_HOME".into(),
        file.to_str().unwrap().to_string(),
    ));
    assert_eq!(env_b.hook("session-start", "s-b"), "");

    // (c) a database file full of garbage
    let env = Env::new();
    std::fs::write(env.db_path(), vec![0xAB_u8; 8192]).unwrap();
    assert_eq!(env.hook("stop", "s-c"), "");
    let log = env.log();
    assert!(log.contains(" stop "), "{log}");
}
