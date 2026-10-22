//! Getting a repo onto bridge-screen: `bridge-screen import` (plan files, spec §12.1) and
//! `bridge-screen init` (a half-built repo, spec §12.2).

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use bridge_screen_core::import::{self, ImportCtx, ImportOutcome, ScanBudget};
use bridge_screen_core::owner::OwnerAction;
use bridge_screen_core::pages::{self, PagesState};
use bridge_screen_core::paths::Home;
use bridge_screen_core::repo;
use bridge_screen_core::store::{Actor, OWNER_SESSION};
use bridge_screen_core::waits;

use crate::cli::{PagesCommand, WaitCommand};

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
    if file.is_none() {
        // Files still waiting for a link, from earlier scans.
        for s in import::sources(store.conn(), &repo.id)? {
            let said = outcomes.iter().any(|o| o.path() == s.path);
            if s.needs_link && !said {
                println!("{}", import::needs_link_line(&s.path));
            }
        }
    }
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

/// `bridge-screen wait list|clear <id>|promote <id> [-C DIR]`. Clearing and promoting are
/// owner actions (`owner::act`, stale-click checked).
pub fn wait(dir: &Path, action: WaitCommand) -> Result<()> {
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    match action {
        WaitCommand::List => {
            let repo = repo::identify(dir)?;
            store.register_repo(&repo)?;
            let all = waits::of_repo(store.conn(), &repo.id)?;
            if all.is_empty() {
                println!("No waits in {}.", repo.name);
            }
            let now = jiff::Timestamp::now();
            for w in all {
                let session: String = w.session_id.chars().take(6).collect();
                let state = match (w.cleared_why, &w.cleared_sha) {
                    (None, _) => format!("open for {}m", w.since_min(now)),
                    (Some(waits::ClearedWhy::TargetDone), Some(sha)) => {
                        format!("done @{}", sha.chars().take(7).collect::<String>())
                    }
                    (Some(waits::ClearedWhy::Promoted), _) => "now a plan link".to_string(),
                    (Some(_), _) => "cleared by hand".to_string(),
                };
                println!(
                    "{}  {session}  waiting on {} — {} ({state})",
                    w.wait_id, w.target_text, w.reason
                );
            }
            Ok(())
        }
        WaitCommand::Clear { wait_id } => crate::commands::owner_cmd(
            &home,
            &mut store,
            Some(dir),
            &OwnerAction::ClearWait { wait_id },
        ),
        WaitCommand::Promote { wait_id } => crate::commands::owner_cmd(
            &home,
            &mut store,
            Some(dir),
            &OwnerAction::PromoteWait { wait_id },
        ),
    }
}

/// `bridge-screen pages set-up|decline|show|path [-C DIR]`: the owner's own route to the
/// project pages. Running `set-up` or `decline` is the owner's yes or no. Needs no database,
/// only the repo's id (the same for every worktree).
pub fn pages(dir: &Path, action: &PagesCommand) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let folder = home.lead_dir(&repo.id);
    match action {
        PagesCommand::Path => println!("{}", folder.display()),
        PagesCommand::SetUp => {
            let out = pages::set_up(
                &home,
                &repo.id,
                "set up by the owner with `bridge-screen pages set-up`",
            )?;
            if out.created.is_empty() {
                println!(
                    "Project pages for {} were already set up in {}; nothing changed.",
                    repo.name,
                    folder.display()
                );
            } else {
                println!(
                    "Project pages are set up for {}: {} written to {}. Every worktree of this repo reads the same pages; nothing was written into the repo.",
                    repo.name,
                    if out.created.len() == 5 {
                        "five pages".to_string()
                    } else {
                        format!("{} missing page(s)", out.created.len())
                    },
                    folder.display()
                );
            }
        }
        PagesCommand::Decline => {
            pages::decline(
                &home,
                &repo.id,
                "declined by the owner with `bridge-screen pages decline`",
            )?;
            println!(
                "Okay: bridge-screen won't offer project pages for {} again. Run `bridge-screen pages set-up` any time to change your mind.",
                repo.name
            );
        }
        PagesCommand::Show => print!("{}", render_pages(&home, &repo.id, &repo.name)),
    }
    Ok(())
}

/// The `pages show` text: where the pages are and what each one holds.
fn render_pages(home: &Home, repo_id: &str, repo_name: &str) -> String {
    let folder = home.lead_dir(repo_id);
    let mut out = String::new();
    match pages::state(home, repo_id) {
        PagesState::SetUp => {
            let _ = writeln!(
                out,
                "Project pages for {repo_name} (in {}):",
                folder.display()
            );
        }
        PagesState::Declined => {
            let _ = writeln!(
                out,
                "You chose not to have project pages for {repo_name}. Run `bridge-screen pages set-up` to change that."
            );
            return out;
        }
        PagesState::NotSetUp { .. } => {
            let _ = writeln!(
                out,
                "Project pages are not set up for {repo_name}. Run `bridge-screen pages set-up` to create them in {}.",
                folder.display()
            );
            return out;
        }
    }
    for info in pages::status(home, repo_id) {
        let when = info.updated.map_or_else(
            || "not written yet".to_string(),
            |t| format!("updated {}", t.to_zoned(jiff::tz::TimeZone::UTC).date()),
        );
        let _ = writeln!(
            out,
            "  {} ({}) — {when}, {} KiB",
            info.page.title(),
            info.page.file_name(),
            info.bytes.div_ceil(1024)
        );
    }
    out
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
