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
pub fn import(
    dir: &Path,
    file: Option<&Path>,
    into: Option<&str>,
    track: Option<u32>,
    dry_run: bool,
) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let ctx = ImportCtx {
        session_id: OWNER_SESSION.to_string(),
        actor: Actor::default(),
        dry_run,
        track,
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
                let open = bridge_screen_core::plan::track::open_tracks(store.conn(), &repo.id)?;
                if s.baseline || open.len() < 2 {
                    println!("{}", import::needs_link_line(&s.path));
                } else {
                    println!(
                        "{}",
                        import::needs_track_line(
                            &s.path,
                            &bridge_screen_core::plan::track::open_list_text(&open)
                        )
                    );
                }
            }
        }
    }
    Ok(())
}

/// `bridge-screen import --fresh | --done <file>…`: the owner's answer to "which plan
/// files are already finished?" (an owner action, stale-click checked).
pub fn import_scope(dir: &Path, fresh: bool, done: &[String]) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let done = if fresh || done.is_empty() {
        None
    } else {
        Some(done.join(","))
    };
    crate::commands::owner_cmd(
        &home,
        &mut store,
        Some(dir),
        &OwnerAction::PlanScope {
            repo: repo.id,
            done,
        },
    )
}

/// `bridge-screen run close [--track N] [-C DIR]`: close a track's finished run (an owner
/// action).
pub fn run_close(dir: &Path, track: Option<u32>) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    crate::commands::owner_cmd(
        &home,
        &mut store,
        Some(dir),
        &OwnerAction::CloseRun {
            repo: repo.id,
            track,
        },
    )
}

/// `1 minute`, `3 hours`, `2 days`.
fn age_words(secs: i64) -> String {
    let (n, unit) = if secs < 3600 {
        ((secs / 60).max(1), "minute")
    } else if secs < 86_400 {
        (secs / 3600, "hour")
    } else {
        (secs / 86_400, "day")
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// The glance of this repo, for the hide commands.
fn repo_glance(
    store: &bridge_screen_core::store::Store,
    home: &Home,
    repo_id: &str,
) -> Result<bridge_screen_core::view::Glance> {
    let cfg = bridge_screen_core::config::Config::load(&home.config_path())?;
    Ok(
        bridge_screen_core::view::glance(store.conn(), &cfg, jiff::Timestamp::now())?
            .for_repo(repo_id),
    )
}

/// `bridge-screen hidden`: what is hidden from the front screen, newest first.
pub fn hidden(dir: &Path) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let store = open_store(&home)?;
    store.register_repo(&repo)?;
    let g = repo_glance(&store, &home, &repo.id)?;
    if g.hidden.is_empty() {
        println!("Nothing is hidden in {}.", repo.name);
    }
    for (i, h) in g.hidden.iter().enumerate() {
        println!(
            "{}. {} '{}' — hidden {} ago",
            i + 1,
            h.kind.as_str(),
            h.label,
            age_words(h.age_s)
        );
    }
    Ok(())
}

/// `bridge-screen hide project | track N | card ID`.
pub fn hide(dir: &Path, what: &[String]) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let g = repo_glance(&store, &home, &repo.id)?;
    let (subject, label) = match what.first().map(String::as_str) {
        Some("project") => (
            bridge_screen_core::hide::project_subject(&repo.id),
            repo.name.clone(),
        ),
        Some("track") => {
            let no: u32 = what
                .get(1)
                .and_then(|n| n.parse().ok())
                .with_context(|| "say which track: `hide track 2`")?;
            let t = bridge_screen_core::plan::track::track(store.conn(), &repo.id, no)?
                .with_context(|| format!("there is no track {no}"))?;
            (
                bridge_screen_core::hide::track_subject(&repo.id, no),
                t.title,
            )
        }
        Some("card") => {
            let id = what
                .get(1)
                .with_context(|| "say which card: `hide card ID`")?;
            let card = g
                .needs_you
                .iter()
                .find(|c| c.id == *id || c.id.starts_with(id.as_str()))
                .with_context(|| format!("no card on screen starts with {id}"))?;
            (card.subject.clone(), card.title.clone())
        }
        _ => anyhow::bail!("hide what? `hide project`, `hide track N` or `hide card ID`"),
    };
    let stamp = bridge_screen_core::hide::stamp_of(
        store.conn(),
        &repo.id,
        &bridge_screen_core::hide::Subject::parse(&subject)
            .with_context(|| "that can't be hidden")?,
    )?;
    crate::commands::owner_cmd(
        &home,
        &mut store,
        Some(dir),
        &OwnerAction::Hide {
            repo: repo.id,
            subject,
            label,
            stamp,
        },
    )
}

/// `bridge-screen unhide N | <subject>`.
pub fn unhide(dir: &Path, what: &str) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let g = repo_glance(&store, &home, &repo.id)?;
    let subject = match what.parse::<usize>() {
        Ok(n) => g
            .hidden
            .get(n.saturating_sub(1))
            .map(|h| h.subject.clone())
            .with_context(|| format!("nothing hidden is numbered {n}; see `hidden`"))?,
        Err(_) => what.to_string(),
    };
    crate::commands::owner_cmd(
        &home,
        &mut store,
        Some(dir),
        &OwnerAction::Unhide {
            repo: repo.id,
            subject,
        },
    )
}

/// `bridge-screen remove <id> [--yes]`: remove abandoned work for good (an owner action).
/// Says what goes and asks first, unless `--yes`.
pub fn remove(dir: &Path, id: &str, yes: bool) -> Result<()> {
    use std::io::{BufRead as _, Write as _};
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let impact = bridge_screen_core::owner::remove_impact(store.conn(), &repo.id, id)?;
    println!("{}", impact.text);
    if !yes {
        print!("Remove it for good? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Kept it. Nothing was removed.");
            return Ok(());
        }
    }
    crate::commands::owner_cmd(
        &home,
        &mut store,
        Some(dir),
        &OwnerAction::Remove {
            repo: repo.id,
            id: id.to_string(),
            title: impact.title,
            confirm: true,
        },
    )
}

/// `bridge-screen track list|start|close|reopen`: the folder's tracks. Starting, finishing
/// and reopening are owner actions (`owner::act`).
pub fn track(dir: &Path, action: crate::cli::TrackCommand) -> Result<()> {
    use crate::cli::TrackCommand;
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let act = |store: &mut bridge_screen_core::store::Store, a: OwnerAction| {
        crate::commands::owner_cmd(&home, store, Some(dir), &a)
    };
    match action {
        TrackCommand::List => {
            let all = bridge_screen_core::plan::track::tracks(store.conn(), &repo.id)?;
            if all.is_empty() {
                println!("No tracks in {} yet.", repo.name);
            }
            for t in all {
                let p = bridge_screen_core::verify::progress(store.conn(), &repo.id, t.no)?;
                println!(
                    "{}  {}{}  {}% done",
                    t.no,
                    t.title,
                    if t.is_open() { "" } else { " (finished)" },
                    p.done_pct
                );
            }
            Ok(())
        }
        TrackCommand::Start { goal, intent } => act(
            &mut store,
            OwnerAction::StartTrack {
                repo: repo.id.clone(),
                title: goal,
                intent,
            },
        ),
        TrackCommand::Close { number } => act(
            &mut store,
            OwnerAction::CloseTrack {
                repo: repo.id.clone(),
                track: number,
            },
        ),
        TrackCommand::Reopen { number } => act(
            &mut store,
            OwnerAction::ReopenTrack {
                repo: repo.id.clone(),
                track: number,
            },
        ),
    }
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
            "\n== {} ({}) — {when}, {} KiB ==",
            info.page.title(),
            info.page.file_name(),
            info.bytes.div_ceil(1024)
        );
        match pages::read(home, repo_id, info.page) {
            Ok(Some(text)) => {
                let _ = writeln!(out, "{}", text.trim_end());
            }
            Ok(None) => {}
            Err(e) => {
                let _ = writeln!(out, "(could not be read: {e})");
            }
        }
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
        if matches!(o, ImportOutcome::NeedsScope { .. }) {
            let _ = writeln!(out, "{}", import::scope::HOW_TO_ANSWER);
        }
        if dry_run && let ImportOutcome::Imported { changes, .. } = o {
            for c in changes {
                let _ = writeln!(out, "  - would {c}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use bridge_screen_core::pages;
    use bridge_screen_core::paths::Home;

    use super::render_pages;
    use crate::words::jargon_in;

    #[test]
    fn pages_show_reads_in_plain_words_in_every_state() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::at(dir.path());
        let mut shown = vec![render_pages(&home, "r1", "shop")];
        pages::decline(&home, "r1", "no").unwrap();
        shown.push(render_pages(&home, "r1", "shop"));
        pages::set_up(&home, "r1", "yes").unwrap();
        shown.push(render_pages(&home, "r1", "shop"));
        // The folder path is part of the message; a temp folder's random name (macOS ones hold
        // underscores) is not owner-facing wording, so leave it out of the jargon check.
        let folder = dir.path().display().to_string();
        for text in shown {
            let text = text.replace(&folder, "<folder>");
            assert_eq!(jargon_in(&text), Vec::<String>::new(), "{text}");
        }
    }
}
