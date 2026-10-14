//! `bridge-screen ui`: one instance at a time, and a lock left by a dead process is
//! replaced, not obeyed (spec §4.1). The one test that binds a real socket (port 0).

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::channel;
use std::time::Duration;

use common::BIN;

fn ui(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(["ui", "--no-open", "--port", "0"])
        .env("BRIDGE_SCREEN_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Start a server and return it with the URL it printed.
fn start(home: &std::path::Path) -> (Child, String) {
    let mut child = ui(home).spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let line = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("bridge-screen ui printed nothing");
    let url = line
        .strip_prefix("bridge-screen dashboard: ")
        .unwrap_or_else(|| panic!("unexpected first line: {line}"))
        .to_string();
    (child, url)
}

fn lock(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("ui.lock")).unwrap()).unwrap()
}

#[test]
fn ui_lock_single_instance_and_stale() {
    let home = tempfile::tempdir().unwrap();
    let (mut first, url) = start(home.path());
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let l = lock(home.path());
    let token = l["token"].as_str().unwrap().to_string();
    assert_eq!(
        url,
        format!("http://127.0.0.1:{}/#token={token}", l["port"])
    );
    assert_eq!(token.len(), 64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(home.path().join("ui.lock"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // A second start prints the running instance's URL and exits 0.
    let second = ui(home.path()).output().unwrap();
    assert!(second.status.success());
    assert_eq!(
        String::from_utf8(second.stdout).unwrap(),
        format!("bridge-screen dashboard (already running): {url}\n")
    );

    // The first dies without cleaning up: its lock is stale and is replaced.
    first.kill().unwrap();
    first.wait().unwrap();
    assert!(home.path().join("ui.lock").exists());
    let (mut third, url3) = start(home.path());
    assert_ne!(url3, url);
    assert_ne!(lock(home.path())["token"], token.as_str());
    third.kill().unwrap();
    third.wait().unwrap();

    // The token never reaches a log.
    let log = std::fs::read_to_string(home.path().join("hook-errors.log")).unwrap_or_default();
    assert!(!log.contains(&token));
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut third.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(!stderr.contains(&token), "{stderr}");
}
