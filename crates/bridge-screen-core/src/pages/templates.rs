//! The five page templates, taken from the lead-briefing skill's living-docs reference.

use super::Page;

/// The starting text of a page. Unknown facts say "not known yet", never a guess.
pub fn template(page: Page) -> &'static str {
    match page {
        Page::Intent => include_str!("templates/intent.md"),
        Page::ProjectMap => include_str!("templates/project-map.md"),
        Page::CurrentState => include_str!("templates/current-state.md"),
        Page::DecisionLog => include_str!("templates/decision-log.md"),
        Page::Glossary => include_str!("templates/glossary.md"),
    }
}
