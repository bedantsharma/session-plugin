//! Sessions, units of work and agent-active time (spec §5.4–5.5).
//!
//! A **unit** runs from a `prompt` to its `unit_stopped`. Its active time is wall-clock time
//! minus time spent waiting on a permission prompt. All durations come from event
//! timestamps, never from "now", so `rebuild` reproduces them exactly.

pub mod project;

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::error::Result;

pub use project::{
    FileEvent, NeedsOwner, NeedsOwnerKind, Prompt, SESSION_KINDS, SessionEnded, SessionStarted,
    SubagentStopped, TaskClaimed, TaskReleased, UnitStopped, project,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Working,
    Idle,
    NeedsOwner,
    Ended,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::NeedsOwner => "needs_owner",
            Self::Ended => "ended",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "working" => Some(Self::Working),
            "idle" => Some(Self::Idle),
            "needs_owner" => Some(Self::NeedsOwner),
            "ended" => Some(Self::Ended),
            _ => None,
        }
    }
}

/// One row of the `sessions` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub session_id: String,
    pub repo_id: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub source: String,
    pub model: Option<String>,
    pub transcript_path: Option<String>,
    pub started_at: String,
    pub last_event_at: String,
    pub state: SessionState,
    pub active_task: Option<String>,
    pub active_since: Option<String>,
    pub unit_count: i64,
    pub active_secs: i64,
    pub ended_at: Option<String>,
    pub end_reason: Option<String>,
}

/// One row of the `units` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unit {
    pub session_id: String,
    pub unit_no: i64,
    pub prompt_id: Option<String>,
    pub task_id: Option<String>,
    pub started_at: String,
    pub stopped_at: Option<String>,
    pub waited_secs: i64,
    pub wait_started_at: Option<String>,
    pub active_secs: i64,
    pub files: Vec<String>,
}

const SESSION_COLUMNS: &str = "session_id, repo_id, cwd, branch, source, model, transcript_path,
     started_at, last_event_at, state, active_task, active_since, unit_count, active_secs,
     ended_at, end_reason";

const UNIT_COLUMNS: &str = "session_id, unit_no, prompt_id, task_id, started_at, stopped_at,
     waited_secs, wait_started_at, active_secs, files";

fn read_session(r: &Row) -> rusqlite::Result<Session> {
    let state: String = r.get(9)?;
    Ok(Session {
        session_id: r.get(0)?,
        repo_id: r.get(1)?,
        cwd: r.get(2)?,
        branch: r.get(3)?,
        source: r.get(4)?,
        model: r.get(5)?,
        transcript_path: r.get(6)?,
        started_at: r.get(7)?,
        last_event_at: r.get(8)?,
        state: SessionState::parse(&state).unwrap_or(SessionState::Idle),
        active_task: r.get(10)?,
        active_since: r.get(11)?,
        unit_count: r.get(12)?,
        active_secs: r.get(13)?,
        ended_at: r.get(14)?,
        end_reason: r.get(15)?,
    })
}

fn read_unit(r: &Row) -> rusqlite::Result<(Unit, String)> {
    Ok((
        Unit {
            session_id: r.get(0)?,
            unit_no: r.get(1)?,
            prompt_id: r.get(2)?,
            task_id: r.get(3)?,
            started_at: r.get(4)?,
            stopped_at: r.get(5)?,
            waited_secs: r.get(6)?,
            wait_started_at: r.get(7)?,
            active_secs: r.get(8)?,
            files: Vec::new(),
        },
        r.get(9)?,
    ))
}

fn with_files((mut unit, files): (Unit, String)) -> Result<Unit> {
    unit.files = serde_json::from_str(&files)?;
    Ok(unit)
}

/// The session, if bridge-screen has seen it.
pub fn get(conn: &Connection, session_id: &str) -> Result<Option<Session>> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE session_id = ?1"
        ))?
        .query_row([session_id], read_session)
        .optional()?)
}

/// The session's open unit (started, not yet stopped), if any.
pub fn current_unit(conn: &Connection, session_id: &str) -> Result<Option<Unit>> {
    conn.prepare_cached(&format!(
        "SELECT {UNIT_COLUMNS} FROM units WHERE session_id = ?1 AND stopped_at IS NULL
         ORDER BY unit_no DESC LIMIT 1"
    ))?
    .query_row([session_id], read_unit)
    .optional()?
    .map(with_files)
    .transpose()
}

/// All of a session's units, in order.
pub fn units(conn: &Connection, session_id: &str) -> Result<Vec<Unit>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {UNIT_COLUMNS} FROM units WHERE session_id = ?1 ORDER BY unit_no"
    ))?;
    let rows = stmt.query_map([session_id], read_unit)?;
    rows.map(|r| with_files(r?)).collect()
}

/// How far back [`open_unit_intents`] looks for the unit's prompt.
const INTENT_SCAN_LIMIT: i64 = 2000;

/// Files that already have a `tool_intent` in the session's current unit (since its last
/// `prompt`). Reads the event log backwards, so it stays cheap on large databases.
pub fn open_unit_intents(conn: &Connection, session_id: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT kind, data FROM events WHERE session_id = ?1 ORDER BY seq DESC LIMIT ?2",
    )?;
    let mut rows = stmt.query(params![session_id, INTENT_SCAN_LIMIT])?;
    let mut files = HashSet::new();
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        match kind.as_str() {
            "prompt" => break,
            "tool_intent" => {
                let data: String = row.get(1)?;
                let d: FileEvent = serde_json::from_str(&data)?;
                files.extend(d.files);
            }
            _ => {}
        }
    }
    Ok(files)
}

/// Sessions that haven't ended (optionally in one repo), newest first.
pub fn list_live(conn: &Connection, repo_id: Option<&str>) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions
         WHERE state != 'ended' AND (?1 IS NULL OR repo_id = ?1)"
    ))?;
    let mut sessions = stmt
        .query_map(params![repo_id], read_session)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Timestamps are compared as instants: their text form varies in fraction digits.
    sessions.sort_by_cached_key(|s| {
        std::cmp::Reverse((
            s.started_at.parse::<jiff::Timestamp>().ok(),
            s.session_id.clone(),
        ))
    });
    Ok(sessions)
}

/// Active seconds and unit count recorded against a task, if any.
pub fn task_time(conn: &Connection, repo_id: &str, task_id: &str) -> Result<Option<(i64, i64)>> {
    Ok(conn
        .query_row(
            "SELECT active_secs, units FROM task_time WHERE repo_id = ?1 AND task_id = ?2",
            params![repo_id, task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

#[cfg(test)]
mod tests;
