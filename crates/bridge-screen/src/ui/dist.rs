//! Which folder the dashboard's assets are embedded from (shared by `build.rs`): the
//! built SPA when there is one, else a folder holding one page that says how to build it.
//! The crate always compiles, from a checkout without a web build or from crates.io.

use std::path::{Path, PathBuf};

/// The page served when the dashboard wasn't built.
pub const PLACEHOLDER: &str = "<!doctype html>
<html lang=\"en\"><head><meta charset=\"utf-8\"><title>bridge-screen</title>
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
<style>body{font:15px/1.5 system-ui,sans-serif;background:#1f1e1d;color:#ece9e3;margin:3rem auto;max-width:40rem;padding:0 1rem}
code{font-family:ui-monospace,Menlo,monospace;background:#2a2927;padding:.1rem .3rem;border-radius:4px}
.mark{color:#d97757}</style></head>
<body><h1><span class=\"mark\">✱</span> bridge-screen</h1>
<p>This binary was built without the dashboard. Build it, then rebuild bridge-screen:</p>
<p><code>npm --prefix web ci &amp;&amp; npm --prefix web run build</code></p>
<p>The JSON API under <code>/api/</code> works without it.</p></body></html>
";

/// `built` when `built/<index>` exists; otherwise `fallback`, created holding a
/// placeholder `<index>`.
pub fn choose(built: &Path, index: &str, fallback: &Path) -> std::io::Result<PathBuf> {
    if built.join(index).is_file() {
        return Ok(built.to_path_buf());
    }
    std::fs::create_dir_all(fallback)?;
    std::fs::write(fallback.join(index), PLACEHOLDER)?;
    Ok(fallback.to_path_buf())
}
