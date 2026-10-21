//! Sessions, units of work and agent-active time (spec §5.4–5.5).
//!
//! A **unit** runs from a `prompt` to its `unit_stopped`. Its active time is wall-clock time
//! minus time spent waiting on the owner (`needs_owner`: a permission prompt, Claude idle
//! at the prompt, an MCP form). All durations come from event
//! timestamps, never from "now", so `rebuild` reproduces them exactly.

pub mod project;

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::error::Result;
use crate::handoff::{BriefingStatus, EndKind, ReadbackState};
use crate::tangent::{Label, LabelSource, NavState};

pub use project::{
    BranchChanged, FileEvent, NeedsOwner, NeedsOwnerKind, OwnerQuestion, Prompt, QuestionSource,
    SESSION_KINDS, SessionCrashed, SessionEnded, SessionStarted, SubagentStopped, TaskClaimed,
    TaskReleased, UnitStopped, project,
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
    /// The detour the session is focused on, if any (spec §7.4).
    pub current_detour: Option<String>,
    /// Consecutive unit time not on-task (spec §7.3), from labelled units.
    pub away_secs: i64,
    pub away_since: Option<String>,
    pub nav_state: NavState,
    /// Away time the owner already said "continue" to.
    pub ack_away_secs: i64,
    /// The last unit when the owner said "continue": drift flags up to it are acknowledged.
    pub ack_unit: i64,
    /// An owner answer waiting to be delivered: `<answer>:<owner_action event id>`.
    pub pending_reply: Option<String>,
    /// The worktree's root directory (spec §8.3).
    pub worktree: Option<String>,
    /// How the session ended, once it has.
    pub end_kind: Option<EndKind>,
    /// SHA of the last checkpoint commit.
    pub last_checkpoint: Option<String>,
    /// Why a checkpoint is due (`claimed_done`, `detour_closed`, `task_switched`).
    pub checkpoint_pending: Option<String>,
    /// The unit in which the agent was asked to checkpoint.
    pub checkpoint_asked_unit: Option<i64>,
    /// The unit in which a requested checkpoint was missed; 0 when none is.
    pub checkpoint_missed: i64,
    pub readback: ReadbackState,
    pub briefing_status: Option<BriefingStatus>,
    /// The task the session last claimed; kept after it ends, for recovery.
    pub last_task: Option<String>,
    /// A later session has analysed this (dead) session's leftovers.
    pub leftovers_analysed: bool,
    /// Decisions should be asked for with the next note: `precompact` or `claimed_done`
    /// (spec §8.6); cleared by a `decision` or `decision_asked`.
    pub decision_capture: Option<String>,
}

/// One row of the `units` view.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    pub label: Option<Label>,
    pub label_source: Option<LabelSource>,
    /// The drift heuristic's score for the unit (0..=1), if it was scored.
    pub drift_score: Option<f64>,
    /// The detour open during the unit; its time is charged there too.
    pub detour_id: Option<String>,
    /// The agent's or owner's label differs from the heuristic's.
    pub disagreement: bool,
    /// The heuristic flagged the unit as possible drift.
    pub flagged: bool,
}

const SESSION_COLUMNS: &str = "session_id, repo_id, cwd, branch, source, model, transcript_path,
     started_at, last_event_at, state, active_task, active_since, unit_count, active_secs,
     ended_at, end_reason, current_detour, away_secs, away_since, nav_state, ack_away_secs,
     ack_unit, pending_reply, worktree, end_kind, last_checkpoint, checkpoint_pending,
     checkpoint_asked_unit, checkpoint_missed, readback, briefing_status, last_task,
     leftovers_analysed, decision_capture";

const UNIT_COLUMNS: &str = "session_id, unit_no, prompt_id, task_id, started_at, stopped_at,
     waited_secs, wait_started_at, active_secs, files, label, label_source, drift_score,
     detour_id, disagreement, flagged";

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
        current_detour: r.get(16)?,
        away_secs: r.get(17)?,
        away_since: r.get(18)?,
        nav_state: NavState::parse(&r.get::<_, String>(19)?).unwrap_or(NavState::Green),
        ack_away_secs: r.get(20)?,
        ack_unit: r.get(21)?,
        pending_reply: r.get(22)?,
        worktree: r.get(23)?,
        end_kind: r
            .get::<_, Option<String>>(24)?
            .as_deref()
            .and_then(EndKind::parse),
        last_checkpoint: r.get(25)?,
        checkpoint_pending: r.get(26)?,
        checkpoint_asked_unit: r.get(27)?,
        checkpoint_missed: r.get(28)?,
        readback: ReadbackState::parse(&r.get::<_, String>(29)?)
            .unwrap_or(ReadbackState::NotNeeded),
        briefing_status: r
            .get::<_, Option<String>>(30)?
            .as_deref()
            .and_then(BriefingStatus::parse),
        last_task: r.get(31)?,
        leftovers_analysed: r.get(32)?,
        decision_capture: r.get(33)?,
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
            label: r
                .get::<_, Option<String>>(10)?
                .as_deref()
                .and_then(Label::parse),
            label_source: r
                .get::<_, Option<String>>(11)?
                .as_deref()
                .and_then(LabelSource::parse),
            drift_score: r.get(12)?,
            detour_id: r.get(13)?,
            disagreement: r.get(14)?,
            flagged: r.get(15)?,
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

/// One unit of a session.
pub fn unit(conn: &Connection, session_id: &str, unit_no: i64) -> Result<Option<Unit>> {
    conn.prepare_cached(&format!(
        "SELECT {UNIT_COLUMNS} FROM units WHERE session_id = ?1 AND unit_no = ?2"
    ))?
    .query_row(params![session_id, unit_no], read_unit)
    .optional()?
    .map(with_files)
    .transpose()
}

/// The session's most recent unit, open or stopped.
pub fn last_unit(conn: &Connection, session_id: &str) -> Result<Option<Unit>> {
    conn.prepare_cached(&format!(
        "SELECT {UNIT_COLUMNS} FROM units WHERE session_id = ?1 ORDER BY unit_no DESC LIMIT 1"
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
