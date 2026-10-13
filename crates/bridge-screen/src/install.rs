//! Registering the Claude Code plugin through the `claude` CLI (spec §4.1, §16.2).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// The published marketplace (a GitHub repo holding `.claude-plugin/marketplace.json`).
pub const MARKETPLACE: &str = "bedantsharma/session-plugin";

/// `<plugin>@<marketplace>`.
pub const PLUGIN_ID: &str = "bridge-screen@bridge-screen";

const MARKETPLACE_FILE: &str = ".claude-plugin/marketplace.json";

/// Add the marketplace and install the plugin. `dev` uses this checkout instead of GitHub.
pub fn register_plugin(dev: bool) -> Result<()> {
    let source = if dev {
        checkout_root()?.to_string_lossy().into_owned()
    } else {
        MARKETPLACE.to_string()
    };
    let Some(claude) = find_on_path("claude") else {
        println!(
            "Claude Code's `claude` command isn't on your PATH. To finish, run these in Claude Code:"
        );
        print_manual_steps(&source);
        return Ok(());
    };
    run_claude(&claude, &["plugin", "marketplace", "add", &source], &source)?;
    run_claude(&claude, &["plugin", "install", PLUGIN_ID], &source)?;
    println!("Registered the bridge-screen plugin with Claude Code ({source}).");
    println!("New Claude Code sessions load it; restart running ones.");
    Ok(())
}

fn print_manual_steps(source: &str) {
    println!("  /plugin marketplace add {source}");
    println!("  /plugin install {PLUGIN_ID}");
}

/// Run one `claude plugin …` step. "Already added/installed" counts as success.
fn run_claude(claude: &Path, args: &[&str], source: &str) -> Result<()> {
    let out = Command::new(claude)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", claude.display()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() || text.to_lowercase().contains("already") {
        return Ok(());
    }
    println!("To finish by hand, run these in Claude Code:");
    print_manual_steps(source);
    bail!("`claude {}` failed: {}", args.join(" "), text.trim())
}

/// The checkout holding the marketplace: searched upward from the current directory, then
/// the checkout this binary was built from.
fn checkout_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    cwd.ancestors()
        .map(Path::to_path_buf)
        .chain(std::iter::once(built_from))
        .find(|dir| dir.join(MARKETPLACE_FILE).is_file())
        .map(|dir| std::fs::canonicalize(&dir).unwrap_or(dir))
        .context("--dev needs a bridge-screen checkout (a folder with .claude-plugin/marketplace.json); run it from inside one")
}

/// The first executable called `name` on PATH.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file() || path.with_extension("exe").is_file() || path.with_extension("cmd").is_file()
}
