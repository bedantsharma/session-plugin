//! Routes each stored event to the module that keeps its view tables up to date.

use rusqlite::Transaction;

use crate::error::Result;
use crate::plan::project::{PLAN_KINDS, project as project_plan};
use crate::session::{SESSION_KINDS, project as project_session};
use crate::store::Event;

/// View tables that `Store::rebuild` empties and refills from events.
pub const VIEW_TABLES: &[&str] = &["plan_nodes", "plan_deps", "sessions", "units", "task_time"];

/// Update view tables for one event. Unknown kinds change no views.
pub fn apply(tx: &Transaction, event: &Event) -> Result<()> {
    let kind = event.kind.as_str();
    if PLAN_KINDS.contains(&kind) {
        project_plan(tx, event)?;
    } else if SESSION_KINDS.contains(&kind) {
        project_session(tx, event)?;
    }
    Ok(())
}
