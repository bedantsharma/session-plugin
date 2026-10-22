//! The read model of the dashboard's "Project pages" screen: the five pages as text with
//! plain titles and last-updated dates. Read-only.

use serde::Serialize;

use super::{Page, PagesState, read, state, status};
use crate::error::{Error, Result};
use crate::paths::Home;

/// What the screen shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PagesView {
    /// `set_up`, `not_set_up` or `declined`.
    pub state: &'static str,
    /// The five pages when set up; empty otherwise.
    pub pages: Vec<PageView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageView {
    /// The page's name in plain words ("Where things stand").
    pub title: &'static str,
    /// The day the page was last written (`2026-10-22`).
    pub updated: Option<String>,
    pub text: String,
}

/// The pages of one repo. The id comes from a URL, so anything but a plain id (letters,
/// digits, `-`, `_`) is "not found" rather than a path.
pub fn view(home: &Home, repo_id: &str) -> Result<PagesView> {
    let plain = !repo_id.is_empty()
        && repo_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !plain {
        return Err(Error::NotFound(format!("no project {repo_id}")));
    }
    let (state, pages) = match state(home, repo_id) {
        PagesState::Declined => ("declined", Vec::new()),
        PagesState::NotSetUp { .. } => ("not_set_up", Vec::new()),
        PagesState::SetUp => {
            let info = status(home, repo_id);
            let mut pages = Vec::with_capacity(Page::ALL.len());
            for page in Page::ALL {
                let updated = info
                    .iter()
                    .find(|i| i.page == page)
                    .and_then(|i| i.updated)
                    .map(|t| t.to_zoned(jiff::tz::TimeZone::UTC).date().to_string());
                pages.push(PageView {
                    title: page.title(),
                    updated,
                    text: read(home, repo_id, page)?.unwrap_or_default(),
                });
            }
            ("set_up", pages)
        }
    };
    Ok(PagesView { state, pages })
}
