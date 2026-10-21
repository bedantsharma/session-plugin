//! Routes each stored event to the module that keeps its view tables up to date.

use rusqlite::Transaction;

use crate::decision::{DECISION_KINDS, project as project_decision};
use crate::error::Result;
use crate::forecast::{FORECAST_KINDS, project as project_forecast};
use crate::handoff::{HANDOFF_KINDS, project as project_handoff};
use crate::plan::project::{PLAN_KINDS, project as project_plan};
use crate::session::{SESSION_KINDS, project as project_session};
use crate::store::Event;
use crate::tangent::{TANGENT_KINDS, project as project_tangent};
use crate::telemetry::{USAGE_KINDS, project as project_usage};
use crate::verify::{VERIFY_KINDS, project as project_verify};

/// View tables that `Store::rebuild` empties and refills from events.
pub const VIEW_TABLES: &[&str] = &[
    "plan_nodes",
    "plan_deps",
    "sessions",
    "units",
    "task_time",
    "detours",
    "suspended_items",
    "drift_samples",
    "repo_tuning",
    "notes_sent",
    "checkpoints",
    "leftovers",
    "handoff_notes",
    "claims_done",
    "verifications",
    "decisions",
    "verify_nudges",
    "repo_settings",
    "usage_replies",
    "transcript_cursors",
    "forecast_history",
    "plan_sources",
    "plan_source_items",
    "waits",
];

/// Update view tables for one event. Unknown kinds change no views.
pub fn apply(tx: &Transaction, event: &Event) -> Result<()> {
    let kind = event.kind.as_str();
    if PLAN_KINDS.contains(&kind) {
        project_plan(tx, event)?;
    } else if SESSION_KINDS.contains(&kind) {
        project_session(tx, event)?;
    } else if TANGENT_KINDS.contains(&kind) {
        project_tangent(tx, event)?;
    } else if HANDOFF_KINDS.contains(&kind) {
        project_handoff(tx, event)?;
    } else if VERIFY_KINDS.contains(&kind) {
        project_verify(tx, event)?;
    } else if DECISION_KINDS.contains(&kind) {
        project_decision(tx, event)?;
    } else if USAGE_KINDS.contains(&kind) {
        project_usage(tx, event)?;
    } else if FORECAST_KINDS.contains(&kind) {
        project_forecast(tx, event)?;
    }
    Ok(())
}
