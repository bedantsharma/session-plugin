//! `bridge-screen ui`: the local dashboard (spec §4.1, §6.4, §10.1–10.2).
//!
//! One instance at a time: `~/.bridge-screen/ui.lock` holds `{pid, port, token}` (mode
//! 0600). A second start finds the first answering on its port with its token and prints
//! its URL; a lock whose server doesn't answer is stale and is replaced. The server binds
//! 127.0.0.1 only and makes no outbound calls.

pub mod api;
// Shared with build.rs; the binary itself uses it only in tests.
#[allow(dead_code)]
pub mod dist;

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bridge_screen_core::config::Config;
use bridge_screen_core::paths::Home;
use serde::{Deserialize, Serialize};

/// What `ui.lock` holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    pub pid: u32,
    pub port: u16,
    pub token: String,
}

impl Lock {
    /// The address with the token in the fragment (never sent to the server).
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/#token={}", self.port, self.token)
    }

    /// The address without the token.
    pub fn address(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// The lock file's contents, if it exists and reads.
pub fn read_lock(home: &Home) -> Option<Lock> {
    let text = std::fs::read_to_string(home.ui_lock_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Does a bridge-screen dashboard answer on the lock's port with the lock's token?
pub fn answers(lock: &Lock) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], lock.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!(
        "GET /api/ping HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        lock.port, lock.token
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = String::new();
    let _ = stream.take(16 * 1024).read_to_string(&mut response);
    response.starts_with("HTTP/1.1 200") && response.contains("\"ok\":true")
}

/// The running dashboard, if there is one.
pub fn running(home: &Home) -> Option<Lock> {
    read_lock(home).filter(answers)
}

/// 32 random bytes, hex.
fn new_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness for the token: {e}"))?;
    Ok(bytes.iter().fold(String::with_capacity(64), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    }))
}

/// Write the lock atomically (temp file, then rename), readable by the owner only.
fn write_lock(path: &Path, lock: &Lock) -> Result<()> {
    let tmp = path.with_extension(format!("lock.{}.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(&tmp)
        .with_context(|| format!("writing {}", tmp.display()))?;
    file.write_all(serde_json::to_string(lock)?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Open `url` in the browser; failure is ignored (the URL is printed anyway).
fn open_browser(url: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = cmd
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
}

/// `bridge-screen ui [--port N] [--no-open]`.
pub fn run(port: Option<u16>, no_open: bool) -> Result<()> {
    let home = Home::resolve()?;
    home.ensure()?;
    if let Some(lock) = running(&home) {
        println!("bridge-screen dashboard (already running): {}", lock.url());
        return Ok(());
    }
    let cfg = Config::load(&home.config_path())?;
    let listener = TcpListener::bind(("127.0.0.1", port.unwrap_or(cfg.ui_port)))
        .context("binding the dashboard to 127.0.0.1")?;
    let lock = Lock {
        pid: std::process::id(),
        port: listener.local_addr()?.port(),
        token: new_token()?,
    };
    let lock_path = home.ui_lock_path();
    write_lock(&lock_path, &lock)?;
    println!("bridge-screen dashboard: {}", lock.url());
    println!("Ctrl-C stops it.");
    if !no_open {
        open_browser(&lock.url());
    }
    let state = api::AppState {
        home: home.clone(),
        cfg,
        token: lock.token.clone(),
        port: lock.port,
        assets: Arc::new(api::Embedded),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let served = runtime.block_on(async move {
        listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        axum::serve(listener, api::router(state))
            .with_graceful_shutdown(shutdown_signal())
            .await
    });
    // Remove the lock only if it is still ours (a later start may have replaced it).
    if read_lock(&home).is_some_and(|l| l.token == lock.token) {
        let _ = std::fs::remove_file(&lock_path);
    }
    served.context("serving the dashboard")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::dist;

    #[test]
    fn placeholder_is_chosen_without_a_build() {
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("dist");
        let fallback = dir.path().join("placeholder");
        let chosen = dist::choose(&built, "index.html", &fallback).unwrap();
        assert_eq!(chosen, fallback);
        let page = std::fs::read_to_string(fallback.join("index.html")).unwrap();
        assert!(page.contains("npm --prefix web ci &amp;&amp; npm --prefix web run build"));
        std::fs::create_dir_all(&built).unwrap();
        std::fs::write(built.join("index.html"), "<p>app</p>").unwrap();
        assert_eq!(
            dist::choose(&built, "index.html", &fallback).unwrap(),
            built
        );
    }

    #[test]
    fn tokens_are_32_random_bytes() {
        let a = super::new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, super::new_token().unwrap());
    }
}
