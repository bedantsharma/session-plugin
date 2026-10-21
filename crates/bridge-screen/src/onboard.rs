//! Getting a repo onto bridge-screen: `bridge-screen import` (plan files, spec §12.1) and
//! `bridge-screen init` (a half-built repo, spec §12.2).

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use bridge_screen_core::import::{self, ImportCtx, ImportOutcome, ScanBudget};
use bridge_screen_core::paths::Home;
use bridge_screen_core::repo;
use bridge_screen_core::store::{Actor, OWNER_SESSION};

use crate::commands::open_store;

/// `bridge-screen import [-C DIR] [FILE] [--into NODE] [--dry-run]`.
pub fn import(dir: &Path, file: Option<&Path>, into: Option<&str>, dry_run: bool) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let ctx = ImportCtx {
        session_id: OWNER_SESSION.to_string(),
        actor: Actor::default(),
        dry_run,
    };
    let outcomes = match file {
        Some(file) => {
            let path = std::fs::canonicalize(file)
                .with_context(|| format!("reading {}", file.display()))?;
            // Paths are kept relative to the checkout the file is in.
            let root = path
                .parent()
                .and_then(|p| repo::worktree_toplevel(p).ok())
                .unwrap_or_else(|| repo.root.clone());
            vec![import::import_path(
                &mut store, &repo.id, &root, &path, &ctx, into,
            )?]
        }
        None => import::scan(
            &mut store,
            &repo.id,
            &repo.root,
            &ctx,
            ScanBudget::UNLIMITED,
        )?,
    };
    print!("{}", render(&outcomes, dry_run));
    Ok(())
}

/// `bridge-screen init [-C DIR] [--goal TEXT]`: the evidence and the agent's steps. Reads
/// only (git and files); writes nothing but the repo's registration.
pub fn init(dir: &Path, goal: Option<&str>) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let store = open_store(&home)?;
    store.register_repo(&repo)?;
    let wt = repo::worktree_toplevel(dir).unwrap_or_else(|_| repo.root.clone());
    let evidence = bridge_screen_core::init::evidence(store.conn(), &repo.id, &wt)?;
    print!(
        "{}",
        bridge_screen_core::init::render(&evidence, &repo.name, goal)
    );
    Ok(())
}

/// What an import did, in plain words.
pub fn render(outcomes: &[ImportOutcome], dry_run: bool) -> String {
    let mut out = String::new();
    if outcomes.is_empty() {
        out.push_str("No new or changed plan files.\n");
    }
    if dry_run {
        out.push_str("Dry run: nothing was written.\n");
    }
    for o in outcomes {
        match o.summary() {
            Some(line) => {
                out.push_str(&line);
                out.push('\n');
            }
            None => {
                let _ = writeln!(out, "{}: unchanged", import::short_name(o.path()));
            }
        }
        if dry_run && let ImportOutcome::Imported { changes, .. } = o {
            for c in changes {
                let _ = writeln!(out, "  - would {c}");
            }
        }
    }
    out
}
