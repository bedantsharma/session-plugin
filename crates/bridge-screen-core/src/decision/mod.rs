//! Decisions (spec §8.6): what was decided, why, and the alternatives, captured at task
//! completion, before compaction and at session end. A changed decision supersedes the
//! one it replaces; the same decision logged twice on a task merges into one row.

pub mod project;

use std::fmt::Write as _;

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

/// The ask that goes out at task completion, with the handoff note, and with the first
/// note after a compaction (spec §8.6).
pub const DECISION_ASK: &str =
    "Log any decisions this task settled (log_decision): what, why, alternatives.";

/// Which task a decision is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionTask {
    /// The session's active task (none if it has none).
    Active,
    /// No task: a repo-wide decision.
    RepoWide,
    Task(String),
}

/// A `log_decision` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRequest {
    pub session_id: String,
    pub task: DecisionTask,
    pub what: String,
    pub why: String,
    pub alternatives: Vec<String>,
    pub supersedes: Option<String>,
    pub actor: crate::store::Actor,
}

/// What `log_decision` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    pub decision_id: String,
    pub task_id: Option<String>,
    /// Merged into an existing decision with the same `what` on the same task.
    pub merged: bool,
    pub supersedes: Option<String>,
}

impl Logged {
    /// The tool result: `Logged D7 (supersedes D4).` or `Merged into D3 (same decision
    /// on 2.3).`
    pub fn reply(&self) -> String {
        if self.merged {
            let on = self
                .task_id
                .as_deref()
                .map_or_else(|| "repo-wide".to_string(), |t| format!("on {t}"));
            return format!("Merged into {} (same decision {on}).", self.decision_id);
        }
        match &self.supersedes {
            Some(old) => format!("Logged {} (supersedes {old}).", self.decision_id),
            None => format!("Logged {}.", self.decision_id),
        }
    }
}

fn refuse(msg: impl Into<String>) -> crate::error::Error {
    crate::error::Error::Verify(msg.into())
}

/// The next `D<n>` id of a repo.
fn next_id(conn: &Connection, repo_id: &str) -> Result<String> {
    let mut stmt = conn.prepare_cached("SELECT decision_id FROM decisions WHERE repo_id = ?1")?;
    let ids = stmt.query_map([repo_id], |r| r.get::<_, String>(0))?;
    let mut max = 0;
    for id in ids {
        max = max.max(number(&id?));
    }
    Ok(format!("D{}", max + 1))
}

/// The live (not superseded) decision on `task_id` with the same `what_key`.
fn duplicate(
    conn: &Connection,
    repo_id: &str,
    task_id: Option<&str>,
    key: &str,
) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached(
            "SELECT decision_id FROM decisions
             WHERE repo_id = ?1 AND task_id IS ?2 AND what_key = ?3 AND superseded_by IS NULL
             LIMIT 1",
        )?
        .query_row(params![repo_id, task_id, key], |r| r.get(0))
        .optional()?)
}

/// Record a decision (spec §8.6, §11.2). Over-long text is cut, never refused. A
/// `supersedes` must name a live decision of this repo other than the one being logged;
/// the same `what` on the same task merges into the existing decision. Ids are given out
/// inside the write transaction, so concurrent writers never share one.
pub fn log_decision(store: &mut crate::store::Store, req: &LogRequest) -> Result<Logged> {
    let what = clip(&req.what, MAX_WHAT);
    let why = clip(&req.why, MAX_WHY);
    if what.is_empty() || why.is_empty() {
        return Err(refuse("what and why are both required"));
    }
    let mut alternatives: Vec<String> = Vec::new();
    for a in &req.alternatives {
        let a = clip(a, MAX_WHAT);
        if !a.is_empty() && !alternatives.contains(&a) {
            alternatives.push(a);
        }
    }
    alternatives.truncate(MAX_ALTERNATIVES);
    store.write(|tx| {
        let s = crate::session::get(tx, &req.session_id)?
            .ok_or_else(|| refuse(format!("unknown session {}", req.session_id)))?;
        let task = match &req.task {
            DecisionTask::Active => s.active_task.clone(),
            DecisionTask::RepoWide => None,
            DecisionTask::Task(t) => Some(t.trim().to_string()).filter(|t| !t.is_empty()),
        };
        if let Some(t) = &task
            && crate::plan::get_node(tx, &s.repo_id, t)?.is_none()
        {
            return Err(refuse(format!("no task {t} in this plan")));
        }
        let dup = duplicate(tx, &s.repo_id, task.as_deref(), &what_key(&what))?;
        let id = match &dup {
            Some(id) => id.clone(),
            None => next_id(tx, &s.repo_id)?,
        };
        let supersedes = req
            .supersedes
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        if let Some(old) = &supersedes {
            if *old == id {
                return Err(refuse(format!("{old} can't supersede itself")));
            }
            let found = get(tx, &s.repo_id, old)?
                .ok_or_else(|| refuse(format!("no decision {old} in this repo")))?;
            if let Some(newer) = found.superseded_by {
                return Err(refuse(format!(
                    "{old} was already superseded by {newer} — supersede {newer} instead"
                )));
            }
        }
        let data = DecisionData {
            decision_id: id.clone(),
            task_id: task.clone(),
            what: what.clone(),
            why: why.clone(),
            alternatives: alternatives.clone(),
            supersedes: supersedes.clone(),
        };
        let mut draft = crate::store::EventDraft::new(
            &s.repo_id,
            &s.session_id,
            "decision",
            serde_json::to_value(data)?,
        )
        .actor(req.actor.clone());
        if let Some(t) = &task {
            draft = draft.task(t);
        }
        crate::store::append_in(tx, draft)?;
        Ok(Logged {
            decision_id: id,
            task_id: task,
            merged: dup.is_some(),
            supersedes,
        })
    })
}

/// `decision_asked`: the capture request went out (clears the session's flag).
pub fn asked_draft(repo_id: &str, session_id: &str, via: &str) -> crate::store::EventDraft {
    crate::store::EventDraft::new(
        repo_id,
        session_id,
        "decision_asked",
        serde_json::json!({ "via": via }),
    )
}

/// `D7 2.3 Use SQLite — one file (alternatives: Postgres) [supersedes D4]`.
pub fn line(d: &Decision) -> String {
    let mut text = d.decision_id.clone();
    if let Some(t) = &d.task_id {
        text.push(' ');
        text.push_str(t);
    }
    let _ = write!(text, " {} — {}", d.what, d.why);
    if !d.alternatives.is_empty() {
        let _ = write!(text, " (alternatives: {})", d.alternatives.join(", "));
    }
    if let Some(old) = &d.supersedes {
        let _ = write!(text, " [supersedes {old}]");
    }
    if let Some(newer) = &d.superseded_by {
        let _ = write!(text, " [superseded by {newer}]");
    }
    text
}

#[cfg(test)]
mod tests;
