//! Handoff (spec §8): checkpoints, how sessions ended, leftovers of dead sessions,
//! handoff notes, the briefing and its read-back.
//!
//! This module holds the model: event payloads, the projector for the handoff views and
//! read-side queries. The logic lives in `checkpoint`, `recovery` and the briefing.

pub mod briefing;
pub mod project;

use rusqlite::{Connection, OptionalExtension, Row, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;

pub use briefing::{
    Briefing, READBACK_ASK, READBACK_NUDGE, ReadbackCheck, briefing, check_readback,
    compact_rebriefing,
};
pub use project::{
    Briefed, CheckpointDue, CheckpointMissed, CheckpointReason, CheckpointRecorded, ClaimedDone,
    HANDOFF_KINDS, HandoffNoteData, LeftoverFound, LeftoverResolved, LeftoversAnalysed,
    ReadbackData, SquashFailed, Squashed, project,
};

macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($text => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

text_enum!(
    /// How a session ended (spec §8.2).
    EndKind { Clean = "clean", Dirty = "dirty", Crashed = "crashed" }
);

text_enum!(
    /// Where the session's read-back of its briefing stands (spec §8.4).
    ReadbackState {
        NotNeeded = "not_needed",
        Pending = "pending",
        Ok = "ok",
        Mismatch = "mismatch",
    }
);

text_enum!(
    /// I-PASS severity of a briefing.
    BriefingStatus { Stable = "stable", Watcher = "watcher", Unstable = "unstable" }
);

text_enum!(
    /// What to do with a leftover (spec §8.3).
    LeftoverAction { Adopt = "adopt", Resume = "resume", Undo = "undo" }
);

text_enum!(
    LeftoverState { Open = "open", ReportOnly = "report_only", Resolved = "resolved" }
);

text_enum!(
    ResolvedBy { Agent = "agent", Owner = "owner" }
);

/// One row of `checkpoints`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Checkpoint {
    pub sha: String,
    pub session_id: String,
    pub repo_id: String,
    pub task_id: Option<String>,
    pub next_action: Option<String>,
    pub ts: String,
}

/// One row of `leftovers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Leftover {
    pub leftover_id: String,
    pub repo_id: String,
    pub from_session: String,
    pub worktree: String,
    pub path: String,
    pub task_id: Option<String>,
    pub suggested: LeftoverAction,
    pub state: LeftoverState,
    pub resolution: Option<LeftoverAction>,
    pub stash_ref: Option<String>,
    pub commit_sha: Option<String>,
    pub resolved_by: Option<ResolvedBy>,
    pub ts: String,
}

/// One row of `handoff_notes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HandoffNote {
    pub session_id: String,
    pub task_id: Option<String>,
    pub in_flight: String,
    pub if_then: Vec<String>,
    pub ts: String,
}

/// One row of `claims_done` (the L0 record).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaimDone {
    pub task_id: String,
    pub session_id: String,
    pub evidence: String,
    pub ts: String,
}

fn read_checkpoint(r: &Row) -> rusqlite::Result<Checkpoint> {
    Ok(Checkpoint {
        sha: r.get(0)?,
        session_id: r.get(1)?,
        repo_id: r.get(2)?,
        task_id: r.get(3)?,
        next_action: r.get(4)?,
        ts: r.get(5)?,
    })
}

const LEFTOVER_COLUMNS: &str = "leftover_id, repo_id, from_session, worktree, path, task_id,
     suggested, state, resolution, stash_ref, commit_sha, resolved_by, ts";

fn read_leftover(r: &Row) -> rusqlite::Result<Leftover> {
    let suggested: String = r.get(6)?;
    let state: String = r.get(7)?;
    Ok(Leftover {
        leftover_id: r.get(0)?,
        repo_id: r.get(1)?,
        from_session: r.get(2)?,
        worktree: r.get(3)?,
        path: r.get(4)?,
        task_id: r.get(5)?,
        suggested: LeftoverAction::parse(&suggested).unwrap_or(LeftoverAction::Resume),
        state: LeftoverState::parse(&state).unwrap_or(LeftoverState::Open),
        resolution: r
            .get::<_, Option<String>>(8)?
            .as_deref()
            .and_then(LeftoverAction::parse),
        stash_ref: r.get(9)?,
        commit_sha: r.get(10)?,
        resolved_by: r
            .get::<_, Option<String>>(11)?
            .as_deref()
            .and_then(ResolvedBy::parse),
        ts: r.get(12)?,
    })
}

/// The session's most recent checkpoint.
pub fn latest_checkpoint(conn: &Connection, session_id: &str) -> Result<Option<Checkpoint>> {
    Ok(conn
        .prepare_cached(
            "SELECT sha, session_id, repo_id, task_id, next_action, ts FROM checkpoints
             WHERE session_id = ?1 ORDER BY ts DESC, rowid DESC LIMIT 1",
        )?
        .query_row([session_id], read_checkpoint)
        .optional()?)
}

/// The latest checkpoint recorded against `task_id` by any session.
pub fn latest_checkpoint_for_task(
    conn: &Connection,
    repo_id: &str,
    task_id: &str,
) -> Result<Option<Checkpoint>> {
    Ok(conn
        .prepare_cached(
            "SELECT sha, session_id, repo_id, task_id, next_action, ts FROM checkpoints
             WHERE repo_id = ?1 AND task_id = ?2 ORDER BY ts DESC, rowid DESC LIMIT 1",
        )?
        .query_row(params![repo_id, task_id], read_checkpoint)
        .optional()?)
}

/// A repo's leftovers in the order they were found; `unresolved` keeps open and
/// report-only ones.
pub fn leftovers(conn: &Connection, repo_id: &str, unresolved: bool) -> Result<Vec<Leftover>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {LEFTOVER_COLUMNS} FROM leftovers
         WHERE repo_id = ?1 AND (?2 = 0 OR state != 'resolved') ORDER BY ts, rowid"
    ))?;
    Ok(stmt
        .query_map(params![repo_id, unresolved], read_leftover)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn get_leftover(conn: &Connection, leftover_id: &str) -> Result<Option<Leftover>> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {LEFTOVER_COLUMNS} FROM leftovers WHERE leftover_id = ?1"
        ))?
        .query_row([leftover_id], read_leftover)
        .optional()?)
}

/// The latest handoff note on `task_id` (or on no task) in a repo.
pub fn latest_note(
    conn: &Connection,
    repo_id: &str,
    task_id: Option<&str>,
) -> Result<Option<HandoffNote>> {
    let row = conn
        .prepare_cached(
            "SELECT session_id, task_id, in_flight, if_then, ts FROM handoff_notes
             WHERE repo_id = ?1 AND task_id IS ?2 ORDER BY ts DESC, rowid DESC LIMIT 1",
        )?
        .query_row(params![repo_id, task_id], |r| {
            Ok((
                HandoffNote {
                    session_id: r.get(0)?,
                    task_id: r.get(1)?,
                    in_flight: r.get(2)?,
                    if_then: Vec::new(),
                    ts: r.get(4)?,
                },
                r.get::<_, String>(3)?,
            ))
        })
        .optional()?;
    row.map(|(mut note, if_then)| {
        note.if_then = serde_json::from_str(&if_then)?;
        Ok(note)
    })
    .transpose()
}

/// Every claimed-done record in a repo, oldest first.
pub fn claims_done(conn: &Connection, repo_id: &str) -> Result<Vec<ClaimDone>> {
    let mut stmt = conn.prepare(
        "SELECT task_id, session_id, evidence, ts FROM claims_done WHERE repo_id = ?1
         ORDER BY ts, rowid",
    )?;
    Ok(stmt
        .query_map([repo_id], |r| {
            Ok(ClaimDone {
                task_id: r.get(0)?,
                session_id: r.get(1)?,
                evidence: r.get(2)?,
                ts: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn err(msg: impl Into<String>) -> crate::error::Error {
    crate::error::Error::Tangent(msg.into())
}

/// Longest `in_flight` text and if/then line kept in a handoff note.
pub const MAX_NOTE_CHARS: usize = 300;
/// Most if/then lines in one note.
pub const MAX_IF_THEN: usize = 5;

/// Record a handoff note for the session's active task (spec §8.4). Returns the task.
pub fn leave_note(
    store: &mut crate::store::Store,
    session_id: &str,
    in_flight: &str,
    if_then: &[String],
    actor: &crate::store::Actor,
) -> Result<Option<String>> {
    let in_flight = in_flight.trim();
    if in_flight.is_empty() || in_flight.chars().count() > MAX_NOTE_CHARS {
        return Err(err(format!(
            "in_flight is required: what is half-done, at most {MAX_NOTE_CHARS} characters"
        )));
    }
    let lines: Vec<String> = if_then
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() > MAX_IF_THEN || lines.iter().any(|l| l.chars().count() > MAX_NOTE_CHARS) {
        return Err(err(format!(
            "if_then takes at most {MAX_IF_THEN} lines of at most {MAX_NOTE_CHARS} characters"
        )));
    }
    store.write(|tx| {
        let s = crate::session::get(tx, session_id)?
            .ok_or_else(|| err(format!("unknown session {session_id}")))?;
        let data = HandoffNoteData {
            task_id: s.active_task.clone(),
            in_flight: in_flight.to_string(),
            if_then: lines.clone(),
        };
        let mut draft = crate::store::EventDraft::new(
            &s.repo_id,
            &s.session_id,
            "handoff_note",
            serde_json::to_value(data)?,
        )
        .actor(actor.clone());
        if let Some(t) = &s.active_task {
            draft = draft.task(t);
        }
        crate::store::append_in(tx, draft)?;
        Ok(s.active_task)
    })
}

/// Check a read-back and record it (`readback`). Returns the check, whose `reply` is
/// the tool result.
pub fn read_back(
    store: &mut crate::store::Store,
    session_id: &str,
    goal: &str,
    in_flight: &str,
    next_action: &str,
    actor: &crate::store::Actor,
) -> Result<ReadbackCheck> {
    store.write(|tx| {
        let s = crate::session::get(tx, session_id)?
            .ok_or_else(|| err(format!("unknown session {session_id}")))?;
        let check = check_readback(tx, &s, goal, in_flight, next_action)?;
        let data = ReadbackData {
            result: check.result,
            diff: check.diff.clone(),
        };
        crate::store::append_in(
            tx,
            crate::store::EventDraft::new(
                &s.repo_id,
                &s.session_id,
                "readback",
                serde_json::to_value(data)?,
            )
            .actor(actor.clone()),
        )?;
        Ok(check)
    })
}

#[cfg(test)]
mod tests;
