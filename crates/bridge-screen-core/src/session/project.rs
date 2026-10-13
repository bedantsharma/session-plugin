//! Session event payloads and the projector that keeps `sessions`, `units` and `task_time`
//! up to date.

use jiff::Timestamp;
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use super::{Session, SessionState, Unit, current_unit, get};
use crate::error::{Error, Result};
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
    pub confidence: f32,
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
        upsert_session(tx, event, &payload(event)?)?;
    }
    // Events for a session bridge-screen never saw start change nothing.
    let Some(session) = get(tx, &event.session_id)? else {
        return Ok(());
    };
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
        "needs_owner" => at.needs_owner(unit, &payload::<NeedsOwner>(event)?)?,
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
        }
        // subagent_stopped and precompact only mark activity; claims arrive with claiming.
        _ => {}
    }
    Ok(())
}

fn upsert_session(tx: &Transaction, event: &Event, d: &SessionStarted) -> Result<()> {
    // A resumed session keeps its counters and first start time.
    tx.execute(
        "INSERT INTO sessions (session_id, repo_id, cwd, branch, source, model, transcript_path,
                               started_at, last_event_at, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 'idle')
         ON CONFLICT(session_id) DO UPDATE SET
             cwd = excluded.cwd,
             branch = COALESCE(excluded.branch, branch),
             source = excluded.source,
             model = COALESCE(excluded.model, model),
             transcript_path = COALESCE(excluded.transcript_path, transcript_path),
             state = 'idle',
             ended_at = NULL,
             end_reason = NULL",
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

    fn needs_owner(&self, open: Option<Unit>, d: &NeedsOwner) -> Result<()> {
        self.set_state(SessionState::NeedsOwner)?;
        if d.kind == NeedsOwnerKind::PermissionPrompt
            && let Some(mut u) = open
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
