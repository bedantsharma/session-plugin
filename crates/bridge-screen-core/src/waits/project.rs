//! Wait event payloads and the projector that keeps the `waits` view up to date.

use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::Event;

/// Event kinds routed to this projector.
pub const WAIT_KINDS: &[&str] = &["waiting_on", "wait_cleared"];

/// A session says it is waiting on something (spec §5.2, §11.2): a task (here or in
/// another repo) or free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingOn {
    pub wait_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_repo_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_task: Option<String>,
    /// How the target reads: `2.4 Login form` or the free text.
    pub target_text: String,
    pub reason: String,
}

/// Why a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearedWhy {
    /// Its target task reached L2 or above.
    TargetDone,
    /// The waiting agent cleared it.
    Agent,
    /// The owner cleared it.
    Owner,
    /// It became a plan link (`promote_wait`).
    Promoted,
}

impl ClearedWhy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TargetDone => "target_done",
            Self::Agent => "agent",
            Self::Owner => "owner",
            Self::Promoted => "promoted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "target_done" => Some(Self::TargetDone),
            "agent" => Some(Self::Agent),
            "owner" => Some(Self::Owner),
            "promoted" => Some(Self::Promoted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitCleared {
    pub wait_id: String,
    pub why: ClearedWhy,
    /// The commit the target was verified at (`target_done`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
}

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

/// Apply one wait event to the `waits` view. A wait clears once: a later clear (or a
/// stale drop of its target) never reopens or changes it.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    match event.kind.as_str() {
        "waiting_on" => {
            let d: WaitingOn = payload(event)?;
            tx.execute(
                "INSERT OR IGNORE INTO waits (wait_id, repo_id, session_id, target_repo_id,
                                              target_task, target_text, reason, opened_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    d.wait_id,
                    event.repo_id,
                    event.session_id,
                    d.target_repo_id,
                    d.target_task,
                    d.target_text,
                    d.reason,
                    event.ts
                ],
            )?;
        }
        "wait_cleared" => {
            let d: WaitCleared = payload(event)?;
            tx.execute(
                "UPDATE waits SET cleared_at = ?2, cleared_why = ?3, cleared_sha = ?4
                 WHERE wait_id = ?1 AND cleared_at IS NULL",
                params![d.wait_id, event.ts, d.why.as_str(), d.sha],
            )?;
        }
        _ => {}
    }
    Ok(())
}
