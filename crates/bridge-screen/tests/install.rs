//! `bridge-screen install` plugin registration, the plugin package, and its launcher.
//!
//! Every test puts a fake `claude` (or none) on a PATH it fully controls, so the real
//! Claude Code CLI and `~/.claude` are never touched.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::BIN;
use serde_json::Value;

/// The repo checkout: it holds `.claude-plugin/marketplace.json` and `plugin/`.
fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Install {
    home: tempfile::TempDir,
    bin: tempfile::TempDir,
    log: PathBuf,
}

impl Install {
    /// A PATH holding only a fake `claude` that logs its arguments. `script` is the body
    /// after the logging line.
    fn with_fake_claude(script: &str) -> Self {
        let bin = tempfile::tempdir().unwrap();
        let log = bin.path().join("claude.log");
        write_script(
            &bin.path().join("claude"),
            &format!("#!/bin/sh\necho \"$*\" >> \"{}\"\n{script}", log.display()),
        );
        Self {
            home: tempfile::tempdir().unwrap(),
            bin,
            log,
        }
    }

    /// A PATH with no `claude` at all.
    fn without_claude() -> Self {
        let bin = tempfile::tempdir().unwrap();
        let log = bin.path().join("claude.log");
        Self {
            home: tempfile::tempdir().unwrap(),
            bin,
            log,
        }
    }

    fn run(&self, args: &[&str], cwd: &Path) -> Output {
        Command::new(BIN)
            .args(args)
            .current_dir(cwd)
            .env("BRIDGE_SCREEN_HOME", self.home.path())
            .env("PATH", self.bin.path())
            .output()
            .unwrap()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn install_dev_registers_the_checkout() {
    let env = Install::with_fake_claude("exit 0\n");
    let out = env.run(&["install", "--dev"], &checkout().join("crates"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        env.calls(),
        vec![
            format!("plugin marketplace add {}", checkout().display()),
            "plugin install bridge-screen@bridge-screen".to_string(),
        ]
    );
    assert!(env.home.path().join("bridge-screen.db").is_file());
    assert!(stdout(&out).contains("Registered the bridge-screen plugin"));
}

#[test]
fn install_uses_the_github_marketplace_by_default() {
    let env = Install::with_fake_claude("exit 0\n");
    let elsewhere = tempfile::tempdir().unwrap();
    let out = env.run(&["install"], elsewhere.path());
    assert!(out.status.success());
    assert_eq!(
        env.calls()[0],
        "plugin marketplace add bedantsharma/session-plugin"
    );
}

#[test]
fn no_plugin_calls_nothing() {
    let env = Install::with_fake_claude("exit 0\n");
    let out = env.run(&["install", "--no-plugin"], env.home.path());
    assert!(out.status.success());
    assert_eq!(env.calls(), Vec::<String>::new());
}

#[test]
fn missing_claude_prints_manual_steps() {
    let env = Install::without_claude();
    let out = env.run(&["install"], env.home.path());
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("/plugin marketplace add bedantsharma/session-plugin"),
        "{text}"
    );
    assert!(
        text.contains("/plugin install bridge-screen@bridge-screen"),
        "{text}"
    );
}

#[test]
fn already_installed_counts_as_success() {
    let env = Install::with_fake_claude(
        "case \"$*\" in\n\
         *'marketplace add'*) echo \"Marketplace 'bridge-screen' already on disk\"; exit 1;;\n\
         *install*) echo 'Plugin \"bridge-screen@bridge-screen\" is already installed (scope: user)' >&2; exit 1;;\n\
         esac\n",
    );
    let out = env.run(&["install"], env.home.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(env.calls().len(), 2);
}

#[test]
fn other_claude_failures_are_reported() {
    let env = Install::with_fake_claude("echo 'network unreachable' >&2\nexit 1\n");
    let out = env.run(&["install"], env.home.path());
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("network unreachable"), "{err}");
    assert_eq!(env.calls().len(), 1, "stops after the failed step");
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn plugin_package_is_consistent() {
    let plugin = checkout().join("plugin");
    let manifest = json(&plugin.join(".claude-plugin/plugin.json"));
    assert_eq!(manifest["name"], "bridge-screen");
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        core_dependency_version(),
        env!("CARGO_PKG_VERSION"),
        "crates/bridge-screen/Cargo.toml: the bridge-screen-core dependency's version must equal the workspace version (crates.io needs both)"
    );

    let market = json(&checkout().join(".claude-plugin/marketplace.json"));
    assert_eq!(market["name"], "bridge-screen");
    assert_eq!(market["plugins"][0]["name"], "bridge-screen");
    assert_eq!(market["plugins"][0]["source"], "./plugin");

    let hooks = json(&plugin.join("hooks/hooks.json"));
    for (event, arg) in [
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "user-prompt-submit"),
        ("PreToolUse", "pre-tool-use"),
        ("PostToolUse", "post-tool-use"),
        ("Stop", "stop"),
        ("SubagentStop", "subagent-stop"),
        ("PreCompact", "pre-compact"),
        ("Notification", "notification"),
        ("SessionEnd", "session-end"),
    ] {
        let entry = &hooks["hooks"][event][0];
        assert!(
            entry.get("matcher").is_none(),
            "{event} must see every tool"
        );
        let hook = &entry["hooks"][0];
        assert_eq!(hook["type"], "command");
        assert_eq!(
            hook["command"],
            "${CLAUDE_PLUGIN_ROOT}/scripts/bridge-screen"
        );
        assert_eq!(hook["args"], serde_json::json!(["hook", arg]), "{event}");
    }
    assert_eq!(hooks["hooks"].as_object().unwrap().len(), 9);

    let mcp = json(&plugin.join(".mcp.json"));
    let server = &mcp["mcpServers"]["bridge-screen"];
    assert_eq!(
        server["command"],
        "${CLAUDE_PLUGIN_ROOT}/scripts/bridge-screen"
    );
    assert_eq!(server["args"], serde_json::json!(["mcp"]));

    let launcher = plugin.join("scripts/bridge-screen");
    let mode = std::fs::metadata(&launcher).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0, "launcher must be executable");
    let skill = std::fs::read_to_string(plugin.join("skills/bridge-screen/SKILL.md")).unwrap();
    assert!(skill.lines().count() <= 150);
    assert!(skill.starts_with("---\nname: bridge-screen\n"));
}

/// The `version = "…"` of the `bridge-screen-core` dependency in the binary's `Cargo.toml`.
fn core_dependency_version() -> String {
    let manifest =
        std::fs::read_to_string(checkout().join("crates/bridge-screen/Cargo.toml")).unwrap();
    let line = manifest
        .lines()
        .find(|l| l.trim_start().starts_with("bridge-screen-core"))
        .expect("a bridge-screen-core dependency");
    let after = line
        .split("version")
        .nth(1)
        .expect("a version on that line");
    after
        .split('"')
        .nth(1)
        .expect("a quoted version")
        .to_string()
}

#[test]
fn plugin_ships_lead_briefing_skill() {
    let dir = checkout().join("plugin/skills/lead-briefing");
    for file in [
        "SKILL.md",
        "references/why-it-works.md",
        "references/living-docs.md",
    ] {
        let text = std::fs::read_to_string(dir.join(file))
            .unwrap_or_else(|e| panic!("{file} missing from the plugin: {e}"));
        assert!(!text.trim().is_empty(), "{file} is empty");
    }
    let skill = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
    assert!(
        skill.starts_with("---\nname: lead-briefing\n"),
        "{skill:.80}"
    );
    // The page templates the program writes come from the living-docs reference.
    let docs = std::fs::read_to_string(dir.join("references/living-docs.md")).unwrap();
    for heading in ["## 1. Intent", "## 3. Current state", "## 4. Decision log"] {
        assert!(docs.contains(heading), "{heading}");
    }
}

#[test]
fn bridge_screen_skill_mentions_pages_and_lead_briefing() {
    let skill =
        std::fs::read_to_string(checkout().join("plugin/skills/bridge-screen/SKILL.md")).unwrap();
    for needle in [
        "lead-briefing",
        "lead_page_read",
        "lead_page_write",
        "lead_pages",
        "owner_said",
        "current-state",
        "decision-log",
        "glossary",
    ] {
        assert!(skill.contains(needle), "the skill never says {needle}");
    }
    assert!(skill.lines().count() <= 150, "{}", skill.lines().count());
}

/// The target triple the launcher picks on this machine.
fn host_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => "x86_64-unknown-linux-gnu",
    }
}

fn sha256_of(path: &Path) -> String {
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg("if command -v sha256sum >/dev/null; then sha256sum \"$1\"; else shasum -a 256 \"$1\"; fi")
        .arg("sh")
        .arg(path)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

/// A fake GitHub release: `bridge-screen-<target>.tar.xz` holding a script that echoes its
/// arguments, plus its `.sha256` file (correct or not).
fn fake_release(correct_checksum: bool) -> tempfile::TempDir {
    let release = tempfile::tempdir().unwrap();
    let name = format!("bridge-screen-{}", host_target());
    let stage = release.path().join("stage").join(&name);
    std::fs::create_dir_all(&stage).unwrap();
    write_script(
        &stage.join("bridge-screen"),
        "#!/bin/sh\necho \"downloaded bridge-screen $*\"\n",
    );
    let archive = release.path().join(format!("{name}.tar.xz"));
    let ok = Command::new("tar")
        .arg("-cJf")
        .arg(&archive)
        .arg("-C")
        .arg(release.path().join("stage"))
        .arg(&name)
        .env("PATH", "/usr/bin:/bin")
        .status()
        .unwrap();
    assert!(ok.success());
    let sum = if correct_checksum {
        sha256_of(&archive)
    } else {
        "0".repeat(64)
    };
    std::fs::write(
        release.path().join(format!("{name}.tar.xz.sha256")),
        format!("{sum}  {name}.tar.xz\n"),
    )
    .unwrap();
    release
}

fn launch(release: &Path, data: &Path, path: &str, args: &[&str]) -> Output {
    Command::new(checkout().join("plugin/scripts/bridge-screen"))
        .args(args)
        .env_clear()
        .env("PATH", path)
        .env("HOME", data)
        .env("CLAUDE_PLUGIN_DATA", data)
        .env(
            "BRIDGE_SCREEN_RELEASE_BASE",
            format!("file://{}", release.display()),
        )
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

#[test]
fn launcher_downloads_verifies_and_caches() {
    let release = fake_release(true);
    let data = tempfile::tempdir().unwrap();
    let out = launch(release.path(), data.path(), SYSTEM_PATH, &["--version"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout(&out), "downloaded bridge-screen --version\n");
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        data.path()
            .join(format!("bin/bridge-screen-{version}"))
            .is_file()
    );

    // Second run uses the cached binary: no release needed.
    let gone = tempfile::tempdir().unwrap();
    let out = launch(gone.path(), data.path(), SYSTEM_PATH, &["hook", "stop"]);
    assert_eq!(stdout(&out), "downloaded bridge-screen hook stop\n");
}

#[test]
fn launcher_refuses_a_bad_checksum() {
    let release = fake_release(false);
    let data = tempfile::tempdir().unwrap();

    // Hook mode: never break the session.
    let out = launch(release.path(), data.path(), SYSTEM_PATH, &["hook", "stop"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
    let log = std::fs::read_to_string(data.path().join("launcher.log")).unwrap();
    assert!(log.contains("checksum"), "{log}");

    // Anything else: a clear error.
    let out = launch(release.path(), data.path(), SYSTEM_PATH, &["--version"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("checksum"), "{err}");
    assert!(err.contains("brew install"), "{err}");
    let version = env!("CARGO_PKG_VERSION");
    assert!(
        !data
            .path()
            .join(format!("bin/bridge-screen-{version}"))
            .exists()
    );
}

#[test]
fn launcher_prefers_a_matching_binary_on_path() {
    let version = env!("CARGO_PKG_VERSION");
    let bin = tempfile::tempdir().unwrap();
    write_script(
        &bin.path().join("bridge-screen"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'bridge-screen {version}'; else echo \"path bridge-screen $*\"; fi\n"
        ),
    );
    let data = tempfile::tempdir().unwrap();
    let nowhere = tempfile::tempdir().unwrap();
    let path = format!("{}:{SYSTEM_PATH}", bin.path().display());
    let out = launch(nowhere.path(), data.path(), &path, &["status"]);
    assert_eq!(stdout(&out), "path bridge-screen status\n");

    // A different version on PATH is ignored in favour of the matching download.
    write_script(
        &bin.path().join("bridge-screen"),
        "#!/bin/sh\necho 'bridge-screen 0.0.1'\n",
    );
    let release = fake_release(true);
    let out = launch(release.path(), data.path(), &path, &["status"]);
    assert_eq!(stdout(&out), "downloaded bridge-screen status\n");
}

/// A PATH where `uname -s` says this is Git Bash on Windows.
fn windows_path() -> (tempfile::TempDir, String) {
    let fake = tempfile::tempdir().unwrap();
    write_script(
        &fake.path().join("uname"),
        "#!/bin/sh\nif [ \"$1\" = -s ]; then echo MINGW64_NT-10.0-19045; else echo x86_64; fi\n",
    );
    let path = format!("{}:{SYSTEM_PATH}", fake.path().display());
    (fake, path)
}

#[test]
fn launcher_on_windows_hook_exits_zero_and_logs() {
    let (_fake, path) = windows_path();
    let release = fake_release(true);
    let data = tempfile::tempdir().unwrap();
    let out = launch(release.path(), data.path(), &path, &["hook", "stop"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    let log = std::fs::read_to_string(data.path().join("launcher.log")).unwrap();
    assert!(log.contains("doesn't support Windows"), "{log}");
}

#[test]
fn launcher_on_windows_run_explains_install() {
    let (_fake, path) = windows_path();
    let release = fake_release(true);
    let data = tempfile::tempdir().unwrap();
    let out = launch(release.path(), data.path(), &path, &["--version"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        err.trim(),
        "bridge-screen's plugin launcher doesn't support Windows yet. Install the program with the PowerShell installer from the release page and make sure `bridge-screen` is on your PATH, then run `bridge-screen install`."
    );
    assert!(!data.path().join("bin").exists(), "nothing was downloaded");
}

#[test]
fn launcher_still_uses_matching_binary_on_path_on_windows() {
    let (fake, _) = windows_path();
    let version = env!("CARGO_PKG_VERSION");
    write_script(
        &fake.path().join("bridge-screen"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'bridge-screen {version}'; else echo \"path bridge-screen $*\"; fi\n"
        ),
    );
    let path = format!("{}:{SYSTEM_PATH}", fake.path().display());
    let data = tempfile::tempdir().unwrap();
    let nowhere = tempfile::tempdir().unwrap();
    let out = launch(nowhere.path(), data.path(), &path, &["status"]);
    assert_eq!(stdout(&out), "path bridge-screen status\n");
    let out = launch(nowhere.path(), data.path(), &path, &["hook", "stop"]);
    assert_eq!(stdout(&out), "path bridge-screen hook stop\n");
}

#[test]
fn launcher_hook_mode_survives_a_bare_environment() {
    // No HOME, no CLAUDE_PLUGIN_DATA, nothing to download: still exit 0 and print nothing.
    let out = Command::new(checkout().join("plugin/scripts/bridge-screen"))
        .args(["hook", "stop"])
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .env("BRIDGE_SCREEN_RELEASE_BASE", "file:///nonexistent")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout(&out), "");
}
