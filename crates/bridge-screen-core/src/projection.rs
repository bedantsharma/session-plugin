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
