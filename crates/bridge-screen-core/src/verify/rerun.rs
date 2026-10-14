//! The L2 re-run (spec §8.5): the owner's test command, run with `sh -c` in the session's
//! worktree with a time limit. Only the outcome is kept: exit code, duration and the last
//! non-empty output line. Never run from a hook.
//!
//! The command runs in its own process group, so at the time limit the whole group —
//! including anything it started in the background — is stopped (`TERM`, then `KILL`
//! after a grace period). Output (stdout and stderr on one pipe) is read on a thread so a chatty command never
//! blocks on a full pipe; only its tail is kept.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Longest last line kept, in characters.
pub const MAX_LAST_LINE: usize = 200;

/// How often the child is polled.
const POLL: Duration = Duration::from_millis(50);
/// After `TERM`, how long the group gets before `KILL`.
const GRACE: Duration = Duration::from_secs(2);
/// How long to wait for the output readers once the command is gone (a process that
/// escaped the group may still hold the pipes; it is then left behind, not waited on).
const DRAIN: Duration = Duration::from_secs(2);
/// Bytes of output kept (the tail).
const TAIL_BYTES: usize = 64 * 1024;

/// What a re-run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerunOutcome {
    Passed {
        secs: u64,
        last_line: String,
    },
    Failed {
        code: i32,
        secs: u64,
        last_line: String,
    },
    TimedOut {
        secs: u64,
    },
    /// The shell couldn't run the command: 127 (not found) or 126 (not executable). A
    /// broken test command, not a test failure.
    CannotRun {
        code: i32,
        last_line: String,
    },
    /// The command couldn't be started.
    SpawnError(String),
}

/// Exit codes `sh` uses when it can't run the command at all.
const CANNOT_RUN: [i32; 2] = [126, 127];

/// Copy everything from `from` into the shared tail buffer; say when done.
fn reader(mut from: impl Read + Send + 'static, buf: Arc<Mutex<Vec<u8>>>) -> Receiver<()> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match from.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut b) = buf.lock() {
                        b.extend_from_slice(&chunk[..n]);
                        if b.len() > 2 * TAIL_BYTES {
                            let cut = b.len() - TAIL_BYTES;
                            b.drain(..cut);
                        }
                    }
                }
            }
        }
        let _ = tx.send(());
    });
    rx
}

/// The last non-empty line of `out`, trimmed, at most [`MAX_LAST_LINE`] characters.
pub fn last_line(out: &[u8]) -> String {
    String::from_utf8_lossy(out)
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(MAX_LAST_LINE)
        .collect()
}

/// Send `signal` to the child's whole process group.
#[cfg(unix)]
fn signal_group(child: &Child, signal: &str) {
    let _ = Command::new("kill")
        .args(["-s", signal, "--", &format!("-{}", child.id())])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn signal_group(child: &Child, _signal: &str) {
    // No process groups: only the shell itself can be stopped.
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &child.id().to_string()])
        .status();
}

/// Poll until the child exits or `deadline` passes; its exit status if it exited.
fn wait_until(child: &mut Child, deadline: Instant) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => return None,
        }
    }
}

fn secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_millis() >= 500)
}

/// Run `command` with `sh -c` in `wt`, stdin closed, for at most `timeout`.
pub fn run(wt: &Path, command: &str, timeout: Duration) -> RerunOutcome {
    // stdout and stderr share one pipe, so the last line is the last one written.
    let (out_read, out_write) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return RerunOutcome::SpawnError(format!("could not make a pipe: {e}")),
    };
    let err_write = match out_write.try_clone() {
        Ok(w) => w,
        Err(e) => return RerunOutcome::SpawnError(format!("could not make a pipe: {e}")),
    };
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(wt)
        .stdin(Stdio::null())
        .stdout(out_write)
        .stderr(err_write);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    let start = Instant::now();
    let spawned = cmd.spawn();
    // Our copies of the write ends must close, or the reader never sees the end.
    drop(cmd);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return RerunOutcome::SpawnError(format!("could not run sh: {e}")),
    };
    let buf = Arc::new(Mutex::new(Vec::new()));
    let done = vec![reader(out_read, Arc::clone(&buf))];
    let status = wait_until(&mut child, start + timeout);
    let took = start.elapsed();
    if status.is_none() {
        signal_group(&child, "TERM");
        let shell_gone = wait_until(&mut child, Instant::now() + GRACE).is_some();
        // Whatever ignored TERM goes now, whether or not the shell is gone: a member
        // that doesn't hold the output open would otherwise outlive the time limit.
        signal_group(&child, "KILL");
        if !shell_gone {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    // Background processes left in the group would keep the pipes open: once the shell is
    // gone they have no business running, so the group is stopped before draining.
    let drain_until = Instant::now() + DRAIN;
    let drained = |done: &[Receiver<()>], until: Instant| {
        done.iter().all(|rx| {
            rx.recv_timeout(until.saturating_duration_since(Instant::now()))
                .is_ok()
        })
    };
    if !drained(&done, Instant::now() + Duration::from_millis(200)) {
        signal_group(&child, "KILL");
        drained(&done, drain_until);
    }
    let Some(status) = status else {
        return RerunOutcome::TimedOut { secs: secs(took) };
    };
    let last_line = buf.lock().map(|b| last_line(&b)).unwrap_or_default();
    let secs = secs(took);
    let code = status.code().unwrap_or(-1);
    if status.success() {
        RerunOutcome::Passed { secs, last_line }
    } else if CANNOT_RUN.contains(&code) {
        RerunOutcome::CannotRun { code, last_line }
    } else {
        RerunOutcome::Failed {
            code,
            secs,
            last_line,
        }
    }
}
