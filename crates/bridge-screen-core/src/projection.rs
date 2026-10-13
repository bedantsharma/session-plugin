//! Routes each stored event to the module that keeps its view tables up to date.

use rusqlite::Transaction;

use crate::error::Result;
use crate::store::Event;

/// View tables that `Store::rebuild` empties and refills from events.
pub const VIEW_TABLES: &[&str] = &[];

/// Update view tables for one event. Unknown kinds change no views.
#[allow(clippy::unnecessary_wraps)] // Task 4 routes plan events here.
pub fn apply(_tx: &Transaction, _event: &Event) -> Result<()> {
    Ok(())
}
