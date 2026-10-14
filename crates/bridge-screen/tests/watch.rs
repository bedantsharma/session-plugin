//! `bridge-screen watch` without a terminal prints what `status` prints, once.

mod common;

use common::Env;

#[test]
fn watch_without_a_tty_prints_status() {
    let env = Env::new();
    env.hook("session-start", "w1aaaa");
    env.hook("user-prompt-submit", "w1aaaa");
    let status = env.run(&["status"], "");
    let watch = env.run(&["watch"], "");
    assert!(
        watch.status.success(),
        "{}",
        String::from_utf8_lossy(&watch.stderr)
    );
    assert_eq!(
        String::from_utf8(watch.stdout).unwrap(),
        String::from_utf8(status.stdout).unwrap()
    );
    // NO_COLOR and TERM=dumb: plain text either way.
    let mut cmd = std::process::Command::new(common::BIN);
    let out = cmd
        .arg("status")
        .current_dir(env.repo.path())
        .env("BRIDGE_SCREEN_HOME", env.home.path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .output()
        .unwrap();
    assert!(!String::from_utf8(out.stdout).unwrap().contains('\x1b'));
}
