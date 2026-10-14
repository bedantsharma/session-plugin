//! Owner actions (spec §10.1–10.5): every click on a needs-you item — on the dashboard,
//! in `watch`, on the desktop card — and every owner CLI command goes through [`act`], so
//! each records the same events wherever it came from.
//!
//! Each action first checks, inside its write transaction, that the thing it answers is
//! still open; a stale or double click is refused with [`Error::Stale`] and records
//! nothing.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::handoff::LeftoverAction;
use crate::tangent::Label;

/// The owner's answer to a drift question (spec §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriftAnswer {
    /// Keep going: the away time so far is acknowledged.
    Continue,
    /// Promote the current detour to a task (needs `parent` and `title`).
    #[serde(alias = "make-task")]
    MakeTask,
    /// Stop the detour and return to the task.
    Stop,
}

impl DriftAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::MakeTask => "make_task",
            Self::Stop => "stop",
        }
    }
}

fn level_four() -> i64 {
    4
}

/// One owner action. The JSON form (`{"kind": "drift", …}`) is what the screens send
/// back: each needs-you item carries the exact actions it offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnerAction {
    /// Answer a drift question about a live session (id or unique prefix).
    Drift {
        session: String,
        answer: DriftAnswer,
        /// make_task: the new task's parent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// make_task: the new task's title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// make_task: estimate in agent-minutes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        estimate: Option<u32>,
        /// continue: why, in the owner's words (optional; at least 8 characters and 2
        /// words when given).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Set a unit's label (overrides the agent and the heuristic).
    Label {
        session: String,
        unit_no: i64,
        label: Label,
    },
    /// Resolve leftovers of a dead session.
    Resolve {
        /// The repo; found from the leftovers when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
        leftover_ids: Vec<String>,
        action: LeftoverAction,
    },
    /// It works: the owner's own check (L4 by default).
    Verify {
        repo: String,
        task: String,
        #[serde(default = "level_four")]
        level: i64,
    },
    /// It doesn't work: back to failed, with the owner's note.
    Broken {
        repo: String,
        task: String,
        note: String,
    },
    /// Put the repo's auto-tuned drift thresholds back to the defaults.
    ResetDrift { repo: String },
}
