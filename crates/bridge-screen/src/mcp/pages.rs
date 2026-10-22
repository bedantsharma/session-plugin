//! The MCP side of project pages: `lead_pages`, `lead_page_read`, `lead_page_write`
//! (spec 2026-10-22). The repo comes from the calling session, so every worktree of a repo
//! reaches the same pages.

use std::fmt::Write as _;

use bridge_screen_core::pages::{self, Page, PagesState, WriteMode};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Args, Bridge, msg};

/// What to do with the repo's project pages.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeadPagesAction {
    /// Create the five pages (only after the owner said yes).
    SetUp,
    /// Record that the owner said no: nothing more is offered.
    Decline,
    /// List the pages with their last-updated dates.
    Status,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeadPagesInput {
    pub action: LeadPagesAction,
    /// The owner's own words answering whether to set up project pages. Required for
    /// `set_up` and `decline`.
    #[serde(default)]
    pub owner_said: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeadPageReadInput {
    /// One of: intent, project-map, current-state, decision-log, glossary.
    pub page: String,
}

/// How a write changes the page.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageWriteMode {
    /// Replace the whole page (the default).
    Replace,
    /// Add to the end of the page (the only way to change the decision log).
    Append,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeadPageWriteInput {
    /// One of: intent, project-map, current-state, decision-log, glossary.
    pub page: String,
    /// The new page text (replace) or the entry to add (append). At most 64 KiB.
    pub content: String,
    /// "replace" (default) or "append".
    #[serde(default)]
    pub mode: Option<PageWriteMode>,
    /// Why you are writing it, in one line.
    pub reason: String,
    /// The owner's own words. Required to change the intent page.
    #[serde(default)]
    pub owner_said: Option<String>,
}

const NAMES: &str = "intent, project-map, current-state, decision-log, glossary";

const NEXT_STEPS: &str = "Next: (1) Draft current-state from the repo's evidence (recent commits, the plan, the README) and save it with lead_page_write; write \"not known yet\" for anything you can't show, never a guess. (2) Draft intent (purpose, what done looks like, what is not being done, hard constraints, current focus), show it to the owner once, and save it only with their words in owner_said. (3) Add the terms you already know to glossary. (4) Use the lead-briefing skill for every report to the owner, and keep current-state true after each merge.";

fn parse_page(name: &str) -> Result<Page, String> {
    Page::parse(name)
        .ok_or_else(|| format!("There is no page called \"{name}\". The pages are: {NAMES}."))
}

fn date(t: Option<jiff::Timestamp>) -> String {
    t.map_or_else(
        || "not written yet".to_string(),
        |t| format!("updated {}", t.to_zoned(jiff::tz::TimeZone::UTC).date()),
    )
}

impl Bridge {
    pub(super) fn do_lead_pages(&self, args: &Args<LeadPagesInput>) -> Result<String, String> {
        let store = self.open()?;
        let session = self.caller(&store, "lead_pages", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let repo = &session.repo_id;
        let said = input.owner_said.as_deref().unwrap_or("");
        match input.action {
            LeadPagesAction::SetUp => {
                let out = pages::set_up(&self.home, repo, said).map_err(msg)?;
                let mut reply = String::from("Project pages are set up");
                if out.created.is_empty() {
                    reply.push_str(": all five already existed, nothing was changed.");
                } else {
                    let names: Vec<&str> = out.created.iter().map(|p| p.name()).collect();
                    let _ = write!(reply, ". Created: {}.", names.join(", "));
                    if !out.kept.is_empty() {
                        let _ = write!(reply, " Kept as they were: {}.", out.kept.len());
                    }
                }
                let _ = write!(
                    reply,
                    " They are kept outside the repo, so every worktree sees the same ones.\n{NEXT_STEPS}"
                );
                Ok(reply)
            }
            LeadPagesAction::Decline => {
                pages::decline(&self.home, repo, said).map_err(msg)?;
                Ok("Noted. bridge-screen won't offer project pages for this repo again. The owner can still run `bridge-screen pages set-up` any time.".to_string())
            }
            LeadPagesAction::Status => {
                let mut reply = match pages::state(&self.home, repo) {
                    PagesState::SetUp => "Project pages are set up.".to_string(),
                    PagesState::Declined => {
                        "The owner declined project pages for this repo; don't offer them again."
                            .to_string()
                    }
                    PagesState::NotSetUp { .. } => {
                        "Project pages are not set up for this repo yet.".to_string()
                    }
                };
                for info in pages::status(&self.home, repo) {
                    let _ = write!(reply, "\n- {}: {}", info.page.name(), date(info.updated));
                }
                Ok(reply)
            }
        }
    }

    pub(super) fn do_lead_page_read(
        &self,
        args: &Args<LeadPageReadInput>,
    ) -> Result<String, String> {
        let store = self.open()?;
        let session = self.caller(&store, "lead_page_read", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let page = parse_page(&input.page)?;
        match pages::read(&self.home, &session.repo_id, page).map_err(msg)? {
            Some(text) => Ok(text),
            None => Ok(
                "Project pages are not set up for this repo yet. Ask the owner whether to set them up, then call lead_pages."
                    .to_string(),
            ),
        }
    }

    pub(super) fn do_lead_page_write(
        &self,
        args: &Args<LeadPageWriteInput>,
    ) -> Result<String, String> {
        let store = self.open()?;
        let session = self.caller(&store, "lead_page_write", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let page = parse_page(&input.page)?;
        if input.reason.trim().is_empty() {
            return Err("Say in one line why you are changing the page (reason).".to_string());
        }
        match pages::state(&self.home, &session.repo_id) {
            PagesState::SetUp => {}
            PagesState::Declined => {
                return Err("The owner declined project pages for this repo, so nothing is written. They can run `bridge-screen pages set-up` to change that.".to_string());
            }
            PagesState::NotSetUp { .. } => {
                return Err("Project pages are not set up for this repo yet. Ask the owner whether to set them up, then call lead_pages.".to_string());
            }
        }
        let mode = match input.mode {
            Some(PageWriteMode::Append) => WriteMode::Append,
            Some(PageWriteMode::Replace) | None => WriteMode::Replace,
        };
        pages::write(
            &self.home,
            &session.repo_id,
            page,
            &input.content,
            mode,
            input.owner_said.as_deref(),
        )
        .map_err(msg)?;
        Ok("Saved.".to_string())
    }
}
