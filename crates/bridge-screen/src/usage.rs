//! `bridge-screen usage`: tokens and cost in plain words (spec §9.2), and the owner's
//! catch-up of transcripts the hooks haven't fully read yet.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use bridge_screen_core::paths::Home;
use bridge_screen_core::repo;
use bridge_screen_core::telemetry::{self, Scope};

use crate::cli::UsageBy;
use crate::commands::open_store;

pub fn run(
    dir: &Path,
    session: Option<&str>,
    task: Option<&str>,
    by: UsageBy,
    catch_up: bool,
) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let mut text = String::new();
    if catch_up {
        let done = telemetry::catch_up(&mut store, &repo.id)?;
        if done.replies == 0 {
            let _ = writeln!(
                text,
                "Nothing new: {} transcript(s) of {} are fully read.",
                done.files, repo.name
            );
        } else {
            let _ = writeln!(
                text,
                "Read {} transcript(s) of {}: {} replies, {} tokens added.",
                done.files,
                repo.name,
                done.replies,
                telemetry::tokens_text(done.tokens)
            );
        }
    }
    let _ = (session, task, by);
    let total = telemetry::totals(store.conn(), &Scope::Repo(repo.id.clone()))?;
    let _ = writeln!(
        text,
        "{}: {} tokens",
        repo.name,
        telemetry::tokens_text(total.sum.total())
    );
    print!("{text}");
    Ok(())
}
