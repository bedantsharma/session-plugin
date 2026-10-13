//! Session event payloads and the projector that keeps `sessions`, `units` and `task_time`
//! up to date.

use jiff::Timestamp;
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use super::{Session, SessionState, Unit, current_unit, get};
use crate::error::{Error, Result};
use crate::plan::project::{mark_claimed, mark_released};
use crate::store::Event;

/// Event kinds routed to this projector.
pub const SESSION_KINDS: &[&str] = &[
    "session_started",
    "prompt",
    "tool_intent",
    "file_changed",
    "needs_owner",
    "unit_stopped",
    "subagent_stopped",
    "precompact",
    "session_ended",
    "task_claimed",
    "task_released",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStarted {
    pub cwd: String,
    #[serde(default)]
    pub branch: Option<String>,
    /// startup, resume, clear, compact, fork — or `unknown` for a session first seen
    /// mid-way (started before bridge-screen was installed).
    #[serde(default = "unknown")]
    pub source: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
}

fn unknown() -> String {
    "unknown".to_string()
}

/// A user prompt. Only its length is kept, never its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    #[serde(default)]
    pub prompt_id: Option<String>,
    #[serde(default)]
    pub chars: u64,
}

/// `tool_intent` (before an edit tool runs) and `file_changed` (after it succeeded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEvent {
    pub tool: String,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedsOwnerKind {
    PermissionPrompt,
    IdlePrompt,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsOwner {
    pub kind: NeedsOwnerKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitStopped {
    #[serde(default)]
    pub unit_no: Option<i64>,
    /// First 300 characters of the agent's last message.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentStopped {
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEnded {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskClaimed {
    pub task_id: String,
    /// `auto` or `confirmed`.
    pub mode: String,
    pub confidence: f64,
    #[serde(default)]
    pub previous: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReleased {
    pub task_id: String,
    pub reason: String,
}

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

fn parse_ts(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

/// Whole seconds from `from` to `to`; 0 if either can't be read.
fn secs_between(from: &str, to: Timestamp) -> i64 {
    parse_ts(from).map_or(0, |f| to.duration_since(f).as_secs())
}

/// Apply one session event to the session views.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    let ts: Timestamp = event
        .ts
        .parse()
        .map_err(|e| Error::BadEvent(format!("timestamp {}: {e}", event.ts)))?;
    if event.kind == "session_started" {
        let d: SessionStarted = payload(event)?;
        if d.source != COMPACT
            && let Some(before) = get(tx, &event.session_id)?
            && let Some(open) = current_unit(tx, &event.session_id)?
        {
            close_crashed_unit(tx, &before, open, ts)?;
        }
        upsert_session(tx, event, &d)?;
    }
    // Events for a session bridge-screen never saw start change nothing.
    let Some(session) = get(tx, &event.session_id)? else {
        return Ok(());
    };
    // Late hooks after SessionEnd don't bring an ended session back; a new start or a new
    // prompt does.
    if session.state == SessionState::Ended
        && !matches!(event.kind.as_str(), "session_started" | "prompt")
    {
        return Ok(());
    }
    let mut unit = current_unit(tx, &session.session_id)?;
    if let Some(u) = unit.as_mut()
        && event.kind != "needs_owner"
        && let Some(since) = u.wait_started_at.take()
    {
        u.waited_secs += secs_between(&since, ts).max(0);
        save_unit(tx, u)?;
    }
    tx.execute(
        "UPDATE sessions SET last_event_at = ?2 WHERE session_id = ?1",
        params![session.session_id, event.ts],
    )?;
    let at = Moment {
        tx,
        ts,
        ts_text: &event.ts,
        session: &session,
    };
    match event.kind.as_str() {
        "prompt" => at.prompt(unit, &payload(event)?)?,
        "needs_owner" => {
            payload::<NeedsOwner>(event)?;
            at.needs_owner(unit)?;
        }
        "tool_intent" => at.tool_activity(unit.as_ref())?,
        "file_changed" => {
            at.tool_activity(unit.as_ref())?;
            if let Some(mut u) = unit {
                let d: FileEvent = payload(event)?;
                for f in d.files {
                    if !u.files.contains(&f) {
                        u.files.push(f);
                    }
                }
                save_unit(tx, &u)?;
            }
        }
        "unit_stopped" => {
            if let Some(u) = unit {
                at.close_unit(u)?;
            }
            at.set_state(SessionState::Idle)?;
        }
        "session_ended" => {
            let d: SessionEnded = payload(event)?;
            if let Some(u) = unit {
                at.close_unit(u)?;
            }
            tx.execute(
                "UPDATE sessions SET state = 'ended', ended_at = ?2, end_reason = ?3
                 WHERE session_id = ?1",
                params![session.session_id, event.ts, d.reason],
            )?;
            // An ended session lets go of its task.
            if let Some(task) = &session.active_task {
                at.release(task)?;
            }
        }
        "task_claimed" => {
            let d: TaskClaimed = payload(event)?;
            tx.execute(
                "UPDATE sessions SET active_task = ?2, active_since = ?3 WHERE session_id = ?1",
                params![session.session_id, d.task_id, event.ts],
            )?;
            if let Some(mut u) = unit.filter(|u| u.task_id.is_none()) {
                u.task_id = Some(d.task_id.clone());
                save_unit(tx, &u)?;
            }
            mark_claimed(tx, &session.repo_id, &d.task_id)?;
        }
        "task_released" => {
            let d: TaskReleased = payload(event)?;
            at.release(&d.task_id)?;
        }
        // subagent_stopped and precompact only mark activity.
        _ => {}
    }
    Ok(())
}

/// `SessionStart` source for a compaction inside a running session: not a new start.
const COMPACT: &str = "compact";

/// A session that starts again (resume) while a unit is still open crashed or was killed
/// mid-unit. The unit ends at the session's last recorded event, not at the restart, so the
/// time it lay dead doesn't count; a wait still open then ends there too.
fn close_crashed_unit(
    tx: &Transaction,
    before: &Session,
    mut u: Unit,
    ts: Timestamp,
) -> Result<()> {
    let last = parse_ts(&before.last_event_at).unwrap_or(ts).min(ts);
    let end = parse_ts(&u.started_at).map_or(last, |start| last.max(start).min(ts));
    if let Some(since) = u.wait_started_at.take() {
        u.waited_secs += secs_between(&since, end).max(0);
    }
    let end_text = end.to_string();
    Moment {
        tx,
        ts: end,
        ts_text: &end_text,
        session: before,
    }
    .close_unit(u)
}

fn upsert_session(tx: &Transaction, event: &Event, d: &SessionStarted) -> Result<()> {
    // A resumed session keeps its counters and first start time. A compaction happens
    // inside a running session, so it changes neither its state nor how it started.
    tx.execute(
        "INSERT INTO sessions (session_id, repo_id, cwd, branch, source, model, transcript_path,
                               started_at, last_event_at, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 'idle')
         ON CONFLICT(session_id) DO UPDATE SET
             cwd = excluded.cwd,
             branch = COALESCE(excluded.branch, branch),
             model = COALESCE(excluded.model, model),
             transcript_path = COALESCE(excluded.transcript_path, transcript_path),
             source = CASE WHEN excluded.source = 'compact' THEN source ELSE excluded.source END,
             state = CASE WHEN excluded.source = 'compact' THEN state ELSE 'idle' END,
             ended_at = CASE WHEN excluded.source = 'compact' THEN ended_at ELSE NULL END,
             end_reason = CASE WHEN excluded.source = 'compact' THEN end_reason ELSE NULL END",
        params![
            event.session_id,
            event.repo_id,
            d.cwd,
            d.branch,
            d.source,
            d.model,
            d.transcript_path,
            event.ts
        ],
    )?;
    Ok(())
}

fn save_unit(tx: &Transaction, u: &Unit) -> Result<()> {
    tx.execute(
        "UPDATE units SET task_id = ?3, stopped_at = ?4, waited_secs = ?5, wait_started_at = ?6,
                          active_secs = ?7, files = ?8
         WHERE session_id = ?1 AND unit_no = ?2",
        params![
            u.session_id,
            u.unit_no,
            u.task_id,
            u.stopped_at,
            u.waited_secs,
            u.wait_started_at,
            u.active_secs,
            serde_json::to_string(&u.files)?
        ],
    )?;
    Ok(())
}

/// One event being applied to one session.
struct Moment<'a> {
    tx: &'a Transaction<'a>,
    ts: Timestamp,
    ts_text: &'a str,
    session: &'a Session,
}

impl Moment<'_> {
    fn set_state(&self, state: SessionState) -> Result<()> {
        self.tx.execute(
            "UPDATE sessions SET state = ?2 WHERE session_id = ?1",
            params![self.session.session_id, state.as_str()],
        )?;
        Ok(())
    }

    fn prompt(&self, open: Option<Unit>, d: &Prompt) -> Result<()> {
        // A unit still open here missed its Stop: it ends where the next one begins.
        if let Some(u) = open {
            self.close_unit(u)?;
        }
        // A prompt after SessionEnd means the session is alive after all.
        self.tx.execute(
            "UPDATE sessions SET ended_at = NULL, end_reason = NULL WHERE session_id = ?1",
            [&self.session.session_id],
        )?;
        let unit_count: i64 = self.tx.query_row(
            "SELECT unit_count FROM sessions WHERE session_id = ?1",
            [&self.session.session_id],
            |r| r.get(0),
        )?;
        self.tx.execute(
            "INSERT INTO units (session_id, unit_no, prompt_id, task_id, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                self.session.session_id,
                unit_count + 1,
                d.prompt_id,
                self.session.active_task,
                self.ts_text
            ],
        )?;
        self.set_state(SessionState::Working)
    }

    /// Every kind is a wait on the owner: a permission prompt, Claude idle at the prompt
    /// (after an interrupt, which sends no Stop), or an MCP form. Inside a unit, the wait
    /// lasts until the next activity.
    fn needs_owner(&self, open: Option<Unit>) -> Result<()> {
        self.set_state(SessionState::NeedsOwner)?;
        if let Some(mut u) = open
            && u.wait_started_at.is_none()
        {
            u.wait_started_at = Some(self.ts_text.to_string());
            save_unit(self.tx, &u)?;
        }
        Ok(())
    }

    /// Tool activity inside a unit means the agent is working again.
    fn tool_activity(&self, open: Option<&Unit>) -> Result<()> {
        if open.is_some() && self.session.state == SessionState::NeedsOwner {
            self.set_state(SessionState::Working)?;
        }
        Ok(())
    }

    /// The session stops holding `task`; the task goes back to todo if nobody else has it.
    fn release(&self, task: &str) -> Result<()> {
        self.tx.execute(
            "UPDATE sessions SET active_task = NULL, active_since = NULL
             WHERE session_id = ?1 AND active_task = ?2",
            params![self.session.session_id, task],
        )?;
        mark_released(self.tx, &self.session.repo_id, task)
    }

    fn close_unit(&self, mut u: Unit) -> Result<()> {
        let wall = secs_between(&u.started_at, self.ts);
        u.active_secs = (wall - u.waited_secs).max(0);
        u.stopped_at = Some(self.ts_text.to_string());
        u.wait_started_at = None;
        save_unit(self.tx, &u)?;
        self.tx.execute(
            "UPDATE sessions SET active_secs = active_secs + ?2, unit_count = unit_count + 1
             WHERE session_id = ?1",
            params![self.session.session_id, u.active_secs],
        )?;
        if let Some(task) = &u.task_id {
            self.tx.execute(
                "INSERT INTO task_time (repo_id, task_id, active_secs, units) VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT(repo_id, task_id) DO UPDATE SET
                     active_secs = active_secs + excluded.active_secs, units = units + 1",
                params![self.session.repo_id, task, u.active_secs],
            )?;
        }
        Ok(())
    }
}
