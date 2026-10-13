//! Handoff event payloads and the projector for `checkpoints`, `leftovers`,
//! `handoff_notes`, `claims_done` and the handoff columns of `sessions`.
//!
//! `session_ended` (with its end kind) and `session_crashed` change the session's life
//! cycle, so the session projector handles them.

use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use super::{BriefingStatus, LeftoverAction, ReadbackState, ResolvedBy};
use crate::error::Result;
use crate::plan::project::mark_claimed_done;
use crate::store::Event;

/// Event kinds routed to this projector.
pub const HANDOFF_KINDS: &[&str] = &[
    "claimed_done",
    "checkpoint_due",
    "checkpoint",
    "checkpoint_missed",
    "leftover_found",
    "leftover_resolved",
    "leftovers_analysed",
    "handoff_note",
    "readback",
    "briefed",
    "squashed",
    "squash_failed",
];

/// The agent says a task is done (verification level L0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedDone {
    pub task_id: String,
    pub evidence: String,
}

/// Why a checkpoint commit is due (spec §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointReason {
    ClaimedDone,
    DetourClosed,
    TaskSwitched,
}

impl CheckpointReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaimedDone => "claimed_done",
            Self::DetourClosed => "detour_closed",
            Self::TaskSwitched => "task_switched",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointDue {
    pub reason: CheckpointReason,
}

/// The `checkpoint` event: a commit that marks a boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointRecorded {
    pub sha: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub next_action: Option<String>,
}

/// The agent stopped again without the checkpoint it was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointMissed {
    pub unit_no: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftoverFound {
    pub leftover_id: String,
    pub from_session: String,
    pub worktree: String,
    pub path: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub suggested: LeftoverAction,
    /// The worktree belongs to a live session: report it, never touch it.
    #[serde(default)]
    pub report_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftoverResolved {
    pub leftover_id: String,
    pub action: LeftoverAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stash_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub by: ResolvedBy,
}

/// A dead session's leftovers have been looked at (whether or not any were found).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftoversAnalysed {
    pub from_session: String,
}

/// The `handoff_note` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffNoteData {
    #[serde(default)]
    pub task_id: Option<String>,
    pub in_flight: String,
    #[serde(default)]
    pub if_then: Vec<String>,
}

/// The `readback` event: `result` is `ok` or `mismatch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadbackData {
    pub result: ReadbackState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

/// A briefing went out at SessionStart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Briefed {
    pub status: BriefingStatus,
    pub readback_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Squashed {
    pub branch: String,
    pub old_shas: Vec<String>,
    pub new_sha: String,
    pub backup_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SquashFailed {
    pub reason: String,
}

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

/// Apply one handoff event to the handoff views.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    let sid = event.session_id.as_str();
    match event.kind.as_str() {
        "claimed_done" => {
            let d: ClaimedDone = payload(event)?;
            tx.execute(
                "INSERT INTO claims_done (repo_id, task_id, session_id, evidence, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![event.repo_id, d.task_id, sid, d.evidence, event.ts],
            )?;
            mark_claimed_done(tx, &event.repo_id, &d.task_id)?;
        }
        "checkpoint_due" => {
            let d: CheckpointDue = payload(event)?;
            tx.execute(
                "UPDATE sessions SET checkpoint_pending = COALESCE(checkpoint_pending, ?2)
                 WHERE session_id = ?1",
                params![sid, d.reason.as_str()],
            )?;
        }
        "checkpoint" => {
            let d: CheckpointRecorded = payload(event)?;
            tx.execute(
                "INSERT OR IGNORE INTO checkpoints (sha, session_id, repo_id, task_id,
                                                    next_action, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    d.sha,
                    sid,
                    event.repo_id,
                    d.task_id,
                    d.next_action,
                    event.ts
                ],
            )?;
            tx.execute(
                "UPDATE sessions SET last_checkpoint = ?2, checkpoint_pending = NULL,
                                     checkpoint_asked_unit = NULL, checkpoint_missed = 0
                 WHERE session_id = ?1",
                params![sid, d.sha],
            )?;
        }
        "checkpoint_missed" => {
            let d: CheckpointMissed = payload(event)?;
            tx.execute(
                "UPDATE sessions SET checkpoint_missed = ?2 WHERE session_id = ?1",
                params![sid, d.unit_no],
            )?;
        }
        "leftover_found" => leftover_found(tx, event, &payload(event)?)?,
        "leftover_resolved" => leftover_resolved(tx, event, &payload(event)?)?,
        "leftovers_analysed" => {
            let d: LeftoversAnalysed = payload(event)?;
            tx.execute(
                "UPDATE sessions SET leftovers_analysed = 1 WHERE session_id = ?1",
                [d.from_session],
            )?;
        }
        "handoff_note" => {
            let d: HandoffNoteData = payload(event)?;
            tx.execute(
                "INSERT INTO handoff_notes (session_id, repo_id, task_id, in_flight, if_then, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    sid,
                    event.repo_id,
                    d.task_id,
                    d.in_flight,
                    serde_json::to_string(&d.if_then)?,
                    event.ts
                ],
            )?;
        }
        "readback" => {
            let d: ReadbackData = payload(event)?;
            tx.execute(
                "UPDATE sessions SET readback = ?2 WHERE session_id = ?1",
                params![sid, d.result.as_str()],
            )?;
        }
        "briefed" => {
            let d: Briefed = payload(event)?;
            let readback = if d.readback_required {
                ReadbackState::Pending
            } else {
                ReadbackState::NotNeeded
            };
            tx.execute(
                "UPDATE sessions SET briefing_status = ?2, readback = ?3 WHERE session_id = ?1",
                params![sid, d.status.as_str(), readback.as_str()],
            )?;
        }
        // squashed and squash_failed are history only: no view yet.
        _ => {}
    }
    Ok(())
}

fn leftover_found(tx: &Transaction, event: &Event, d: &LeftoverFound) -> Result<()> {
    let state = if d.report_only { "report_only" } else { "open" };
    tx.execute(
        "INSERT OR IGNORE INTO leftovers (leftover_id, repo_id, from_session, worktree,
                                          path, task_id, suggested, state, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            d.leftover_id,
            event.repo_id,
            d.from_session,
            d.worktree,
            d.path,
            d.task_id,
            d.suggested.as_str(),
            state,
            event.ts
        ],
    )?;
    Ok(())
}

/// The first resolution stands: no later session triages the leftover again.
fn leftover_resolved(tx: &Transaction, event: &Event, d: &LeftoverResolved) -> Result<()> {
    tx.execute(
        "UPDATE leftovers SET state = 'resolved', resolution = ?2, stash_ref = ?3,
                              commit_sha = ?4, resolved_by = ?5, resolved_at = ?6
         WHERE leftover_id = ?1 AND state != 'resolved'",
        params![
            d.leftover_id,
            d.action.as_str(),
            d.stash_ref,
            d.commit,
            d.by.as_str(),
            event.ts
        ],
    )?;
    Ok(())
}
