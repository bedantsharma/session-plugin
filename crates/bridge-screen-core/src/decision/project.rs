//! Decision event payloads and the projector for `decisions` and the decision-capture
//! flag on `sessions` (spec §8.6).

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::{MAX_ALTERNATIVES, what_key};
use crate::error::Result;
use crate::store::Event;

/// Event kinds routed to this projector.
pub const DECISION_KINDS: &[&str] = &["decision", "decision_asked"];

/// The `decision` event. A `decision_id` that already exists merges into that row (the
/// same decision on the same task, logged again).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionData {
    pub decision_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub what: String,
    pub why: String,
    #[serde(default)]
    pub alternatives: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
}

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

/// Apply one decision event to the decision views.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    if event.kind == "decision" {
        decision(tx, event, &payload(event)?)?;
    }
    // Either way the ask has been answered or made: the flag is cleared.
    tx.execute(
        "UPDATE sessions SET decision_capture = NULL WHERE session_id = ?1",
        [&event.session_id],
    )?;
    Ok(())
}

fn decision(tx: &Transaction, event: &Event, d: &DecisionData) -> Result<()> {
    let repo = event.repo_id.as_str();
    let existing: Option<String> = tx
        .query_row(
            "SELECT alternatives FROM decisions WHERE repo_id = ?1 AND decision_id = ?2",
            params![repo, d.decision_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(alternatives) = existing {
        // A duplicate: the newer reasoning wins, alternatives are kept in order.
        let mut merged: Vec<String> = serde_json::from_str(&alternatives)?;
        for a in &d.alternatives {
            if !merged.contains(a) {
                merged.push(a.clone());
            }
        }
        merged.truncate(MAX_ALTERNATIVES);
        tx.execute(
            "UPDATE decisions SET why = ?3, alternatives = ?4,
                                      merged_count = merged_count + 1, updated_ts = ?5,
                                      supersedes = COALESCE(supersedes, ?6)
                 WHERE repo_id = ?1 AND decision_id = ?2",
            params![
                repo,
                d.decision_id,
                d.why,
                serde_json::to_string(&merged)?,
                event.ts,
                d.supersedes
            ],
        )?;
    } else {
        let mut alternatives = d.alternatives.clone();
        alternatives.truncate(MAX_ALTERNATIVES);
        tx.execute(
            "INSERT INTO decisions (decision_id, repo_id, task_id, session_id, what,
                                        what_key, why, alternatives, supersedes, ts,
                                        updated_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![
                d.decision_id,
                repo,
                d.task_id,
                event.session_id,
                d.what,
                what_key(&d.what),
                d.why,
                serde_json::to_string(&alternatives)?,
                d.supersedes,
                event.ts
            ],
        )?;
    }
    if let Some(old) = &d.supersedes {
        tx.execute(
            "UPDATE decisions SET superseded_by = ?3
             WHERE repo_id = ?1 AND decision_id = ?2 AND superseded_by IS NULL",
            params![repo, old, d.decision_id],
        )?;
    }
    Ok(())
}
