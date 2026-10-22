//! Project pages: five short Markdown pages per repo (intent, project map, current state,
//! decision log, glossary) kept in `~/.bridge-screen/lead/<repo-id>/`, never inside the repo,
//! so every worktree and session of the repo reads the same ones.
//!
//! State is kept in marker files, not events: `.offered` (how many sessions were offered
//! the setup) and `.declined`. The pages existing means "set up". No schema change;
//! `rebuild` never touches this folder.

mod templates;
#[cfg(test)]
mod tests;

use std::fs::{self, File};
use std::io::{ErrorKind, Write as _};
use std::path::Path;

use crate::error::{Error, Result};
use crate::paths::Home;

/// The largest single write, in bytes.
pub const MAX_WRITE_BYTES: usize = 64 * 1024;

const OFFERED: &str = ".offered";
const DECLINED: &str = ".declined";
const LOCK: &str = ".lock";

/// One of the five fixed pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Intent,
    ProjectMap,
    CurrentState,
    DecisionLog,
    Glossary,
}

impl Page {
    /// All pages, in the order they are created and listed.
    pub const ALL: [Page; 5] = [
        Page::Intent,
        Page::ProjectMap,
        Page::CurrentState,
        Page::DecisionLog,
        Page::Glossary,
    ];

    /// Exact lowercase kebab names only; anything else (paths, `.md`, other case) is `None`.
    pub fn parse(name: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|p| p.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Page::Intent => "intent",
            Page::ProjectMap => "project-map",
            Page::CurrentState => "current-state",
            Page::DecisionLog => "decision-log",
            Page::Glossary => "glossary",
        }
    }

    pub fn file_name(self) -> &'static str {
        match self {
            Page::Intent => "intent.md",
            Page::ProjectMap => "project-map.md",
            Page::CurrentState => "current-state.md",
            Page::DecisionLog => "decision-log.md",
            Page::Glossary => "glossary.md",
        }
    }
}

/// Where a repo stands with its project pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagesState {
    NotSetUp { offers_made: u8 },
    SetUp,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetUpOutcome {
    pub created: Vec<Page>,
    pub kept: Vec<Page>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageInfo {
    pub page: Page,
    /// When the page was last written; `None` if it doesn't exist.
    pub updated: Option<jiff::Timestamp>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Replace,
    Append,
}

fn refuse(text: &str) -> Error {
    Error::Pages(text.to_string())
}

fn blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// Where the repo stands. Never fails: an unreadable folder counts as "not set up, no offers".
pub fn state(home: &Home, repo_id: &str) -> PagesState {
    let dir = home.lead_dir(repo_id);
    if dir.join(DECLINED).exists() {
        return PagesState::Declined;
    }
    if Page::ALL
        .into_iter()
        .all(|p| dir.join(p.file_name()).is_file())
    {
        return PagesState::SetUp;
    }
    let offers_made = fs::read_to_string(dir.join(OFFERED))
        .ok()
        .and_then(|t| t.trim().parse::<u8>().ok())
        .unwrap_or(0);
    PagesState::NotSetUp { offers_made }
}

/// Create the pages that are missing from the built-in templates; never overwrite one.
pub fn set_up(home: &Home, repo_id: &str, owner_said: &str) -> Result<SetUpOutcome> {
    if blank(owner_said) {
        return Err(refuse(
            "Project pages are set up only after the owner says yes. Ask them first, then pass their words.",
        ));
    }
    let dir = home.lead_dir(repo_id);
    fs::create_dir_all(&dir)?;
    let _lock = lock(&dir)?;
    let mut out = SetUpOutcome {
        created: Vec::new(),
        kept: Vec::new(),
    };
    for page in Page::ALL {
        if dir.join(page.file_name()).is_file() {
            out.kept.push(page);
        } else {
            write_atomic(&dir, page.file_name(), templates::template(page))?;
            out.created.push(page);
        }
    }
    remove_if_present(&dir.join(DECLINED))?;
    remove_if_present(&dir.join(OFFERED))?;
    Ok(out)
}

/// Record that the owner said no: no more offers for this repo.
pub fn decline(home: &Home, repo_id: &str, owner_said: &str) -> Result<()> {
    if blank(owner_said) {
        return Err(refuse(
            "Declining project pages needs the owner's answer. Ask them first, then pass their words.",
        ));
    }
    if state(home, repo_id) == PagesState::SetUp {
        return Err(refuse(
            "Project pages are already set up here, so there is nothing to decline.",
        ));
    }
    let dir = home.lead_dir(repo_id);
    fs::create_dir_all(&dir)?;
    write_atomic(&dir, DECLINED, owner_said.trim())
}

/// The page text, or `None` if the page doesn't exist yet.
pub fn read(home: &Home, repo_id: &str, page: Page) -> Result<Option<String>> {
    match fs::read_to_string(home.lead_dir(repo_id).join(page.file_name())) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Every page with its last-updated time and size (missing pages have no time).
pub fn status(home: &Home, repo_id: &str) -> Vec<PageInfo> {
    let dir = home.lead_dir(repo_id);
    Page::ALL
        .into_iter()
        .map(|page| {
            let meta = fs::metadata(dir.join(page.file_name())).ok();
            PageInfo {
                page,
                updated: meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| jiff::Timestamp::try_from(t).ok()),
                bytes: meta.map_or(0, |m| m.len()),
            }
        })
        .collect()
}

/// Write a page. The intent needs the owner's words; the decision log only grows.
pub fn write(
    home: &Home,
    repo_id: &str,
    page: Page,
    content: &str,
    mode: WriteMode,
    owner_said: Option<&str>,
) -> Result<()> {
    if content.len() > MAX_WRITE_BYTES {
        return Err(refuse(
            "That page text is too long (over 64 KiB). Write a shorter version.",
        ));
    }
    if page == Page::Intent && owner_said.is_none_or(blank) {
        return Err(refuse(
            "The intent page belongs to the owner. Ask them, then pass their words to change it.",
        ));
    }
    if page == Page::DecisionLog && mode == WriteMode::Replace {
        return Err(refuse(
            "The decision log only grows: add to it with mode \"append\" instead of replacing it.",
        ));
    }
    let dir = home.lead_dir(repo_id);
    fs::create_dir_all(&dir)?;
    let _lock = lock(&dir)?;
    let target = dir.join(page.file_name());
    let current = match fs::read_to_string(&target) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(old) = &current {
        write_atomic(&dir, &format!("{}.prev", page.file_name()), old)?;
    }
    let next = match (mode, current) {
        (WriteMode::Append, Some(old)) if !blank(&old) => {
            format!("{}\n\n{}\n", old.trim_end(), content.trim_end())
        }
        _ => content.to_string(),
    };
    write_atomic(&dir, page.file_name(), &next)
}

/// Sessions are offered the setup at most this many times.
pub const MAX_OFFERS: u8 = 3;

/// The line at the end of a SessionStart briefing, if any (spec 2026-10-22, "Briefing pointer").
///
/// `room` is how many lines are left under the briefing's cap. Pages set up: one pointer
/// line (needs 1 line). Not set up and fewer than [`MAX_OFFERS`] offers made: a two-line
/// offer (needs 2 lines, and no other onboarding note in the briefing), counted in
/// `.offered` with one small file write. Declined, offers used up or no room: nothing,
/// and nothing is written. Any problem with the folder is an `Err` the caller logs and
/// drops, so the briefing is never lost.
pub fn briefing_note(
    home: &Home,
    repo_id: &str,
    room: usize,
    other_onboarding_note: bool,
) -> Result<Option<String>> {
    match state(home, repo_id) {
        PagesState::SetUp => {
            if room < 1 {
                return Ok(None);
            }
            let latest = status(home, repo_id)
                .into_iter()
                .filter_map(|i| i.updated)
                .max();
            let when = latest.map_or_else(
                || "unknown".to_string(),
                |t| t.to_zoned(jiff::tz::TimeZone::UTC).date().to_string(),
            );
            Ok(Some(format!(
                "Project pages: read intent and current state first (lead_page_read); keep current state true after each merge. Last updated {when}."
            )))
        }
        PagesState::NotSetUp { offers_made } if offers_made < MAX_OFFERS => {
            if room < 2 || other_onboarding_note {
                return Ok(None);
            }
            let dir = home.lead_dir(repo_id);
            fs::create_dir_all(&dir)?;
            write_atomic(&dir, OFFERED, &(offers_made + 1).to_string())?;
            Ok(Some(OFFER.to_string()))
        }
        PagesState::NotSetUp { .. } | PagesState::Declined => Ok(None),
    }
}

const OFFER: &str = "bridge-screen: this repo has no project pages yet. Ask the owner once, in plain words, whether they want a short set of pages (what the project is for, how it fits together, where things stand, why choices were made, the words we use) that every session can read.\nRelay their answer with lead_pages (action set_up or decline, owner_said = their words). Do not set them up without a yes.";

/// An exclusive advisory lock on `<dir>/.lock`, held until the returned file drops.
fn lock(dir: &Path) -> Result<File> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK))?;
    file.lock()?;
    Ok(file)
}

/// Temp file in the same folder, then rename.
fn write_atomic(dir: &Path, file_name: &str, text: &str) -> Result<()> {
    let tmp = dir.join(format!("{file_name}.tmp"));
    let mut file = File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, dir.join(file_name))?;
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
