//! Decisions (spec §8.6): what was decided, why, and the alternatives, captured at task
//! completion, before compaction and at session end. A changed decision supersedes the
//! one it replaces; the same decision logged twice on a task merges into one row.

pub mod project;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::error::Result;

pub use project::{DECISION_KINDS, DecisionData, project};

/// Longest `what` kept, in characters.
pub const MAX_WHAT: usize = 200;
/// Longest `why` kept.
pub const MAX_WHY: usize = 500;
/// Most alternatives kept, each at most [`MAX_WHAT`] characters.
pub const MAX_ALTERNATIVES: usize = 5;

/// One row of `decisions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub decision_id: String,
    pub repo_id: String,
    pub task_id: Option<String>,
    pub session_id: String,
    pub what: String,
    pub why: String,
    pub alternatives: Vec<String>,
    pub supersedes: Option<String>,
    pub superseded_by: Option<String>,
    /// How many times it was logged (duplicates merge).
    pub merged_count: i64,
    pub ts: String,
    pub updated_ts: String,
}

/// The duplicate key of a decision: lowercase, whitespace collapsed, trailing punctuation
/// stripped.
pub fn what_key(what: &str) -> String {
    what.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .trim_end()
        .to_string()
}

/// `text` trimmed and cut to at most `max` characters (on a char boundary).
pub fn clip(text: &str, max: usize) -> String {
    text.trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_string()
}

const DECISION_COLUMNS: &str = "decision_id, repo_id, task_id, session_id, what, why,
     alternatives, supersedes, superseded_by, merged_count, ts, updated_ts";

fn read_decision(r: &Row) -> rusqlite::Result<(Decision, String)> {
    Ok((
        Decision {
            decision_id: r.get(0)?,
            repo_id: r.get(1)?,
            task_id: r.get(2)?,
            session_id: r.get(3)?,
            what: r.get(4)?,
            why: r.get(5)?,
            alternatives: Vec::new(),
            supersedes: r.get(7)?,
            superseded_by: r.get(8)?,
            merged_count: r.get(9)?,
            ts: r.get(10)?,
            updated_ts: r.get(11)?,
        },
        r.get(6)?,
    ))
}

fn with_alternatives((mut d, alternatives): (Decision, String)) -> Result<Decision> {
    d.alternatives = serde_json::from_str(&alternatives)?;
    Ok(d)
}

/// The number in a `D<n>` id (0 for anything else).
fn number(id: &str) -> i64 {
    id.strip_prefix('D')
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// One decision of a repo.
pub fn get(conn: &Connection, repo_id: &str, decision_id: &str) -> Result<Option<Decision>> {
    conn.prepare_cached(&format!(
        "SELECT {DECISION_COLUMNS} FROM decisions WHERE repo_id = ?1 AND decision_id = ?2"
    ))?
    .query_row(params![repo_id, decision_id], read_decision)
    .optional()?
    .map(with_alternatives)
    .transpose()
}

/// A repo's decisions (optionally on one task), in id order; superseded ones only with
/// `include_superseded`.
pub fn list(
    conn: &Connection,
    repo_id: &str,
    task_id: Option<&str>,
    include_superseded: bool,
) -> Result<Vec<Decision>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DECISION_COLUMNS} FROM decisions
         WHERE repo_id = ?1 AND (?2 IS NULL OR task_id = ?2)
           AND (?3 = 1 OR superseded_by IS NULL)"
    ))?;
    let rows = stmt.query_map(params![repo_id, task_id, include_superseded], read_decision)?;
    let mut out = rows
        .map(|r| with_alternatives(r?))
        .collect::<Result<Vec<_>>>()?;
    out.sort_by_key(|d| number(&d.decision_id));
    Ok(out)
}

/// The decisions a session logged, in id order.
pub fn by_session(conn: &Connection, session_id: &str) -> Result<Vec<Decision>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DECISION_COLUMNS} FROM decisions WHERE session_id = ?1"
    ))?;
    let rows = stmt.query_map([session_id], read_decision)?;
    let mut out = rows
        .map(|r| with_alternatives(r?))
        .collect::<Result<Vec<_>>>()?;
    out.sort_by_key(|d| number(&d.decision_id));
    Ok(out)
}

/// The supersede chain `id` belongs to, oldest → newest.
pub fn chain(conn: &Connection, repo_id: &str, id: &str) -> Result<Vec<Decision>> {
    let Some(mut first) = get(conn, repo_id, id)? else {
        return Ok(Vec::new());
    };
    // Walk back to the start (bounded: a chain can't be longer than the repo's decisions).
    let mut guard = 0;
    while let Some(prev) = first.supersedes.clone() {
        guard += 1;
        match get(conn, repo_id, &prev)? {
            Some(d) if guard < 10_000 => first = d,
            _ => break,
        }
    }
    let mut out = vec![first];
    while let Some(next) = out.last().and_then(|d| d.superseded_by.clone()) {
        if out.len() >= 10_000 || out.iter().any(|d| d.decision_id == next) {
            break;
        }
        match get(conn, repo_id, &next)? {
            Some(d) => out.push(d),
            None => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
