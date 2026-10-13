# Sub-project 1: Core Store and Plan — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build bridge-screen's foundation: the SQLite event store with append-only events and views kept in the same transaction, repo identity shared across worktrees, the plan tree with validated atomic `plan_edit` batches, view rebuild, a CLI to install/show/edit/rebuild, and a release pipeline that produces installable builds.

**Architecture:** One Cargo workspace. `bridge-screen-core` (library) holds `paths`, `config`, `repo`, `store` (schema, migrations, event append, rebuild), `projection` (routes each event to the module that updates its view tables) and `plan` (model, projector, queries, ops, rendering). `bridge-screen` (binary) is a thin clap CLI over the core. Every state change is an `events` row; view tables (`plan_nodes`, `plan_deps`) are updated in the same `BEGIN IMMEDIATE` transaction and can be rebuilt by replaying events in `pos` order.

**Tech Stack:** Rust 1.99 (edition 2024), rusqlite 0.40.2 (`bundled`), ulid 3.0.0, jiff 0.2.38, serde 1.0.229, serde_json 1.0.154, toml 1.1.8, sha2 0.11.1, thiserror 2.0.21, clap 4.6.7, anyhow 1.0.104, tempfile 3.27.0 (dev). Release: cargo-dist 0.32.0, maturin 1.15.0.

**Spec:** `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` (sections 4, 5.1–5.3, 6.1–6.3, 13 item 1, 16)

## Global Constraints

- Rust toolchain pinned by `rust-toolchain.toml` to `1.99.0`; edition 2024; `unsafe_code = "forbid"`.
- Clippy `all` + `pedantic` at warn, and the hooks/CI run clippy with `-D warnings`. Every task must leave `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Unsafe code is forbidden, so tests must never call `std::env::set_var`; pass environment to subprocesses instead.
- Data lives outside any repo: `~/.bridge-screen/` by default, overridable with the `BRIDGE_SCREEN_HOME` environment variable (used by all tests).
- Database file: `<home>/bridge-screen.db`; config: `<home>/config.toml`.
- SQLite: WAL mode, `busy_timeout` 2000 ms, writes in `BEGIN IMMEDIATE` transactions.
- `events` rows are never updated or deleted (enforced by triggers).
- Event envelope fields: `id` (ULID), `v`, `ts` (UTC RFC 3339), `repo_id`, `session_id`, `seq` (per session, starting at 1), `kind`, `actor`, `task_id`, `data`.
- `repo_id` = first 16 hex characters of SHA-256 of the canonicalized absolute path from `git rev-parse --path-format=absolute --git-common-dir`.
- Plan node kinds: goal → milestone | task; milestone → task; task → subtask. Intent required on goal and milestones. Ids are stable and never reused; the goal is `G`, its children `1`, `2`, …, deeper nodes `<parent>.<n>`.
- Events written by the CLI use session id `owner`.
- The pre-commit hook (fmt, clippy) and pre-push hook (tests, web build) must pass; never use `--no-verify`.
- Commit messages end with: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`

## Review Focus

1. **Concurrent writers:** several processes appending at once (many sessions' hooks) must lose no events and never duplicate a `(session_id, seq)`. Pinned by `concurrent_appends_keep_every_event_and_unique_seq` (Task 3).
2. **Partial batches:** a `plan_edit` batch whose 3rd op is invalid must write nothing, including the first two ops. Pinned by `invalid_op_rolls_back_whole_batch` (Task 5) and the CLI test `invalid_batch_changes_nothing` (Task 8).
3. **Rebuild drift:** views rebuilt from events must equal the incrementally maintained views exactly. Pinned by `rebuild_reproduces_incremental_views` (Task 6).
4. **Worktrees:** running from a linked worktree must resolve the same `repo_id` as the main checkout. Pinned by `linked_worktree_has_same_repo_id` (Task 2).
5. **Wrong place / newer database:** running outside a git repo must say so clearly, and opening a database written by a newer schema must refuse with an upgrade message. Pinned by `outside_git_repo_reports_clearly` (Task 8) and `newer_schema_is_refused` (Task 3).

---

## File Structure

```
Cargo.toml                                   # workspace (modify: lint allows)
crates/bridge-screen-core/
  Cargo.toml                                 # deps
  src/lib.rs                                 # module declarations, VERSION
  src/error.rs                               # Error, Result
  src/paths.rs                               # Home: data folder resolution
  src/config.rs                              # Config: load, defaults, write default
  src/repo.rs                                # RepoInfo, identify(), repo_id_for()
  src/store/mod.rs                           # Store: open, migrate, write, register_repo, append, rebuild
  src/store/schema.rs                        # SQL for schema v1
  src/store/event.rs                         # Actor, EventDraft, Event, append_in, read_all
  src/projection.rs                          # VIEW_TABLES, apply(): routes events to projectors
  src/plan/mod.rs                            # re-exports
  src/plan/model.rs                          # NodeKind, NodeStatus, Hill, PlanNode
  src/plan/project.rs                        # plan event data types + projector
  src/plan/query.rs                          # get_node, load_tree, next_child, would_cycle
  src/plan/ops.rs                            # PlanOp, PlanError, EditContext, plan_edit
  src/plan/render.rs                         # render_text
  tests/store_concurrency.rs                 # multi-process-style append test
  tests/rebuild.rs                           # rebuild equivalence
crates/bridge-screen/
  Cargo.toml                                 # deps
  src/main.rs                                # entry, exit codes
  src/cli.rs                                 # clap definitions
  src/commands.rs                            # command implementations
  tests/cli.rs                               # end-to-end CLI tests
  pyproject.toml                             # maturin packaging for PyPI
dist-workspace.toml                          # cargo-dist config
.github/workflows/release.yml                # generated by cargo-dist
.github/workflows/pypi.yml                   # wheels to PyPI
docs/RELEASING.md                            # owner's release steps and one-time setup
```

---

### Task 1: Core foundations — errors, data folder, config

**Files:**
- Modify: `Cargo.toml` (workspace lints)
- Modify: `crates/bridge-screen-core/Cargo.toml`
- Modify: `crates/bridge-screen-core/src/lib.rs`
- Create: `crates/bridge-screen-core/src/error.rs`
- Create: `crates/bridge-screen-core/src/paths.rs`
- Create: `crates/bridge-screen-core/src/config.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `bridge_screen_core::error::{Error, Result}` — `Result<T, E = Error>`. Variants: `Db(rusqlite::Error)`, `Json(serde_json::Error)`, `Io(std::io::Error)`, `Config(String)`, `NotARepo(String)`, `Git(String)`, `SchemaTooNew { found: i64, supported: i64 }`, `NoHome`. (Task 5 adds `Plan(PlanError)`.)
  - `bridge_screen_core::paths::{Home, HOME_ENV}` — `Home::resolve() -> Result<Home>`, `Home::at(impl Into<PathBuf>) -> Home`, `root(&self) -> &Path`, `db_path(&self) -> PathBuf`, `config_path(&self) -> PathBuf`, `ensure(&self) -> Result<()>`.
  - `bridge_screen_core::config::Config { points_minutes: u32, crash_timeout_min: u32 }` — `Default` (15, 30), `load(&Path) -> Result<Config>`, `write_default_if_missing(&Path) -> Result<bool>`, `to_toml(&self) -> Result<String>`.

- [ ] **Step 1: Add dependencies and lint allows**

In `Cargo.toml` (workspace root), add two lines to `[workspace.lints.clippy]` after `missing_panics_doc`:

```toml
must_use_candidate = "allow"
doc_markdown = "allow"
```

Replace the `[dependencies]` section of `crates/bridge-screen-core/Cargo.toml` with:

```toml
[dependencies]
jiff = "0.2.38"
rusqlite = { version = "0.40.2", features = ["bundled"] }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.154"
sha2 = "0.11.1"
thiserror = "2.0.21"
toml = "1.1.8"
ulid = "3.0.0"

[dev-dependencies]
tempfile = "3.27.0"
```

- [ ] **Step 2: Write the failing tests**

Create `crates/bridge-screen-core/src/paths.rs` with only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::Home;
    use std::path::Path;

    #[test]
    fn layout_is_under_root() {
        let home = Home::at("/tmp/bs-home");
        assert_eq!(home.root(), Path::new("/tmp/bs-home"));
        assert_eq!(home.db_path(), Path::new("/tmp/bs-home/bridge-screen.db"));
        assert_eq!(home.config_path(), Path::new("/tmp/bs-home/config.toml"));
    }

    #[test]
    fn ensure_creates_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path().join("nested/home"));
        home.ensure().unwrap();
        assert!(home.root().is_dir());
    }
}
```

Create `crates/bridge-screen-core/src/config.rs` with only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::Config;
    use crate::error::Error;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(cfg, Config { points_minutes: 15, crash_timeout_min: 30 });
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "points_minutes = 20\n").unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg, Config { points_minutes: 20, crash_timeout_min: 30 });
    }

    #[test]
    fn invalid_file_is_a_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "points_minutes = \"lots\"\n").unwrap();
        assert!(matches!(Config::load(&path), Err(Error::Config(_))));
    }

    #[test]
    fn write_default_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(Config::write_default_if_missing(&path).unwrap());
        std::fs::write(&path, "points_minutes = 9\n").unwrap();
        assert!(!Config::write_default_if_missing(&path).unwrap());
        assert_eq!(Config::load(&path).unwrap().points_minutes, 9);
    }

    #[test]
    fn to_toml_round_trips() {
        let cfg = Config { points_minutes: 12, crash_timeout_min: 45 };
        let text = cfg.to_toml().unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
    }
}
```

Replace `crates/bridge-screen-core/src/lib.rs` with:

```rust
//! Core library for bridge-screen.
//!
//! See `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` §4.2 for the module map.

pub mod config;
pub mod error;
pub mod paths;

/// Crate version, shared by the CLI and the plugin manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_semver() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p bridge-screen-core`
Expected: compile errors — `error` module file missing, `Home` and `Config` not found.

- [ ] **Step 4: Write the implementation**

Create `crates/bridge-screen-core/src/error.rs`:

```rust
use thiserror::Error;

/// Every error the core library can return.
#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("not inside a git repository: {0}")]
    NotARepo(String),
    #[error("git failed: {0}")]
    Git(String),
    #[error(
        "database schema version {found} is newer than this bridge-screen supports ({supported}); upgrade bridge-screen"
    )]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("could not find a home directory; set BRIDGE_SCREEN_HOME")]
    NoHome,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
```

Prepend to `crates/bridge-screen-core/src/paths.rs` (above the tests module):

```rust
//! Where bridge-screen keeps its data (spec §6.1). Never inside a repo.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Environment variable that overrides the data folder (used by tests).
pub const HOME_ENV: &str = "BRIDGE_SCREEN_HOME";

/// The bridge-screen data folder and the files inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// `$BRIDGE_SCREEN_HOME` if set and non-empty, otherwise `~/.bridge-screen`.
    pub fn resolve() -> Result<Self> {
        if let Some(value) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::at(PathBuf::from(value)));
        }
        let home = std::env::home_dir().ok_or(Error::NoHome)?;
        Ok(Self::at(home.join(".bridge-screen")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("bridge-screen.db")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Create the data folder if it doesn't exist.
    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        Ok(())
    }
}
```

Prepend to `crates/bridge-screen-core/src/config.rs` (above the tests module):

```rust
//! User settings in `<home>/config.toml`. Missing keys fall back to defaults.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Agent-minutes per story point, for display only (spec §2).
    pub points_minutes: u32,
    /// Minutes of silence after which a session counts as crashed (spec §8.2).
    pub crash_timeout_min: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self { points_minutes: 15, crash_timeout_min: 30 }
    }
}

impl Config {
    /// Read the config file; a missing file means all defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| Error::Config(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Write a default config file unless one exists. Returns whether it wrote one.
    pub fn write_default_if_missing(path: &Path) -> Result<bool> {
        if path.exists() {
            return Ok(false);
        }
        let body = format!("# bridge-screen settings\n{}", Self::default().to_toml()?);
        std::fs::write(path, body)?;
        Ok(true)
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string(self).map_err(|e| Error::Config(e.to_string()))
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p bridge-screen-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 8 tests pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/bridge-screen-core
git commit -m "core: errors, data folder resolution, config

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Repo identity shared across worktrees

**Files:**
- Create: `crates/bridge-screen-core/src/repo.rs`
- Modify: `crates/bridge-screen-core/src/lib.rs` (add `pub mod repo;`)

**Interfaces:**
- Consumes: `error::{Error, Result}` (Task 1).
- Produces: `bridge_screen_core::repo::{RepoInfo, identify, repo_id_for}` —
  - `pub struct RepoInfo { pub id: String, pub name: String, pub root: PathBuf }` (derives `Debug, Clone, PartialEq, Eq`)
  - `pub fn identify(dir: &Path) -> Result<RepoInfo>` — `Error::NotARepo` outside git.
  - `pub fn repo_id_for(common_dir: &Path) -> String` — 16 lowercase hex chars.

- [ ] **Step 1: Write the failing tests**

Create `crates/bridge-screen-core/src/repo.rs` with only the tests:

```rust
#[cfg(test)]
mod tests {
    use super::{identify, repo_id_for};
    use crate::error::Error;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn new_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
        dir
    }

    #[test]
    fn id_is_16_hex_chars_and_stable() {
        let a = repo_id_for(Path::new("/x/.git"));
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(a, repo_id_for(Path::new("/x/.git")));
        assert_ne!(a, repo_id_for(Path::new("/y/.git")));
    }

    #[test]
    fn subdirectory_resolves_to_same_repo() {
        let repo = new_repo();
        std::fs::create_dir(repo.path().join("sub")).unwrap();
        let top = identify(repo.path()).unwrap();
        let sub = identify(&repo.path().join("sub")).unwrap();
        assert_eq!(top, sub);
        assert_eq!(top.name, repo.path().file_name().unwrap().to_string_lossy());
    }

    #[test]
    fn linked_worktree_has_same_repo_id() {
        let repo = new_repo();
        let wt_parent = tempfile::tempdir().unwrap();
        let wt = wt_parent.path().join("wt");
        git(repo.path(), &["worktree", "add", "-q", "-b", "feature", wt.to_str().unwrap()]);
        let main = identify(repo.path()).unwrap();
        let linked = identify(&wt).unwrap();
        assert_eq!(main.id, linked.id);
        assert_eq!(main.name, linked.name);
        assert_eq!(main.root, linked.root);
    }

    #[test]
    fn outside_git_is_not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(identify(dir.path()), Err(Error::NotARepo(_))));
    }
}
```

Add `pub mod repo;` to `lib.rs` after `pub mod paths;`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p bridge-screen-core repo`
Expected: compile error — `identify` and `repo_id_for` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/bridge-screen-core/src/repo.rs`:

```rust
//! Which repo a directory belongs to. All worktrees of a repo share one id (spec §6.1).

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// 16 hex chars derived from git's shared directory.
    pub id: String,
    /// Folder name of the main checkout.
    pub name: String,
    /// Path of the main checkout.
    pub root: PathBuf,
}

/// Identify the repo that contains `dir`.
pub fn identify(dir: &Path) -> Result<RepoInfo> {
    let common = git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let toplevel = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?);
    let common = std::fs::canonicalize(&common)?;
    let root = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().map_or_else(|| toplevel.clone(), Path::to_path_buf)
    } else {
        toplevel
    };
    let name = root
        .file_name()
        .map_or_else(|| "repo".to_string(), |n| n.to_string_lossy().into_owned());
    Ok(RepoInfo { id: repo_id_for(&common), name, root })
}

/// First 16 hex chars of SHA-256 over the git common directory path.
pub fn repo_id_for(common_dir: &Path) -> String {
    let digest = Sha256::digest(common_dir.to_string_lossy().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Git(format!("could not run git: {e}")))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if msg.contains("not a git repository") {
            return Err(Error::NotARepo(dir.display().to_string()));
        }
        return Err(Error::Git(msg));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p bridge-screen-core repo && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 4 tests pass; clippy clean. (On macOS, temp paths go through `/private/var`; `canonicalize` makes main and worktree agree.)

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-screen-core
git commit -m "core: repo identity shared across worktrees

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Event store — schema, migrations, append-only events

**Files:**
- Create: `crates/bridge-screen-core/src/store/mod.rs`
- Create: `crates/bridge-screen-core/src/store/schema.rs`
- Create: `crates/bridge-screen-core/src/store/event.rs`
- Create: `crates/bridge-screen-core/src/projection.rs`
- Create: `crates/bridge-screen-core/tests/store_concurrency.rs`
- Modify: `crates/bridge-screen-core/src/lib.rs` (add `pub mod projection;` and `pub mod store;`)

**Interfaces:**
- Consumes: `error::{Error, Result}`, `repo::RepoInfo`.
- Produces:
  - `store::Store` — `open(&Path) -> Result<Store>`, `open_in_memory() -> Result<Store>`, `conn(&self) -> &rusqlite::Connection`, `write<T>(&mut self, f: impl FnOnce(&rusqlite::Transaction) -> Result<T>) -> Result<T>` (one `BEGIN IMMEDIATE` transaction), `register_repo(&self, &RepoInfo) -> Result<()>`, `append(&mut self, EventDraft) -> Result<Event>`.
  - `store::SCHEMA_VERSION: i64 = 1`.
  - `store::event::{Actor, EventDraft, Event, EVENT_VERSION, OWNER_SESSION, append_in, read_all}`, re-exported from `store` as `store::{Actor, EventDraft, Event, OWNER_SESSION, append_in, read_all}`.
    - `Actor { agent_id: Option<String>, agent_type: Option<String>, model: Option<String> }` (Default, serde).
    - `EventDraft { repo_id: String, session_id: String, kind: String, actor: Actor, task_id: Option<String>, data: serde_json::Value }`; `EventDraft::new(repo_id, session_id, kind, data)`, builder `.task(id)`, `.actor(actor)`.
    - `Event { pos: i64, id: String, v: i64, ts: String, repo_id: String, session_id: String, seq: i64, kind: String, actor: Actor, task_id: Option<String>, data: serde_json::Value }`.
    - `pub fn append_in(tx: &rusqlite::Transaction, draft: EventDraft) -> Result<Event>` — inserts and runs `projection::apply` in the same transaction.
    - `pub fn read_all(conn: &rusqlite::Connection, repo_id: Option<&str>) -> Result<Vec<Event>>` — ordered by `pos`.
  - `projection::{VIEW_TABLES, apply}` — `pub const VIEW_TABLES: &[&str]`; `pub fn apply(tx: &rusqlite::Transaction, event: &Event) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/bridge-screen-core/src/store/mod.rs` with only the module declarations and tests:

```rust
pub mod event;
pub mod schema;

#[cfg(test)]
mod tests {
    use super::{EventDraft, Store, read_all};
    use crate::error::Error;
    use crate::repo::RepoInfo;
    use serde_json::json;

    #[test]
    fn open_creates_schema_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        Store::open(&path).unwrap();
        let store = Store::open(&path).unwrap();
        let version: String = store
            .conn()
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, "1");
    }

    #[test]
    fn newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .conn()
            .execute("UPDATE meta SET value = '99' WHERE key = 'schema_version'", [])
            .unwrap();
        drop(store);
        assert!(matches!(
            Store::open(&path),
            Err(Error::SchemaTooNew { found: 99, supported: 1 })
        ));
    }

    #[test]
    fn append_assigns_seq_per_session() {
        let mut store = Store::open_in_memory().unwrap();
        let a1 = store.append(EventDraft::new("r", "s1", "note", json!({"n": 1}))).unwrap();
        let a2 = store.append(EventDraft::new("r", "s1", "note", json!({"n": 2}))).unwrap();
        let b1 = store.append(EventDraft::new("r", "s2", "note", json!({}))).unwrap();
        assert_eq!((a1.seq, a2.seq, b1.seq), (1, 2, 1));
        assert!(a1.pos < a2.pos && a2.pos < b1.pos);
        assert_eq!(a1.id.len(), 26);
        assert!(a1.ts.ends_with('Z'));
        let all = read_all(store.conn(), Some("r")).unwrap();
        assert_eq!(all, vec![a1, a2, b1]);
    }

    #[test]
    fn events_cannot_be_updated_or_deleted() {
        let mut store = Store::open_in_memory().unwrap();
        store.append(EventDraft::new("r", "s", "note", json!({}))).unwrap();
        let upd = store.conn().execute("UPDATE events SET kind = 'x'", []);
        let del = store.conn().execute("DELETE FROM events", []);
        assert!(upd.unwrap_err().to_string().contains("append-only"));
        assert!(del.unwrap_err().to_string().contains("append-only"));
    }

    #[test]
    fn register_repo_upserts() {
        let store = Store::open_in_memory().unwrap();
        let mut repo = RepoInfo { id: "abc".into(), name: "one".into(), root: "/a".into() };
        store.register_repo(&repo).unwrap();
        repo.name = "two".into();
        store.register_repo(&repo).unwrap();
        let (count, name): (i64, String) = store
            .conn()
            .query_row("SELECT count(*), max(name) FROM repos", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((count, name.as_str()), (1, "two"));
    }
}
```

Create `crates/bridge-screen-core/tests/store_concurrency.rs`:

```rust
use std::collections::BTreeSet;
use std::thread;

use bridge_screen_core::store::{EventDraft, Store, read_all};
use serde_json::json;

#[test]
fn concurrent_appends_keep_every_event_and_unique_seq() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    Store::open(&path).unwrap();

    let handles: Vec<_> = (0..4)
        .map(|t| {
            let path = path.clone();
            thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                for i in 0..50 {
                    store
                        .append(EventDraft::new("r", "shared", "note", json!({"t": t, "i": i})))
                        .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let store = Store::open(&path).unwrap();
    let events = read_all(store.conn(), None).unwrap();
    assert_eq!(events.len(), 200);
    let seqs: BTreeSet<i64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=200).collect());
}
```

Add to `lib.rs` after `pub mod paths;`: `pub mod projection;` and after `pub mod repo;`: `pub mod store;`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p bridge-screen-core store`
Expected: compile errors — `Store`, `EventDraft`, `read_all`, `projection` not found.

- [ ] **Step 3: Write the schema**

Create `crates/bridge-screen-core/src/store/schema.rs`:

```rust
//! SQL schema. Version 1 is the first released schema; later versions append migrations.

pub const SCHEMA_VERSION: i64 = 1;

pub const V1: &str = r"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE repos (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    root       TEXT NOT NULL,
    settings   TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE TABLE events (
    pos        INTEGER PRIMARY KEY AUTOINCREMENT,
    id         TEXT NOT NULL UNIQUE,
    v          INTEGER NOT NULL,
    ts         TEXT NOT NULL,
    repo_id    TEXT NOT NULL,
    session_id TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    actor      TEXT NOT NULL DEFAULT '{}',
    task_id    TEXT,
    data       TEXT NOT NULL DEFAULT '{}',
    UNIQUE (session_id, seq)
);

CREATE INDEX events_repo_pos ON events (repo_id, pos);

CREATE TRIGGER events_no_update BEFORE UPDATE ON events
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;

CREATE TRIGGER events_no_delete BEFORE DELETE ON events
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;
";
```

- [ ] **Step 4: Write events and the projection router**

Create `crates/bridge-screen-core/src/store/event.rs`:

```rust
//! The event envelope (spec §5.1) and appending events.

use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// Schema version of event `data` payloads.
pub const EVENT_VERSION: i64 = 1;

/// Session id used for events the owner causes (CLI, dashboard).
pub const OWNER_SESSION: &str = "owner";

/// Which agent or subagent caused an event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// An event before it is stored: no id, timestamp, position or sequence yet.
#[derive(Debug, Clone, PartialEq)]
pub struct EventDraft {
    pub repo_id: String,
    pub session_id: String,
    pub kind: String,
    pub actor: Actor,
    pub task_id: Option<String>,
    pub data: Value,
}

impl EventDraft {
    pub fn new(
        repo_id: impl Into<String>,
        session_id: impl Into<String>,
        kind: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            repo_id: repo_id.into(),
            session_id: session_id.into(),
            kind: kind.into(),
            actor: Actor::default(),
            task_id: None,
            data,
        }
    }

    #[must_use]
    pub fn task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    #[must_use]
    pub fn actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }
}

/// A stored event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub pos: i64,
    pub id: String,
    pub v: i64,
    pub ts: String,
    pub repo_id: String,
    pub session_id: String,
    pub seq: i64,
    pub kind: String,
    pub actor: Actor,
    pub task_id: Option<String>,
    pub data: Value,
}

/// Append one event and update the views it affects, inside the caller's transaction.
pub fn append_in(tx: &Transaction, draft: EventDraft) -> Result<Event> {
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
        [&draft.session_id],
        |r| r.get(0),
    )?;
    let id = ulid::Ulid::generate().to_string();
    let ts = jiff::Timestamp::now().to_string();
    tx.execute(
        "INSERT INTO events (id, v, ts, repo_id, session_id, seq, kind, actor, task_id, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            EVENT_VERSION,
            ts,
            draft.repo_id,
            draft.session_id,
            seq,
            draft.kind,
            serde_json::to_string(&draft.actor)?,
            draft.task_id,
            serde_json::to_string(&draft.data)?,
        ],
    )?;
    let event = Event {
        pos: tx.last_insert_rowid(),
        id,
        v: EVENT_VERSION,
        ts,
        repo_id: draft.repo_id,
        session_id: draft.session_id,
        seq,
        kind: draft.kind,
        actor: draft.actor,
        task_id: draft.task_id,
        data: draft.data,
    };
    crate::projection::apply(tx, &event)?;
    Ok(event)
}

/// All events (optionally for one repo), in the order they were appended.
pub fn read_all(conn: &Connection, repo_id: Option<&str>) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT pos, id, v, ts, repo_id, session_id, seq, kind, actor, task_id, data
         FROM events WHERE (?1 IS NULL OR repo_id = ?1) ORDER BY pos",
    )?;
    let rows = stmt.query_map([repo_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, Option<String>>(9)?,
            r.get::<_, String>(10)?,
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (pos, id, v, ts, repo_id, session_id, seq, kind, actor, task_id, data) = row?;
        events.push(Event {
            pos,
            id,
            v,
            ts,
            repo_id,
            session_id,
            seq,
            kind,
            actor: serde_json::from_str(&actor)?,
            task_id,
            data: serde_json::from_str(&data)?,
        });
    }
    Ok(events)
}
```

Create `crates/bridge-screen-core/src/projection.rs`:

```rust
//! Routes each stored event to the module that keeps its view tables up to date.

use rusqlite::Transaction;

use crate::error::Result;
use crate::store::Event;

/// View tables that `Store::rebuild` empties and refills from events.
pub const VIEW_TABLES: &[&str] = &[];

/// Update view tables for one event. Unknown kinds change no views.
#[allow(clippy::unnecessary_wraps)] // Task 4 routes plan events here.
pub fn apply(_tx: &Transaction, _event: &Event) -> Result<()> {
    Ok(())
}
```

- [ ] **Step 5: Write the Store**

Prepend to `crates/bridge-screen-core/src/store/mod.rs` (keep the `pub mod` lines and the tests):

```rust
//! The SQLite store: the append-only event log plus view tables (spec §5–6).

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{Error, Result};
use crate::repo::RepoInfo;

pub use event::{Actor, EVENT_VERSION, Event, EventDraft, OWNER_SESSION, append_in, read_all};
pub use schema::SCHEMA_VERSION;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_millis(2000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Run `f` inside one `BEGIN IMMEDIATE` transaction; commit only if it succeeds.
    pub fn write<T>(&mut self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Append one event (and update its views) in its own transaction.
    pub fn append(&mut self, draft: EventDraft) -> Result<Event> {
        self.write(|tx| append_in(tx, draft))
    }

    /// Record or refresh a repo's name and root.
    pub fn register_repo(&self, repo: &RepoInfo) -> Result<()> {
        self.conn.execute(
            "INSERT INTO repos (id, name, root, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, root = excluded.root",
            params![
                repo.id,
                repo.name,
                repo.root.to_string_lossy(),
                jiff::Timestamp::now().to_string()
            ],
        )?;
        Ok(())
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_meta: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    let current: i64 = if has_meta > 0 {
        tx.query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0)
    } else {
        0
    };
    if current > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew { found: current, supported: SCHEMA_VERSION });
    }
    if current < 1 {
        tx.execute_batch(schema::V1)?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
            [SCHEMA_VERSION.to_string()],
        )?;
    }
    tx.commit()?;
    Ok(())
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p bridge-screen-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all unit tests plus `concurrent_appends_keep_every_event_and_unique_seq` pass; clippy clean.

- [ ] **Step 7: Commit**

```bash
git add crates/bridge-screen-core
git commit -m "core: SQLite event store with append-only events and migrations

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Plan model, plan events and their projector, tree queries

**Files:**
- Create: `crates/bridge-screen-core/src/plan/mod.rs`
- Create: `crates/bridge-screen-core/src/plan/model.rs`
- Create: `crates/bridge-screen-core/src/plan/project.rs`
- Create: `crates/bridge-screen-core/src/plan/query.rs`
- Modify: `crates/bridge-screen-core/src/store/schema.rs` (append plan tables to `V1`)
- Modify: `crates/bridge-screen-core/src/projection.rs` (route plan events, list view tables)
- Modify: `crates/bridge-screen-core/src/lib.rs` (add `pub mod plan;`)

**Interfaces:**
- Consumes: `store::{Event, EventDraft, Store, append_in}`, `error::Result`.
- Produces:
  - `plan::model::{NodeKind, NodeStatus, Hill, PlanNode}` (re-exported from `plan`):
    - `NodeKind { Goal, Milestone, Task, Subtask }`, serde `snake_case`; `as_str(self) -> &'static str`; `parse(&str) -> Option<NodeKind>`; `allows_child(self, child: NodeKind) -> bool`; `requires_intent(self) -> bool`.
    - `NodeStatus { Todo, Active, Claimed, L1, L2, L3, L4, Failed, Dropped }`, serde `snake_case` (`"l1"` … `"l4"`); `as_str`, `parse`.
    - `Hill { Uphill, Downhill }`, serde `snake_case`; `as_str`, `parse`.
    - `PlanNode { id: String, parent_id: Option<String>, kind: NodeKind, title: String, intent: Option<String>, estimate_min: Option<u32>, status: NodeStatus, hill: Option<Hill>, expected_files: Vec<String>, depends_on: Vec<String>, depth: usize }`.
  - `plan::project::{EditMeta, NodeAdded, NodeEdited, NodeMoved, NodeRef, DepChange, IntentSet, EstimateSet, HillMoved, PLAN_KINDS, project}`: serde payloads (each has `#[serde(flatten)] meta: EditMeta` where `EditMeta { reason: Option<String>, replan_id: Option<String> }`); `pub fn project(tx: &Transaction, event: &Event) -> Result<()>`.
  - `plan::query::{get_node, load_tree, next_child, would_cycle, is_in_subtree}` (re-exported from `plan`):
    - `get_node(conn: &Connection, repo_id: &str, id: &str) -> Result<Option<PlanNode>>` (depth 0)
    - `load_tree(conn: &Connection, repo_id: &str, include_dropped: bool) -> Result<Vec<PlanNode>>` (depth-first, siblings by position)
    - `next_child(conn: &Connection, repo_id: &str, id: &str) -> Result<u32>`
    - `would_cycle(conn: &Connection, repo_id: &str, id: &str, depends_on: &str) -> Result<bool>`
    - `is_in_subtree(conn: &Connection, repo_id: &str, root: &str, candidate: &str) -> Result<bool>`

- [ ] **Step 1: Add the plan tables to the schema**

Append inside the `V1` string in `store/schema.rs`, before the closing `";`:

```sql
CREATE TABLE plan_nodes (
    repo_id        TEXT NOT NULL,
    id             TEXT NOT NULL,
    parent_id      TEXT,
    kind           TEXT NOT NULL,
    title          TEXT NOT NULL,
    intent         TEXT,
    estimate_min   INTEGER,
    status         TEXT NOT NULL DEFAULT 'todo',
    hill           TEXT,
    expected_files TEXT NOT NULL DEFAULT '[]',
    source         TEXT NOT NULL DEFAULT 'manual',
    position       INTEGER NOT NULL,
    next_child     INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (repo_id, id)
);

CREATE TABLE plan_deps (
    repo_id    TEXT NOT NULL,
    node_id    TEXT NOT NULL,
    depends_on TEXT NOT NULL,
    PRIMARY KEY (repo_id, node_id, depends_on)
);
```

- [ ] **Step 2: Write the failing tests**

Create `crates/bridge-screen-core/src/plan/mod.rs`:

```rust
//! The plan tree: Goal → Milestones → Tasks → Subtasks (spec §5.3).

pub mod model;
pub mod project;
pub mod query;

pub use model::{Hill, NodeKind, NodeStatus, PlanNode};
pub use query::{get_node, is_in_subtree, load_tree, next_child, would_cycle};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventDraft, Store};
    use serde_json::json;

    fn add(store: &mut Store, id: &str, parent: Option<&str>, kind: &str, title: &str) {
        store
            .append(EventDraft::new(
                "r",
                "owner",
                "node_added",
                json!({"id": id, "parent_id": parent, "kind": kind, "title": title,
                       "intent": if kind == "goal" || kind == "milestone" { Some("why") } else { None },
                       "estimate_min": null, "expected_files": []}),
            ))
            .unwrap();
    }

    fn sample() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        add(&mut s, "G", None, "goal", "Ship it");
        add(&mut s, "1", Some("G"), "milestone", "Core");
        add(&mut s, "1.1", Some("1"), "task", "Schema");
        add(&mut s, "1.2", Some("1"), "task", "Append");
        add(&mut s, "2", Some("G"), "milestone", "Screens");
        s
    }

    #[test]
    fn kind_rules() {
        assert!(NodeKind::Goal.allows_child(NodeKind::Milestone));
        assert!(NodeKind::Goal.allows_child(NodeKind::Task));
        assert!(NodeKind::Milestone.allows_child(NodeKind::Task));
        assert!(NodeKind::Task.allows_child(NodeKind::Subtask));
        assert!(!NodeKind::Task.allows_child(NodeKind::Task));
        assert!(!NodeKind::Subtask.allows_child(NodeKind::Subtask));
        assert!(!NodeKind::Milestone.allows_child(NodeKind::Milestone));
        assert!(NodeKind::Goal.requires_intent() && NodeKind::Milestone.requires_intent());
        assert!(!NodeKind::Task.requires_intent());
        assert_eq!(NodeKind::parse("subtask"), Some(NodeKind::Subtask));
        assert_eq!(NodeStatus::parse(NodeStatus::L2.as_str()), Some(NodeStatus::L2));
        assert_eq!(Hill::parse("uphill"), Some(Hill::Uphill));
    }

    #[test]
    fn tree_is_depth_first_in_position_order() {
        let s = sample();
        let tree = load_tree(s.conn(), "r", false).unwrap();
        let ids: Vec<(&str, usize)> = tree.iter().map(|n| (n.id.as_str(), n.depth)).collect();
        assert_eq!(ids, vec![("G", 0), ("1", 1), ("1.1", 2), ("1.2", 2), ("2", 1)]);
        assert_eq!(next_child(s.conn(), "r", "1").unwrap(), 3);
        assert_eq!(next_child(s.conn(), "r", "G").unwrap(), 3);
    }

    #[test]
    fn edits_moves_and_fields_project() {
        let mut s = sample();
        let ev = |kind: &str, data: serde_json::Value| EventDraft::new("r", "owner", kind, data);
        s.append(ev("node_edited", json!({"id": "1.1", "title": "Schema v1", "expected_files": ["a.rs"]}))).unwrap();
        s.append(ev("estimate_set", json!({"id": "1.1", "estimate_min": 30}))).unwrap();
        s.append(ev("intent_set", json!({"id": "2", "intent": "see it"}))).unwrap();
        s.append(ev("hill_moved", json!({"id": "1", "hill": "downhill"}))).unwrap();
        s.append(ev("node_moved", json!({"id": "1.2", "new_parent_id": "2"}))).unwrap();
        s.append(ev("dep_added", json!({"id": "1.2", "depends_on": "1.1"}))).unwrap();

        let n = get_node(s.conn(), "r", "1.1").unwrap().unwrap();
        assert_eq!(n.title, "Schema v1");
        assert_eq!(n.expected_files, vec!["a.rs".to_string()]);
        assert_eq!(n.estimate_min, Some(30));
        assert_eq!(get_node(s.conn(), "r", "2").unwrap().unwrap().intent.as_deref(), Some("see it"));
        assert_eq!(get_node(s.conn(), "r", "1").unwrap().unwrap().hill, Some(Hill::Downhill));
        let moved = get_node(s.conn(), "r", "1.2").unwrap().unwrap();
        assert_eq!(moved.parent_id.as_deref(), Some("2"));
        assert_eq!(moved.depends_on, vec!["1.1".to_string()]);

        s.append(ev("dep_removed", json!({"id": "1.2", "depends_on": "1.1"}))).unwrap();
        assert!(get_node(s.conn(), "r", "1.2").unwrap().unwrap().depends_on.is_empty());
    }

    #[test]
    fn removal_drops_subtree_and_its_deps() {
        let mut s = sample();
        s.append(EventDraft::new("r", "owner", "dep_added", json!({"id": "2", "depends_on": "1.1"})))
            .unwrap();
        s.append(EventDraft::new("r", "owner", "node_removed", json!({"id": "1"}))).unwrap();
        let live: Vec<String> = load_tree(s.conn(), "r", false).unwrap().into_iter().map(|n| n.id).collect();
        assert_eq!(live, vec!["G", "2"]);
        let all = load_tree(s.conn(), "r", true).unwrap();
        assert!(all.iter().filter(|n| n.id.starts_with('1')).all(|n| n.status == NodeStatus::Dropped));
        assert!(get_node(s.conn(), "r", "2").unwrap().unwrap().depends_on.is_empty());
    }

    #[test]
    fn cycle_and_subtree_checks() {
        let mut s = sample();
        s.append(EventDraft::new("r", "owner", "dep_added", json!({"id": "1.2", "depends_on": "1.1"})))
            .unwrap();
        assert!(would_cycle(s.conn(), "r", "1.1", "1.2").unwrap());
        assert!(!would_cycle(s.conn(), "r", "2", "1.2").unwrap());
        assert!(would_cycle(s.conn(), "r", "1.1", "1.1").unwrap());
        assert!(is_in_subtree(s.conn(), "r", "1", "1.2").unwrap());
        assert!(is_in_subtree(s.conn(), "r", "1", "1").unwrap());
        assert!(!is_in_subtree(s.conn(), "r", "1", "2").unwrap());
    }

    #[test]
    fn other_repos_are_separate() {
        let s = sample();
        assert!(load_tree(s.conn(), "other", true).unwrap().is_empty());
    }
}
```

Add `pub mod plan;` to `lib.rs` after `pub mod paths;`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p bridge-screen-core plan`
Expected: compile errors — `model`, `project`, `query` modules missing.

- [ ] **Step 4: Write the model**

Create `crates/bridge-screen-core/src/plan/model.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Goal,
    Milestone,
    Task,
    Subtask,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Milestone => "milestone",
            Self::Task => "task",
            Self::Subtask => "subtask",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "goal" => Some(Self::Goal),
            "milestone" => Some(Self::Milestone),
            "task" => Some(Self::Task),
            "subtask" => Some(Self::Subtask),
            _ => None,
        }
    }

    /// Goal → milestone | task; milestone → task; task → subtask.
    pub fn allows_child(self, child: Self) -> bool {
        matches!(
            (self, child),
            (Self::Goal, Self::Milestone | Self::Task)
                | (Self::Milestone, Self::Task)
                | (Self::Task, Self::Subtask)
        )
    }

    /// Goals and milestones carry a commander's-intent statement (spec §3).
    pub fn requires_intent(self) -> bool {
        matches!(self, Self::Goal | Self::Milestone)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Todo,
    Active,
    Claimed,
    L1,
    L2,
    L3,
    L4,
    Failed,
    Dropped,
}

impl NodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Active => "active",
            Self::Claimed => "claimed",
            Self::L1 => "l1",
            Self::L2 => "l2",
            Self::L3 => "l3",
            Self::L4 => "l4",
            Self::Failed => "failed",
            Self::Dropped => "dropped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "active" => Some(Self::Active),
            "claimed" => Some(Self::Claimed),
            "l1" => Some(Self::L1),
            "l2" => Some(Self::L2),
            "l3" => Some(Self::L3),
            "l4" => Some(Self::L4),
            "failed" => Some(Self::Failed),
            "dropped" => Some(Self::Dropped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hill {
    Uphill,
    Downhill,
}

impl Hill {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uphill => "uphill",
            Self::Downhill => "downhill",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "uphill" => Some(Self::Uphill),
            "downhill" => Some(Self::Downhill),
            _ => None,
        }
    }
}

/// One node of the plan tree, as read from the views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: NodeKind,
    pub title: String,
    pub intent: Option<String>,
    pub estimate_min: Option<u32>,
    pub status: NodeStatus,
    pub hill: Option<Hill>,
    pub expected_files: Vec<String>,
    pub depends_on: Vec<String>,
    /// Distance from the root in a loaded tree (0 for single-node reads).
    pub depth: usize,
}
```

- [ ] **Step 5: Write the plan events and projector**

Create `crates/bridge-screen-core/src/plan/project.rs`:

```rust
//! Plan event payloads and the projector that applies them to `plan_nodes` / `plan_deps`.

use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use super::model::{Hill, NodeKind};
use crate::error::Result;
use crate::store::Event;

pub const PLAN_KINDS: &[&str] = &[
    "node_added",
    "node_edited",
    "node_moved",
    "node_removed",
    "dep_added",
    "dep_removed",
    "intent_set",
    "estimate_set",
    "hill_moved",
    "replan",
];

/// Why an edit happened; shared by every plan event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replan_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAdded {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: NodeKind,
    pub title: String,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub estimate_min: Option<u32>,
    #[serde(default)]
    pub expected_files: Vec<String>,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeEdited {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub expected_files: Option<Vec<String>>,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeMoved {
    pub id: String,
    pub new_parent_id: String,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRef {
    pub id: String,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepChange {
    pub id: String,
    pub depends_on: String,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSet {
    pub id: String,
    pub intent: String,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstimateSet {
    pub id: String,
    pub estimate_min: u32,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HillMoved {
    pub id: String,
    pub hill: Hill,
    #[serde(flatten)]
    pub meta: EditMeta,
}

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

fn next_position(tx: &Transaction, repo: &str, parent: Option<&str>) -> Result<i64> {
    Ok(tx.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM plan_nodes WHERE repo_id = ?1 AND parent_id IS ?2",
        params![repo, parent],
        |r| r.get(0),
    )?)
}

/// Apply one plan event to the plan views.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    let repo = event.repo_id.as_str();
    match event.kind.as_str() {
        "node_added" => {
            let d: NodeAdded = payload(event)?;
            let position = next_position(tx, repo, d.parent_id.as_deref())?;
            tx.execute(
                "INSERT INTO plan_nodes (repo_id, id, parent_id, kind, title, intent, estimate_min,
                                         expected_files, position)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    repo,
                    d.id,
                    d.parent_id,
                    d.kind.as_str(),
                    d.title,
                    d.intent,
                    d.estimate_min,
                    serde_json::to_string(&d.expected_files)?,
                    position
                ],
            )?;
            if let Some(parent) = &d.parent_id {
                tx.execute(
                    "UPDATE plan_nodes SET next_child = next_child + 1 WHERE repo_id = ?1 AND id = ?2",
                    params![repo, parent],
                )?;
            }
        }
        "node_edited" => {
            let d: NodeEdited = payload(event)?;
            let files = d.expected_files.as_ref().map(serde_json::to_string).transpose()?;
            tx.execute(
                "UPDATE plan_nodes SET title = COALESCE(?3, title),
                                       expected_files = COALESCE(?4, expected_files)
                 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.title, files],
            )?;
        }
        "node_moved" => {
            let d: NodeMoved = payload(event)?;
            let position = next_position(tx, repo, Some(&d.new_parent_id))?;
            tx.execute(
                "UPDATE plan_nodes SET parent_id = ?3, position = ?4 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.new_parent_id, position],
            )?;
        }
        "node_removed" => {
            let d: NodeRef = payload(event)?;
            let subtree = "WITH RECURSIVE sub(id) AS (
                               SELECT ?2
                               UNION ALL
                               SELECT n.id FROM plan_nodes n JOIN sub ON n.parent_id = sub.id
                               WHERE n.repo_id = ?1)";
            tx.execute(
                &format!(
                    "{subtree} UPDATE plan_nodes SET status = 'dropped'
                     WHERE repo_id = ?1 AND id IN (SELECT id FROM sub)"
                ),
                params![repo, d.id],
            )?;
            tx.execute(
                &format!(
                    "{subtree} DELETE FROM plan_deps WHERE repo_id = ?1
                     AND (node_id IN (SELECT id FROM sub) OR depends_on IN (SELECT id FROM sub))"
                ),
                params![repo, d.id],
            )?;
        }
        "dep_added" => {
            let d: DepChange = payload(event)?;
            tx.execute(
                "INSERT OR IGNORE INTO plan_deps (repo_id, node_id, depends_on) VALUES (?1, ?2, ?3)",
                params![repo, d.id, d.depends_on],
            )?;
        }
        "dep_removed" => {
            let d: DepChange = payload(event)?;
            tx.execute(
                "DELETE FROM plan_deps WHERE repo_id = ?1 AND node_id = ?2 AND depends_on = ?3",
                params![repo, d.id, d.depends_on],
            )?;
        }
        "intent_set" => {
            let d: IntentSet = payload(event)?;
            tx.execute(
                "UPDATE plan_nodes SET intent = ?3 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.intent],
            )?;
        }
        "estimate_set" => {
            let d: EstimateSet = payload(event)?;
            tx.execute(
                "UPDATE plan_nodes SET estimate_min = ?3 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.estimate_min],
            )?;
        }
        "hill_moved" => {
            let d: HillMoved = payload(event)?;
            tx.execute(
                "UPDATE plan_nodes SET hill = ?3 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.hill.as_str()],
            )?;
        }
        // `replan` only groups the edits that follow it; it changes no view.
        _ => {}
    }
    Ok(())
}
```

- [ ] **Step 6: Write the queries**

Create `crates/bridge-screen-core/src/plan/query.rs`:

```rust
//! Read-side queries over the plan views.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::model::{Hill, NodeKind, NodeStatus, PlanNode};
use crate::error::Result;

const NODE_COLUMNS: &str =
    "id, parent_id, kind, title, intent, estimate_min, status, hill, expected_files, position";

struct RawNode {
    node: PlanNode,
    position: i64,
}

fn read_row(row: &Row) -> rusqlite::Result<(RawNode, String)> {
    let kind: String = row.get(2)?;
    let status: String = row.get(6)?;
    let hill: Option<String> = row.get(7)?;
    let files: String = row.get(8)?;
    let node = PlanNode {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        kind: NodeKind::parse(&kind).unwrap_or(NodeKind::Task),
        title: row.get(3)?,
        intent: row.get(4)?,
        estimate_min: row.get(5)?,
        status: NodeStatus::parse(&status).unwrap_or(NodeStatus::Todo),
        hill: hill.as_deref().and_then(Hill::parse),
        expected_files: Vec::new(),
        depends_on: Vec::new(),
        depth: 0,
    };
    Ok((RawNode { node, position: row.get(9)? }, files))
}

fn deps_map(conn: &Connection, repo_id: &str) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT node_id, depends_on FROM plan_deps WHERE repo_id = ?1 ORDER BY node_id, depends_on",
    )?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for row in stmt.query_map([repo_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (node, dep) = row?;
        map.entry(node).or_default().push(dep);
    }
    Ok(map)
}

/// One node with its dependencies, or `None` if it doesn't exist.
pub fn get_node(conn: &Connection, repo_id: &str, id: &str) -> Result<Option<PlanNode>> {
    let found = conn
        .query_row(
            &format!("SELECT {NODE_COLUMNS} FROM plan_nodes WHERE repo_id = ?1 AND id = ?2"),
            params![repo_id, id],
            read_row,
        )
        .optional()?;
    let Some((raw, files)) = found else { return Ok(None) };
    let mut node = raw.node;
    node.expected_files = serde_json::from_str(&files)?;
    node.depends_on = deps_map(conn, repo_id)?.remove(id).unwrap_or_default();
    Ok(Some(node))
}

/// The whole tree, depth-first, siblings in position order.
pub fn load_tree(conn: &Connection, repo_id: &str, include_dropped: bool) -> Result<Vec<PlanNode>> {
    let mut stmt =
        conn.prepare(&format!("SELECT {NODE_COLUMNS} FROM plan_nodes WHERE repo_id = ?1"))?;
    let mut deps = deps_map(conn, repo_id)?;
    let mut children: BTreeMap<Option<String>, Vec<RawNode>> = BTreeMap::new();
    for row in stmt.query_map([repo_id], read_row)? {
        let (mut raw, files) = row?;
        if !include_dropped && raw.node.status == NodeStatus::Dropped {
            continue;
        }
        raw.node.expected_files = serde_json::from_str(&files)?;
        raw.node.depends_on = deps.remove(&raw.node.id).unwrap_or_default();
        children.entry(raw.node.parent_id.clone()).or_default().push(raw);
    }
    for list in children.values_mut() {
        list.sort_by_key(|r| r.position);
    }
    let mut out = Vec::new();
    let mut stack: Vec<(PlanNode, usize)> = children
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .map(|r| (r.node, 0))
        .collect();
    while let Some((mut node, depth)) = stack.pop() {
        node.depth = depth;
        if let Some(kids) = children.remove(&Some(node.id.clone())) {
            stack.extend(kids.into_iter().rev().map(|r| (r.node, depth + 1)));
        }
        out.push(node);
    }
    Ok(out)
}

/// The number the next child of `id` will get.
pub fn next_child(conn: &Connection, repo_id: &str, id: &str) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT next_child FROM plan_nodes WHERE repo_id = ?1 AND id = ?2",
        params![repo_id, id],
        |r| r.get(0),
    )?)
}

/// Would adding "`id` depends on `depends_on`" create a cycle (including a self-loop)?
pub fn would_cycle(conn: &Connection, repo_id: &str, id: &str, depends_on: &str) -> Result<bool> {
    let reaches: i64 = conn.query_row(
        "WITH RECURSIVE reach(n) AS (
             SELECT ?3
             UNION
             SELECT d.depends_on FROM plan_deps d JOIN reach ON d.node_id = reach.n
             WHERE d.repo_id = ?1)
         SELECT count(*) FROM reach WHERE n = ?2",
        params![repo_id, id, depends_on],
        |r| r.get(0),
    )?;
    Ok(reaches > 0)
}

/// Is `candidate` equal to `root` or one of its descendants?
pub fn is_in_subtree(conn: &Connection, repo_id: &str, root: &str, candidate: &str) -> Result<bool> {
    let found: i64 = conn.query_row(
        "WITH RECURSIVE sub(id) AS (
             SELECT ?2
             UNION ALL
             SELECT n.id FROM plan_nodes n JOIN sub ON n.parent_id = sub.id WHERE n.repo_id = ?1)
         SELECT count(*) FROM sub WHERE id = ?3",
        params![repo_id, root, candidate],
        |r| r.get(0),
    )?;
    Ok(found > 0)
}
```

- [ ] **Step 7: Route plan events in the projection**

Replace `crates/bridge-screen-core/src/projection.rs` with:

```rust
//! Routes each stored event to the module that keeps its view tables up to date.

use rusqlite::Transaction;

use crate::error::Result;
use crate::plan::project::{PLAN_KINDS, project as project_plan};
use crate::store::Event;

/// View tables that `Store::rebuild` empties and refills from events.
pub const VIEW_TABLES: &[&str] = &["plan_nodes", "plan_deps"];

/// Update view tables for one event. Unknown kinds change no views.
pub fn apply(tx: &Transaction, event: &Event) -> Result<()> {
    if PLAN_KINDS.contains(&event.kind.as_str()) {
        project_plan(tx, event)?;
    }
    Ok(())
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p bridge-screen-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all tests pass (6 new plan tests); clippy clean.

- [ ] **Step 9: Commit**

```bash
git add crates/bridge-screen-core
git commit -m "core: plan model, plan event projector and tree queries

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `plan_edit` — validated, atomic batches and re-plans

**Files:**
- Create: `crates/bridge-screen-core/src/plan/ops.rs`
- Modify: `crates/bridge-screen-core/src/plan/mod.rs` (add `pub mod ops;` and re-exports)
- Modify: `crates/bridge-screen-core/src/error.rs` (add `Plan` variant)

**Interfaces:**
- Consumes: `store::{Store, Actor, Event, EventDraft, append_in}`; `plan::{get_node, next_child, would_cycle, is_in_subtree, NodeKind, NodeStatus, Hill}`; `plan::project::{EditMeta, NodeAdded, NodeEdited, NodeMoved, NodeRef, DepChange, IntentSet, EstimateSet, HillMoved}`.
- Produces (re-exported from `plan`):
  - `PlanOp` — serde `tag = "op"`, `snake_case`: `AddNode { parent: Option<String>, kind: NodeKind, title: String, intent: Option<String>, estimate_min: Option<u32>, expected_files: Vec<String> }`, `EditNode { id, title: Option<String>, expected_files: Option<Vec<String>> }`, `MoveNode { id, new_parent: String }`, `RemoveNode { id }`, `AddDep { id, depends_on }`, `RemoveDep { id, depends_on }`, `SetIntent { id, intent }`, `SetEstimate { id, estimate_min: u32 }`, `SetHill { id, hill: Hill }`.
  - `PlanError` (derives `Debug, thiserror::Error, PartialEq, Eq`): `Empty`, `NotFound { index, id }`, `Dropped { index, id }`, `BadParent { index, parent: String, child: String }`, `GoalExists { index, id }`, `MissingIntent { index, kind: String }`, `BadTitle { index }`, `BadEstimate { index }`, `Cycle { index, id, depends_on }`, `Invalid { index, reason: String }` — `index: usize` is the 0-based op position.
  - `EditContext { repo_id: String, session_id: String, actor: Actor, reason: Option<String>, replan: bool }`.
  - `EditOutcome { events: Vec<Event>, added_ids: Vec<String> }`.
  - `pub fn plan_edit(store: &mut Store, ctx: &EditContext, ops: &[PlanOp]) -> Result<EditOutcome>`.
  - `error::Error::Plan(PlanError)` via `#[from]`.

- [ ] **Step 1: Write the failing tests**

Create `crates/bridge-screen-core/src/plan/ops.rs` with only the tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::plan::{NodeStatus, get_node, load_tree};
    use crate::store::{Actor, Store, read_all};

    fn ctx() -> EditContext {
        EditContext {
            repo_id: "r".into(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: Some("setup".into()),
            replan: false,
        }
    }

    fn goal() -> PlanOp {
        PlanOp::AddNode {
            parent: None,
            kind: NodeKind::Goal,
            title: "Ship".into(),
            intent: Some("Owner sees progress".into()),
            estimate_min: None,
            expected_files: vec![],
        }
    }

    fn node(parent: &str, kind: NodeKind, title: &str) -> PlanOp {
        PlanOp::AddNode {
            parent: Some(parent.into()),
            kind,
            title: title.into(),
            intent: kind.requires_intent().then(|| "why".to_string()),
            estimate_min: None,
            expected_files: vec![],
        }
    }

    fn plan_error(result: Result<EditOutcome>) -> PlanError {
        match result {
            Err(Error::Plan(e)) => e,
            other => panic!("expected a plan error, got {other:?}"),
        }
    }

    fn seeded() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        plan_edit(
            &mut s,
            &ctx(),
            &[
                goal(),
                node("G", NodeKind::Milestone, "Core"),
                node("1", NodeKind::Task, "Schema"),
                node("1", NodeKind::Task, "Append"),
            ],
        )
        .unwrap();
        s
    }

    #[test]
    fn ids_follow_the_tree() {
        let mut s = Store::open_in_memory().unwrap();
        let out = plan_edit(
            &mut s,
            &ctx(),
            &[
                goal(),
                node("G", NodeKind::Milestone, "Core"),
                node("1", NodeKind::Task, "Schema"),
                node("1.1", NodeKind::Subtask, "Write SQL"),
                node("G", NodeKind::Task, "Loose task"),
            ],
        )
        .unwrap();
        assert_eq!(out.added_ids, vec!["G", "1", "1.1", "1.1.1", "2"]);
        assert_eq!(out.events.len(), 5);
        assert!(out.events.iter().all(|e| e.data["reason"] == "setup"));
        assert_eq!(out.events[2].task_id.as_deref(), Some("1.1"));
    }

    #[test]
    fn ids_are_never_reused_after_removal() {
        let mut s = seeded();
        plan_edit(&mut s, &ctx(), &[PlanOp::RemoveNode { id: "1.2".into() }]).unwrap();
        let out = plan_edit(&mut s, &ctx(), &[node("1", NodeKind::Task, "Again")]).unwrap();
        assert_eq!(out.added_ids, vec!["1.3"]);
    }

    #[test]
    fn invalid_op_rolls_back_whole_batch() {
        let mut s = seeded();
        let before = read_all(s.conn(), None).unwrap().len();
        let err = plan_error(plan_edit(
            &mut s,
            &ctx(),
            &[
                node("1", NodeKind::Task, "Ok one"),
                PlanOp::SetEstimate { id: "1.1".into(), estimate_min: 30 },
                node("1.1", NodeKind::Milestone, "Wrong kind"),
            ],
        ));
        assert_eq!(
            err,
            PlanError::BadParent { index: 2, parent: "task".into(), child: "milestone".into() }
        );
        assert_eq!(read_all(s.conn(), None).unwrap().len(), before);
        assert!(get_node(s.conn(), "r", "1.3").unwrap().is_none());
        assert_eq!(get_node(s.conn(), "r", "1.1").unwrap().unwrap().estimate_min, None);
    }

    #[test]
    fn structural_rules() {
        let mut s = seeded();
        assert_eq!(plan_error(plan_edit(&mut s, &ctx(), &[])), PlanError::Empty);
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[goal()])),
            PlanError::GoalExists { index: 0, id: "G".into() }
        );
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[node("9", NodeKind::Task, "x")])),
            PlanError::NotFound { index: 0, id: "9".into() }
        );
        let no_intent = PlanOp::AddNode {
            parent: Some("G".into()),
            kind: NodeKind::Milestone,
            title: "M".into(),
            intent: Some("   ".into()),
            estimate_min: None,
            expected_files: vec![],
        };
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[no_intent])),
            PlanError::MissingIntent { index: 0, kind: "milestone".into() }
        );
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[node("1", NodeKind::Task, "  ")])),
            PlanError::BadTitle { index: 0 }
        );
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[node("1", NodeKind::Task, &"x".repeat(201))])),
            PlanError::BadTitle { index: 0 }
        );
        assert_eq!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::SetEstimate { id: "1.1".into(), estimate_min: 0 }]
            )),
            PlanError::BadEstimate { index: 0 }
        );
        let orphan = PlanOp::AddNode {
            parent: None,
            kind: NodeKind::Task,
            title: "t".into(),
            intent: None,
            estimate_min: None,
            expected_files: vec![],
        };
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[orphan])),
            PlanError::BadParent { index: 0, parent: "nothing".into(), child: "task".into() }
        );
    }

    #[test]
    fn dependency_rules() {
        let mut s = seeded();
        let dep = |id: &str, on: &str| PlanOp::AddDep { id: id.into(), depends_on: on.into() };
        plan_edit(&mut s, &ctx(), &[dep("1.2", "1.1")]).unwrap();
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[dep("1.1", "1.2")])),
            PlanError::Cycle { index: 0, id: "1.1".into(), depends_on: "1.2".into() }
        );
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[dep("1.1", "1.1")])),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[dep("1.1", "G")])),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[dep("1.2", "1.1")])),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::RemoveDep { id: "1.1".into(), depends_on: "1.2".into() }]
            )),
            PlanError::Invalid { index: 0, .. }
        ));
        plan_edit(&mut s, &ctx(), &[PlanOp::RemoveDep { id: "1.2".into(), depends_on: "1.1".into() }])
            .unwrap();
        assert!(get_node(s.conn(), "r", "1.2").unwrap().unwrap().depends_on.is_empty());
    }

    #[test]
    fn move_remove_and_field_rules() {
        let mut s = seeded();
        plan_edit(&mut s, &ctx(), &[node("G", NodeKind::Milestone, "Screens")]).unwrap();
        plan_edit(&mut s, &ctx(), &[PlanOp::MoveNode { id: "1.2".into(), new_parent: "2".into() }])
            .unwrap();
        assert_eq!(get_node(s.conn(), "r", "1.2").unwrap().unwrap().parent_id.as_deref(), Some("2"));
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[PlanOp::MoveNode { id: "1".into(), new_parent: "1".into() }])),
            PlanError::BadParent { .. } | PlanError::Invalid { .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[PlanOp::RemoveNode { id: "G".into() }])),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[PlanOp::SetHill { id: "1.1".into(), hill: Hill::Uphill }])),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[PlanOp::SetIntent { id: "1".into(), intent: " ".into() }])),
            PlanError::MissingIntent { index: 0, .. }
        ));
        plan_edit(&mut s, &ctx(), &[PlanOp::RemoveNode { id: "1".into() }]).unwrap();
        assert_eq!(get_node(s.conn(), "r", "1.1").unwrap().unwrap().status, NodeStatus::Dropped);
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[PlanOp::EditNode { id: "1.1".into(), title: Some("x".into()), expected_files: None }])),
            PlanError::Dropped { index: 0, id: "1.1".into() }
        );
        let live: Vec<String> = load_tree(s.conn(), "r", false).unwrap().into_iter().map(|n| n.id).collect();
        assert_eq!(live, vec!["G", "2", "1.2"]);
    }

    #[test]
    fn replan_groups_edits() {
        let mut s = seeded();
        let mut c = ctx();
        c.replan = true;
        c.reason = None;
        assert!(matches!(
            plan_error(plan_edit(&mut s, &c, &[PlanOp::RemoveNode { id: "1.2".into() }])),
            PlanError::Invalid { index: 0, .. }
        ));
        c.reason = Some("scope changed".into());
        let out = plan_edit(
            &mut s,
            &c,
            &[PlanOp::RemoveNode { id: "1.2".into() }, node("1", NodeKind::Task, "New")],
        )
        .unwrap();
        assert_eq!(out.events.len(), 3);
        assert_eq!(out.events[0].kind, "replan");
        assert_eq!(out.events[0].data["op_count"], 2);
        let replan_id = out.events[0].id.clone();
        assert!(out.events[1..].iter().all(|e| e.data["replan_id"] == replan_id.as_str()));
    }

    #[test]
    fn ops_parse_from_json() {
        let ops: Vec<PlanOp> = serde_json::from_str(
            r#"[{"op":"add_node","kind":"goal","title":"G","intent":"i"},
                {"op":"set_hill","id":"1","hill":"uphill"},
                {"op":"add_dep","id":"1.2","depends_on":"1.1"}]"#,
        )
        .unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], PlanOp::AddNode { parent: None, kind: NodeKind::Goal, .. }));
    }
}
```

In `plan/mod.rs`, add `pub mod ops;` after `pub mod model;` and add the line:

```rust
pub use ops::{EditContext, EditOutcome, PlanError, PlanOp, plan_edit};
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p bridge-screen-core ops`
Expected: compile errors — `PlanOp`, `plan_edit`, `EditContext`, `Error::Plan` not found.

- [ ] **Step 3: Add the error variant**

In `crates/bridge-screen-core/src/error.rs`, add this variant to `Error` after `NoHome`:

```rust
    #[error(transparent)]
    Plan(#[from] crate::plan::PlanError),
```

- [ ] **Step 4: Write the implementation**

Prepend to `crates/bridge-screen-core/src/plan/ops.rs` (above the tests):

```rust
//! Validated plan edits. A batch is applied in one transaction: all ops or none.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::model::{Hill, NodeKind, NodeStatus, PlanNode};
use super::project::{
    DepChange, EditMeta, EstimateSet, HillMoved, IntentSet, NodeAdded, NodeEdited, NodeMoved,
    NodeRef,
};
use super::query::{get_node, is_in_subtree, next_child, would_cycle};
use crate::error::Result;
use crate::store::{Actor, Event, EventDraft, Store, append_in};

const MAX_TITLE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanOp {
    AddNode {
        #[serde(default)]
        parent: Option<String>,
        kind: NodeKind,
        title: String,
        #[serde(default)]
        intent: Option<String>,
        #[serde(default)]
        estimate_min: Option<u32>,
        #[serde(default)]
        expected_files: Vec<String>,
    },
    EditNode {
        id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        expected_files: Option<Vec<String>>,
    },
    MoveNode {
        id: String,
        new_parent: String,
    },
    RemoveNode {
        id: String,
    },
    AddDep {
        id: String,
        depends_on: String,
    },
    RemoveDep {
        id: String,
        depends_on: String,
    },
    SetIntent {
        id: String,
        intent: String,
    },
    SetEstimate {
        id: String,
        estimate_min: u32,
    },
    SetHill {
        id: String,
        hill: Hill,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("no operations given")]
    Empty,
    #[error("op {index}: node {id} not found")]
    NotFound { index: usize, id: String },
    #[error("op {index}: node {id} was dropped")]
    Dropped { index: usize, id: String },
    #[error("op {index}: a {child} cannot go under {parent}")]
    BadParent { index: usize, parent: String, child: String },
    #[error("op {index}: the plan already has a goal ({id})")]
    GoalExists { index: usize, id: String },
    #[error("op {index}: a {kind} needs an intent (its purpose and end state)")]
    MissingIntent { index: usize, kind: String },
    #[error("op {index}: title must be 1 to 200 characters")]
    BadTitle { index: usize },
    #[error("op {index}: estimate must be greater than 0 minutes")]
    BadEstimate { index: usize },
    #[error("op {index}: {id} depending on {depends_on} would create a cycle")]
    Cycle { index: usize, id: String, depends_on: String },
    #[error("op {index}: {reason}")]
    Invalid { index: usize, reason: String },
}

/// Who is editing, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditContext {
    pub repo_id: String,
    pub session_id: String,
    pub actor: Actor,
    pub reason: Option<String>,
    /// Record the batch as one deliberate re-plan (requires a reason).
    pub replan: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditOutcome {
    pub events: Vec<Event>,
    /// Ids assigned to added nodes, in op order.
    pub added_ids: Vec<String>,
}

/// Validate and apply `ops` in order, atomically.
pub fn plan_edit(store: &mut Store, ctx: &EditContext, ops: &[PlanOp]) -> Result<EditOutcome> {
    if ops.is_empty() {
        return Err(PlanError::Empty.into());
    }
    store.write(|tx| {
        let mut events = Vec::new();
        let mut added_ids = Vec::new();
        let replan_id = if ctx.replan {
            if ctx.reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
                return Err(PlanError::Invalid { index: 0, reason: "a replan needs a reason".into() }
                    .into());
            }
            let data = json!({ "reason": ctx.reason, "op_count": ops.len() });
            let event = append_in(tx, draft(ctx, "replan", data, None))?;
            let id = event.id.clone();
            events.push(event);
            Some(id)
        } else {
            None
        };
        let meta = EditMeta { reason: ctx.reason.clone(), replan_id };
        for (index, op) in ops.iter().enumerate() {
            let (kind, data, task) = validate(tx, &ctx.repo_id, index, op, &meta)?;
            if kind == "node_added" {
                added_ids.push(task.clone());
            }
            events.push(append_in(tx, draft(ctx, kind, data, Some(task)))?);
        }
        Ok(EditOutcome { events, added_ids })
    })
}

fn draft(ctx: &EditContext, kind: &str, data: Value, task: Option<String>) -> EventDraft {
    let mut d = EventDraft::new(&ctx.repo_id, &ctx.session_id, kind, data).actor(ctx.actor.clone());
    if let Some(task) = task {
        d = d.task(task);
    }
    d
}

/// Look up a node that must exist and not be dropped.
fn live(conn: &Connection, repo: &str, index: usize, id: &str) -> Result<PlanNode> {
    match get_node(conn, repo, id)? {
        None => Err(PlanError::NotFound { index, id: id.to_string() }.into()),
        Some(n) if n.status == NodeStatus::Dropped => {
            Err(PlanError::Dropped { index, id: id.to_string() }.into())
        }
        Some(n) => Ok(n),
    }
}

fn check_title(index: usize, title: &str) -> Result<String> {
    let t = title.trim();
    if t.is_empty() || t.chars().count() > MAX_TITLE_CHARS {
        return Err(PlanError::BadTitle { index }.into());
    }
    Ok(t.to_string())
}

fn invalid(index: usize, reason: impl Into<String>) -> crate::error::Error {
    PlanError::Invalid { index, reason: reason.into() }.into()
}

/// Validate one op against the current views. Returns (event kind, data, task id).
fn validate(
    conn: &Connection,
    repo: &str,
    index: usize,
    op: &PlanOp,
    meta: &EditMeta,
) -> Result<(&'static str, Value, String)> {
    let meta = meta.clone();
    match op {
        PlanOp::AddNode { parent, kind, title, intent, estimate_min, expected_files } => {
            let title = check_title(index, title)?;
            let intent = intent.as_deref().map(str::trim).filter(|i| !i.is_empty());
            if kind.requires_intent() && intent.is_none() {
                return Err(PlanError::MissingIntent { index, kind: kind.as_str().into() }.into());
            }
            if estimate_min == &Some(0) {
                return Err(PlanError::BadEstimate { index }.into());
            }
            let (id, parent_id) = match (kind, parent) {
                (NodeKind::Goal, None) => {
                    if let Some(g) = get_node(conn, repo, "G")?
                        && g.status != NodeStatus::Dropped
                    {
                        return Err(PlanError::GoalExists { index, id: g.id }.into());
                    }
                    ("G".to_string(), None)
                }
                (_, None) => {
                    return Err(PlanError::BadParent {
                        index,
                        parent: "nothing".into(),
                        child: kind.as_str().into(),
                    }
                    .into());
                }
                (_, Some(pid)) => {
                    let p = live(conn, repo, index, pid)?;
                    if !p.kind.allows_child(*kind) {
                        return Err(PlanError::BadParent {
                            index,
                            parent: p.kind.as_str().into(),
                            child: kind.as_str().into(),
                        }
                        .into());
                    }
                    let n = next_child(conn, repo, &p.id)?;
                    let id = if p.kind == NodeKind::Goal { n.to_string() } else { format!("{}.{n}", p.id) };
                    (id, Some(p.id))
                }
            };
            let data = NodeAdded {
                id: id.clone(),
                parent_id,
                kind: *kind,
                title,
                intent: intent.map(str::to_string),
                estimate_min: *estimate_min,
                expected_files: expected_files.clone(),
                meta,
            };
            Ok(("node_added", serde_json::to_value(data)?, id))
        }
        PlanOp::EditNode { id, title, expected_files } => {
            live(conn, repo, index, id)?;
            let title = title.as_deref().map(|t| check_title(index, t)).transpose()?;
            if title.is_none() && expected_files.is_none() {
                return Err(invalid(index, "edit_node needs a title or expected_files"));
            }
            let data = NodeEdited { id: id.clone(), title, expected_files: expected_files.clone(), meta };
            Ok(("node_edited", serde_json::to_value(data)?, id.clone()))
        }
        PlanOp::MoveNode { id, new_parent } => {
            let node = live(conn, repo, index, id)?;
            if node.kind == NodeKind::Goal {
                return Err(invalid(index, "the goal cannot be moved"));
            }
            let parent = live(conn, repo, index, new_parent)?;
            if is_in_subtree(conn, repo, id, new_parent)? {
                return Err(invalid(index, format!("{id} cannot move under itself or its children")));
            }
            if !parent.kind.allows_child(node.kind) {
                return Err(PlanError::BadParent {
                    index,
                    parent: parent.kind.as_str().into(),
                    child: node.kind.as_str().into(),
                }
                .into());
            }
            let data = NodeMoved { id: id.clone(), new_parent_id: parent.id, meta };
            Ok(("node_moved", serde_json::to_value(data)?, id.clone()))
        }
        PlanOp::RemoveNode { id } => {
            let node = live(conn, repo, index, id)?;
            if node.kind == NodeKind::Goal {
                return Err(invalid(index, "the goal cannot be removed"));
            }
            let data = NodeRef { id: id.clone(), meta };
            Ok(("node_removed", serde_json::to_value(data)?, id.clone()))
        }
        PlanOp::AddDep { id, depends_on } => {
            let node = live(conn, repo, index, id)?;
            let target = live(conn, repo, index, depends_on)?;
            if node.id == target.id {
                return Err(invalid(index, format!("{id} cannot depend on itself")));
            }
            if node.kind == NodeKind::Goal || target.kind == NodeKind::Goal {
                return Err(invalid(index, "the goal cannot take part in dependencies"));
            }
            if node.depends_on.contains(depends_on) {
                return Err(invalid(index, format!("{id} already depends on {depends_on}")));
            }
            if would_cycle(conn, repo, id, depends_on)? {
                return Err(PlanError::Cycle {
                    index,
                    id: id.clone(),
                    depends_on: depends_on.clone(),
                }
                .into());
            }
            let data = DepChange { id: id.clone(), depends_on: depends_on.clone(), meta };
            Ok(("dep_added", serde_json::to_value(data)?, id.clone()))
        }
        PlanOp::RemoveDep { id, depends_on } => {
            let node = live(conn, repo, index, id)?;
            if !node.depends_on.contains(depends_on) {
                return Err(invalid(index, format!("{id} does not depend on {depends_on}")));
            }
            let data = DepChange { id: id.clone(), depends_on: depends_on.clone(), meta };
            Ok(("dep_removed", serde_json::to_value(data)?, id.clone()))
        }
        PlanOp::SetIntent { id, intent } => {
            let node = live(conn, repo, index, id)?;
            let intent = intent.trim();
            if intent.is_empty() {
                return Err(PlanError::MissingIntent { index, kind: node.kind.as_str().into() }.into());
            }
            let data = IntentSet { id: id.clone(), intent: intent.to_string(), meta };
            Ok(("intent_set", serde_json::to_value(data)?, id.clone()))
        }
        PlanOp::SetEstimate { id, estimate_min } => {
            live(conn, repo, index, id)?;
            if *estimate_min == 0 {
                return Err(PlanError::BadEstimate { index }.into());
            }
            let data = EstimateSet { id: id.clone(), estimate_min: *estimate_min, meta };
            Ok(("estimate_set", serde_json::to_value(data)?, id.clone()))
        }
        PlanOp::SetHill { id, hill } => {
            let node = live(conn, repo, index, id)?;
            if node.kind != NodeKind::Milestone {
                return Err(invalid(index, "only milestones have an uphill/downhill state"));
            }
            let data = HillMoved { id: id.clone(), hill: *hill, meta };
            Ok(("hill_moved", serde_json::to_value(data)?, id.clone()))
        }
    }
}
```

Note: `plan_edit` passes `tx` (a `&Transaction`) where `validate` takes `&Connection`; `Transaction` derefs to `Connection`, so `validate(tx, …)` compiles via deref coercion. If clippy flags `too_many_lines` on `validate`, split each match arm into its own `fn validate_<op>(…)` returning the same tuple; keep behaviour identical.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p bridge-screen-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all tests pass (8 new ops tests); clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-screen-core
git commit -m "core: validated, atomic plan_edit batches and re-plans

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Rebuild views from the event log

**Files:**
- Modify: `crates/bridge-screen-core/src/store/mod.rs` (add `RebuildStats`, `Store::rebuild`)
- Create: `crates/bridge-screen-core/tests/rebuild.rs`

**Interfaces:**
- Consumes: `projection::{VIEW_TABLES, apply}`, `store::read_all`, `plan::{plan_edit, EditContext, PlanOp, NodeKind, Hill}`.
- Produces: `store::RebuildStats { events: usize }` (derives `Debug, Clone, Copy, PartialEq, Eq`); `Store::rebuild(&mut self) -> Result<RebuildStats>`.

- [ ] **Step 1: Write the failing test**

Create `crates/bridge-screen-core/tests/rebuild.rs`:

```rust
use bridge_screen_core::plan::{EditContext, Hill, NodeKind, PlanOp, plan_edit};
use bridge_screen_core::store::{Actor, Store};

fn ctx(repo: &str, replan: bool) -> EditContext {
    EditContext {
        repo_id: repo.into(),
        session_id: "owner".into(),
        actor: Actor::default(),
        reason: Some("test".into()),
        replan,
    }
}

fn add(parent: Option<&str>, kind: NodeKind, title: &str) -> PlanOp {
    PlanOp::AddNode {
        parent: parent.map(str::to_string),
        kind,
        title: title.into(),
        intent: kind.requires_intent().then(|| "why".to_string()),
        estimate_min: Some(20),
        expected_files: vec![format!("src/{title}.rs")],
    }
}

/// Every view row, as text, in a stable order.
fn dump(store: &Store) -> Vec<String> {
    let mut out = Vec::new();
    for (table, order) in [("plan_nodes", "repo_id, id"), ("plan_deps", "repo_id, node_id, depends_on")] {
        let mut stmt = store.conn().prepare(&format!("SELECT * FROM {table} ORDER BY {order}")).unwrap();
        let cols = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                Ok((0..cols).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))
            })
            .unwrap();
        out.extend(rows.map(|r| format!("{table}: {}", r.unwrap())));
    }
    out
}

#[test]
fn rebuild_reproduces_incremental_views() {
    let mut s = Store::open_in_memory().unwrap();
    for repo in ["a", "b"] {
        plan_edit(
            &mut s,
            &ctx(repo, false),
            &[
                add(None, NodeKind::Goal, "goal"),
                add(Some("G"), NodeKind::Milestone, "core"),
                add(Some("1"), NodeKind::Task, "schema"),
                add(Some("1"), NodeKind::Task, "append"),
                add(Some("1.1"), NodeKind::Subtask, "sql"),
                add(Some("G"), NodeKind::Milestone, "screens"),
            ],
        )
        .unwrap();
    }
    plan_edit(
        &mut s,
        &ctx("a", false),
        &[
            PlanOp::AddDep { id: "1.2".into(), depends_on: "1.1".into() },
            PlanOp::SetHill { id: "1".into(), hill: Hill::Downhill },
            PlanOp::EditNode { id: "1.1".into(), title: Some("schema v1".into()), expected_files: None },
            PlanOp::MoveNode { id: "1.2".into(), new_parent: "2".into() },
            PlanOp::SetEstimate { id: "1.1.1".into(), estimate_min: 45 },
        ],
    )
    .unwrap();
    plan_edit(
        &mut s,
        &ctx("a", true),
        &[PlanOp::RemoveNode { id: "1".into() }, add(Some("2"), NodeKind::Task, "late")],
    )
    .unwrap();

    let before = dump(&s);
    assert!(before.len() > 10);
    let stats = s.rebuild().unwrap();
    assert_eq!(stats.events, 6 + 6 + 5 + 3);
    assert_eq!(dump(&s), before);
}

#[test]
fn rebuild_of_empty_store_is_fine() {
    let mut s = Store::open_in_memory().unwrap();
    assert_eq!(s.rebuild().unwrap().events, 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p bridge-screen-core --test rebuild`
Expected: compile error — no method `rebuild` on `Store`.

- [ ] **Step 3: Write the implementation**

In `crates/bridge-screen-core/src/store/mod.rs`, add after the `Store` struct definition:

```rust
/// What a rebuild did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildStats {
    pub events: usize,
}
```

and add this method inside `impl Store`, after `register_repo`:

```rust
    /// Empty every view table and replay all events in append order (one transaction).
    pub fn rebuild(&mut self) -> Result<RebuildStats> {
        self.write(|tx| {
            for table in crate::projection::VIEW_TABLES {
                tx.execute(&format!("DELETE FROM {table}"), [])?;
            }
            let events = read_all(tx, None)?;
            for event in &events {
                crate::projection::apply(tx, event)?;
            }
            Ok(RebuildStats { events: events.len() })
        })
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p bridge-screen-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all tests pass, including both rebuild tests; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-screen-core
git commit -m "core: rebuild views by replaying the event log

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Plain-text plan rendering

**Files:**
- Create: `crates/bridge-screen-core/src/plan/render.rs`
- Modify: `crates/bridge-screen-core/src/plan/mod.rs` (add `pub mod render;` and `pub use render::render_text;`)

**Interfaces:**
- Consumes: `plan::PlanNode` (with `depth` set by `load_tree`).
- Produces: `plan::render_text(nodes: &[PlanNode]) -> String`.

Format (each line ends with `\n`; indent is two spaces per depth level):
- Node line: `{indent}{id}  {title}  [{status}]` then, in this order and only when present: ` {estimate_min}m`, ` ↑ uphill` / ` ↓ downhill`, ` (needs {deps joined by ", "})`.
- Intent line, right after its node when the node has an intent: `{indent}   ↳ {intent}`.

- [ ] **Step 1: Write the failing test**

Create `crates/bridge-screen-core/src/plan/render.rs` with only the tests:

```rust
#[cfg(test)]
mod tests {
    use super::render_text;
    use crate::plan::{Hill, NodeKind, NodeStatus, PlanNode};

    fn n(id: &str, kind: NodeKind, title: &str, depth: usize) -> PlanNode {
        PlanNode {
            id: id.into(),
            parent_id: None,
            kind,
            title: title.into(),
            intent: None,
            estimate_min: None,
            status: NodeStatus::Todo,
            hill: None,
            expected_files: vec![],
            depends_on: vec![],
            depth,
        }
    }

    #[test]
    fn renders_tree_with_details() {
        let mut goal = n("G", NodeKind::Goal, "Ship bridge-screen", 0);
        goal.intent = Some("Owner sees progress at a glance.".into());
        let mut m = n("1", NodeKind::Milestone, "Core store", 1);
        m.intent = Some("Events are the source of truth.".into());
        m.hill = Some(Hill::Uphill);
        let mut t = n("1.1", NodeKind::Task, "Schema", 2);
        t.estimate_min = Some(30);
        t.depends_on = vec!["1.2".into(), "2.1".into()];
        t.status = NodeStatus::L2;
        let expected = "\
G  Ship bridge-screen  [todo]
   ↳ Owner sees progress at a glance.
  1  Core store  [todo] ↑ uphill
     ↳ Events are the source of truth.
    1.1  Schema  [l2] 30m (needs 1.2, 2.1)
";
        assert_eq!(render_text(&[goal, m, t]), expected);
    }

    #[test]
    fn empty_tree_renders_nothing() {
        assert_eq!(render_text(&[]), "");
    }
}
```

Add to `plan/mod.rs`: `pub mod render;` after `pub mod query;` and `pub use render::render_text;`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p bridge-screen-core render`
Expected: compile error — `render_text` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/bridge-screen-core/src/plan/render.rs`:

```rust
//! Plain-text rendering of the plan tree for the CLI.

use std::fmt::Write as _;

use super::model::{Hill, PlanNode};

pub fn render_text(nodes: &[PlanNode]) -> String {
    let mut out = String::new();
    for node in nodes {
        let indent = "  ".repeat(node.depth);
        let _ = write!(out, "{indent}{}  {}  [{}]", node.id, node.title, node.status.as_str());
        if let Some(min) = node.estimate_min {
            let _ = write!(out, " {min}m");
        }
        match node.hill {
            Some(Hill::Uphill) => out.push_str(" ↑ uphill"),
            Some(Hill::Downhill) => out.push_str(" ↓ downhill"),
            None => {}
        }
        if !node.depends_on.is_empty() {
            let _ = write!(out, " (needs {})", node.depends_on.join(", "));
        }
        out.push('\n');
        if let Some(intent) = &node.intent {
            let _ = writeln!(out, "{indent}   ↳ {intent}");
        }
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p bridge-screen-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all tests pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-screen-core
git commit -m "core: plain-text plan rendering

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: CLI — install, plan show, plan edit, rebuild, config

**Files:**
- Modify: `crates/bridge-screen/Cargo.toml`
- Modify: `crates/bridge-screen/src/main.rs`
- Create: `crates/bridge-screen/src/cli.rs`
- Create: `crates/bridge-screen/src/commands.rs`
- Create: `crates/bridge-screen/tests/cli.rs`

**Interfaces:**
- Consumes: `paths::Home`, `config::Config`, `repo::identify`, `store::{Store, Actor, OWNER_SESSION}`, `plan::{load_tree, render_text, plan_edit, EditContext, PlanOp}`.
- Produces: the `bridge-screen` binary with subcommands:
  - `install` — creates the data folder, database and default config; prints their paths.
  - `plan show [-C DIR] [--all]` — prints the tree, or `No plan yet for <name>. Add a goal with \`bridge-screen plan edit\`.`
  - `plan edit [-C DIR] [--reason TEXT] [--replan] [--file PATH]` — reads a JSON array of `PlanOp` from `--file` or stdin; prints `applied N operation(s)` and one `added <id>` line per new node.
  - `rebuild` — prints `rebuilt views from N events`.
  - `config` — prints the effective config as TOML.
  - Errors print `error: <message>` to stderr and exit with status 1.

- [ ] **Step 1: Add dependencies**

Replace the `[dependencies]` section of `crates/bridge-screen/Cargo.toml` with:

```toml
[dependencies]
anyhow = "1.0.104"
bridge-screen-core = { path = "../bridge-screen-core" }
clap = { version = "4.6.7", features = ["derive"] }
serde_json = "1.0.154"

[dev-dependencies]
tempfile = "3.27.0"
```

- [ ] **Step 2: Write the failing tests**

Create `crates/bridge-screen/tests/cli.rs`:

```rust
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

struct Env {
    home: tempfile::TempDir,
    repo: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let repo = tempfile::tempdir().unwrap();
        let ok = Command::new("git").arg("-C").arg(repo.path()).args(["init", "-q"]).status().unwrap();
        assert!(ok.success());
        Self { home: tempfile::tempdir().unwrap(), repo }
    }

    fn run(&self, args: &[&str], stdin: Option<&str>, cwd: &Path) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bridge-screen"))
            .args(args)
            .current_dir(cwd)
            .env("BRIDGE_SCREEN_HOME", self.home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = stdin {
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        }
        drop(child.stdin.take());
        child.wait_with_output().unwrap()
    }

    fn ok(&self, args: &[&str], stdin: Option<&str>) -> String {
        let out = self.run(args, stdin, self.repo.path());
        assert!(out.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }
}

const SEED: &str = r#"[
  {"op":"add_node","kind":"goal","title":"Ship","intent":"Owner sees progress"},
  {"op":"add_node","parent":"G","kind":"milestone","title":"Core","intent":"Events first"},
  {"op":"add_node","parent":"1","kind":"task","title":"Schema","estimate_min":30}
]"#;

#[test]
fn version_flag_prints_version() {
    let env = Env::new();
    assert!(env.ok(&["--version"], None).starts_with("bridge-screen "));
}

#[test]
fn install_creates_home_db_and_config() {
    let env = Env::new();
    let out = env.ok(&["install"], None);
    assert!(env.home.path().join("bridge-screen.db").is_file());
    assert!(env.home.path().join("config.toml").is_file());
    assert!(out.contains("config.toml (created)"));
    assert!(env.ok(&["install"], None).contains("config.toml (kept existing)"));
}

#[test]
fn empty_plan_explains_next_step() {
    let env = Env::new();
    let out = env.ok(&["plan", "show"], None);
    assert!(out.starts_with("No plan yet for "));
    assert!(out.contains("bridge-screen plan edit"));
}

#[test]
fn edit_then_show() {
    let env = Env::new();
    let out = env.ok(&["plan", "edit", "--reason", "initial plan"], Some(SEED));
    assert_eq!(out, "applied 3 operation(s)\nadded G\nadded 1\nadded 1.1\n");
    let shown = env.ok(&["plan", "show"], None);
    assert_eq!(
        shown,
        "G  Ship  [todo]\n   ↳ Owner sees progress\n  1  Core  [todo]\n     ↳ Events first\n    1.1  Schema  [todo] 30m\n"
    );
}

#[test]
fn edit_reads_file_and_dir_flag() {
    let env = Env::new();
    let file = env.home.path().join("ops.json");
    std::fs::write(&file, SEED).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let repo = env.repo.path().to_str().unwrap();
    let out = env.run(&["plan", "edit", "-C", repo, "--file", file.to_str().unwrap()], None, elsewhere.path());
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(env.ok(&["plan", "show"], None).contains("1.1  Schema"));
}

#[test]
fn invalid_batch_changes_nothing() {
    let env = Env::new();
    env.ok(&["plan", "edit"], Some(SEED));
    let before = env.ok(&["plan", "show"], None);
    let bad = r#"[{"op":"add_node","parent":"1","kind":"task","title":"Fine"},
                  {"op":"add_node","parent":"1.1","kind":"milestone","title":"Bad","intent":"x"}]"#;
    let out = env.run(&["plan", "edit"], Some(bad), env.repo.path());
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("error: op 1: a milestone cannot go under task"), "{err}");
    assert_eq!(env.ok(&["plan", "show"], None), before);
}

#[test]
fn malformed_json_is_explained() {
    let env = Env::new();
    let out = env.run(&["plan", "edit"], Some("{not json"), env.repo.path());
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("JSON array of plan operations"));
}

#[test]
fn outside_git_repo_reports_clearly() {
    let env = Env::new();
    let plain = tempfile::tempdir().unwrap();
    let out = env.run(&["plan", "show"], None, plain.path());
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a git repository"));
}

#[test]
fn rebuild_reports_event_count() {
    let env = Env::new();
    env.ok(&["plan", "edit"], Some(SEED));
    assert_eq!(env.ok(&["rebuild"], None), "rebuilt views from 3 events\n");
}

#[test]
fn config_prints_effective_values() {
    let env = Env::new();
    std::fs::write(env.home.path().join("config.toml"), "points_minutes = 20\n").unwrap();
    let out = env.ok(&["config"], None);
    assert!(out.contains("points_minutes = 20"));
    assert!(out.contains("crash_timeout_min = 30"));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p bridge-screen`
Expected: tests compile but fail — the binary only prints its version (`install`, `plan`, etc. are unknown).

- [ ] **Step 4: Write the CLI definitions**

Create `crates/bridge-screen/src/cli.rs`:

```rust
use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Keeps track of where your Claude Code agents are: progress, tangents, and handoffs.
#[derive(Debug, Parser)]
#[command(name = "bridge-screen", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Set up the data folder: database and default config.
    Install,
    /// Show or change a repo's plan.
    Plan {
        #[command(subcommand)]
        action: PlanCommand,
    },
    /// Rebuild all views from the event log.
    Rebuild,
    /// Print the effective configuration.
    Config,
}

#[derive(Debug, Subcommand)]
pub enum PlanCommand {
    /// Print the plan tree.
    Show {
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
        /// Include dropped nodes.
        #[arg(long)]
        all: bool,
    },
    /// Apply a JSON array of plan operations (from --file or stdin).
    Edit {
        /// Any directory inside the repo.
        #[arg(short = 'C', long = "dir", default_value = ".")]
        dir: PathBuf,
        /// Why the plan is changing.
        #[arg(long)]
        reason: Option<String>,
        /// Record the batch as one deliberate re-plan (needs --reason).
        #[arg(long)]
        replan: bool,
        /// Read operations from this file instead of stdin.
        #[arg(long)]
        file: Option<PathBuf>,
    },
}
```

- [ ] **Step 5: Write the commands**

Create `crates/bridge-screen/src/commands.rs`:

```rust
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use bridge_screen_core::config::Config;
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{EditContext, PlanOp, load_tree, plan_edit, render_text};
use bridge_screen_core::repo;
use bridge_screen_core::store::{Actor, OWNER_SESSION, Store};

use crate::cli::{Cli, Command, PlanCommand};

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Install => install(),
        Command::Plan { action: PlanCommand::Show { dir, all } } => plan_show(&dir, all),
        Command::Plan { action: PlanCommand::Edit { dir, reason, replan, file } } => {
            plan_edit_cmd(&dir, reason, replan, file.as_deref())
        }
        Command::Rebuild => rebuild(),
        Command::Config => config(),
    }
}

fn open_store(home: &Home) -> Result<Store> {
    home.ensure()?;
    Ok(Store::open(&home.db_path())?)
}

fn install() -> Result<()> {
    let home = Home::resolve()?;
    open_store(&home)?;
    let created = Config::write_default_if_missing(&home.config_path())?;
    println!("bridge-screen home: {}", home.root().display());
    println!("database: {}", home.db_path().display());
    println!(
        "config: {} ({})",
        home.config_path().display(),
        if created { "created" } else { "kept existing" }
    );
    println!("Claude Code plugin registration arrives in a later release.");
    Ok(())
}

fn plan_show(dir: &Path, all: bool) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let store = open_store(&home)?;
    store.register_repo(&repo)?;
    let nodes = load_tree(store.conn(), &repo.id, all)?;
    if nodes.is_empty() {
        println!("No plan yet for {}. Add a goal with `bridge-screen plan edit`.", repo.name);
    } else {
        print!("{}", render_text(&nodes));
    }
    Ok(())
}

fn plan_edit_cmd(
    dir: &Path,
    reason: Option<String>,
    replan: bool,
    file: Option<&Path>,
) -> Result<()> {
    let repo = repo::identify(dir)?;
    let text = match file {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).context("reading stdin")?;
            buf
        }
    };
    let ops: Vec<PlanOp> =
        serde_json::from_str(&text).context("ops must be a JSON array of plan operations")?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let ctx = EditContext {
        repo_id: repo.id,
        session_id: OWNER_SESSION.to_string(),
        actor: Actor::default(),
        reason,
        replan,
    };
    let outcome = plan_edit(&mut store, &ctx, &ops)?;
    println!("applied {} operation(s)", ops.len());
    for id in outcome.added_ids {
        println!("added {id}");
    }
    Ok(())
}

fn rebuild() -> Result<()> {
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    let stats = store.rebuild()?;
    println!("rebuilt views from {} events", stats.events);
    Ok(())
}

fn config() -> Result<()> {
    let home = Home::resolve()?;
    let cfg = Config::load(&home.config_path())?;
    print!("{}", cfg.to_toml()?);
    Ok(())
}
```

Replace `crates/bridge-screen/src/main.rs` with:

```rust
mod cli;
mod commands;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match commands::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
```

Note on the expected stderr in `invalid_batch_changes_nothing`: `{err:#}` prints the anyhow chain; the core error's message is `op 1: a milestone cannot go under task`, so stderr starts with `error: op 1: a milestone cannot go under task`. In `malformed_json_is_explained`, the chain is `ops must be a JSON array of plan operations: <serde detail>`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all 10 CLI tests and all core tests pass; clippy clean.

- [ ] **Step 7: Try it by hand**

```bash
export BRIDGE_SCREEN_HOME="$(mktemp -d)"
cargo run -q -p bridge-screen -- install
echo '[{"op":"add_node","kind":"goal","title":"Try it","intent":"See the tree"}]' | cargo run -q -p bridge-screen -- plan edit --reason demo
cargo run -q -p bridge-screen -- plan show
unset BRIDGE_SCREEN_HOME
```

Expected: install prints three paths; edit prints `applied 1 operation(s)` and `added G`; show prints `G  Try it  [todo]` and the intent line.

- [ ] **Step 8: Commit**

```bash
git add crates/bridge-screen Cargo.lock
git commit -m "cli: install, plan show/edit, rebuild, config

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Release pipeline (cargo-dist + PyPI wheels)

**Files:**
- Create: `dist-workspace.toml` (via `dist init`, then edited)
- Create: `.github/workflows/release.yml` (generated by `dist generate`)
- Create: `crates/bridge-screen/pyproject.toml`
- Create: `.github/workflows/pypi.yml`
- Create: `docs/RELEASING.md`
- Modify: `CLAUDE.md` (one line under Commands)

**Interfaces:**
- Consumes: the `bridge-screen` binary crate.
- Produces: a tag-triggered release (`v*`) that builds binaries for macOS arm64/x86_64, Linux x86_64/arm64/musl and Windows x86_64; publishes GitHub Release artifacts with checksums, shell and PowerShell installers, a Homebrew formula to `bedantsharma/homebrew-tap`, npm packages, and PyPI wheels. No release is cut in this task.

- [ ] **Step 1: Install cargo-dist**

Run: `cargo install cargo-dist --version 0.32.0 --locked`
Expected: `dist --version` prints `cargo-dist 0.32.0`.

- [ ] **Step 2: Initialise dist**

Run: `dist init --yes`
Expected: creates `dist-workspace.toml` and `.github/workflows/release.yml`.

- [ ] **Step 3: Set the release configuration**

Make the `[dist]` table of `dist-workspace.toml` contain exactly these keys (keep whatever `[workspace]` table `dist init` wrote):

```toml
[dist]
cargo-dist-version = "0.32.0"
ci = "github"
installers = ["shell", "powershell", "homebrew", "npm"]
targets = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-unknown-linux-musl",
  "x86_64-pc-windows-msvc",
]
tap = "bedantsharma/homebrew-tap"
publish-jobs = ["homebrew", "npm"]
install-path = "CARGO_HOME"
install-updater = false
```

In `crates/bridge-screen-core/Cargo.toml`, add:

```toml
[package.metadata.dist]
dist = false
```

Then run: `dist generate && dist plan`
Expected: `dist plan` lists one app, `bridge-screen 0.1.0`, with the six targets and installers; no errors. If dist rejects a key name, run `dist init` again interactively-free with `--yes` and compare the key it writes; keep the intent of each setting above.

- [ ] **Step 4: Add PyPI packaging**

Create `crates/bridge-screen/pyproject.toml`:

```toml
[build-system]
requires = ["maturin>=1.15,<2"]
build-backend = "maturin"

[project]
name = "bridge-screen"
description = "Keeps track of where your Claude Code agents are: progress, tangents, and handoffs."
requires-python = ">=3.9"
classifiers = ["Programming Language :: Rust", "Environment :: Console"]
dynamic = ["version"]

[tool.maturin]
bindings = "bin"
```

Create `.github/workflows/pypi.yml`:

```yaml
name: PyPI

on:
  push:
    tags: ["v*"]

permissions:
  contents: read

jobs:
  wheels:
    name: Wheel (${{ matrix.target }})
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - { os: macos-latest, target: aarch64-apple-darwin }
          - { os: macos-latest, target: x86_64-apple-darwin }
          - { os: ubuntu-latest, target: x86_64-unknown-linux-gnu }
          - { os: ubuntu-latest, target: aarch64-unknown-linux-gnu }
          - { os: windows-latest, target: x86_64-pc-windows-msvc }
    steps:
      - uses: actions/checkout@v4
      - uses: PyO3/maturin-action@v1
        with:
          target: ${{ matrix.target }}
          args: --release --out dist --manifest-path crates/bridge-screen/Cargo.toml
          manylinux: auto
      - uses: actions/upload-artifact@v4
        with:
          name: wheel-${{ matrix.target }}
          path: dist

  publish:
    needs: wheels
    runs-on: ubuntu-latest
    environment: pypi
    permissions:
      id-token: write
    steps:
      - uses: actions/download-artifact@v4
        with:
          pattern: wheel-*
          merge-multiple: true
          path: dist
      - uses: pypa/gh-action-pypi-publish@release/v1
```

- [ ] **Step 5: Check the wheel builds locally**

Run: `uvx maturin@1.15.0 build --release --manifest-path crates/bridge-screen/Cargo.toml --out target/wheels` (if `uvx` is missing, use `pipx run maturin==1.15.0 build …` with the same arguments).
Expected: one `bridge_screen-0.1.0-*.whl` in `target/wheels`.

- [ ] **Step 6: Document releasing**

Create `docs/RELEASING.md`:

```markdown
# Releasing bridge-screen

## One-time setup (owner)

1. Create the public repo `bedantsharma/homebrew-tap` (empty is fine).
2. Create a GitHub token that can push to that repo and save it in this repo's Actions secrets as `HOMEBREW_TAP_TOKEN`.
3. Create an npm access token (publish rights) and save it as the Actions secret `NPM_TOKEN`.
4. On pypi.org, add a "pending trusted publisher" for project `bridge-screen`: owner `bedantsharma`, repo `session-plugin`, workflow `pypi.yml`, environment `pypi`. Create the `pypi` environment under this repo's Settings → Environments.

## Cutting a release

1. Bump `version` in the root `Cargo.toml` (`[workspace.package]`) and commit.
2. `dist plan` to check the release.
3. `git tag v<version> && git push origin v<version>`.
4. The `Release` workflow builds binaries, the GitHub Release, installers, the Homebrew formula and npm packages. The `PyPI` workflow uploads wheels.

## What users run

- `brew install bedantsharma/tap/bridge-screen`
- `pipx install bridge-screen` or `uv tool install bridge-screen`
- `npx bridge-screen`
- `curl -LsSf https://github.com/bedantsharma/session-plugin/releases/latest/download/bridge-screen-installer.sh | sh`
```

In `CLAUDE.md`, under `## Commands`, add the line:

```markdown
- Releases: see `docs/RELEASING.md` (`dist plan` to preview)
```

- [ ] **Step 7: Verify nothing else broke**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && dist plan`
Expected: all pass; `dist plan` succeeds.

- [ ] **Step 8: Commit**

```bash
git add dist-workspace.toml .github/workflows crates/bridge-screen/pyproject.toml crates/bridge-screen-core/Cargo.toml docs/RELEASING.md CLAUDE.md
git commit -m "release: cargo-dist pipeline, PyPI wheels, releasing guide

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
