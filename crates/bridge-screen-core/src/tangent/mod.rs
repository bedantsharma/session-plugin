//! Tangents (spec §7): detours, suspended items with resume cues, unit labels, away time
//! and the tolerance band.
//!
//! A **detour** is work that isn't the task but blocks it. Opening one suspends the current
//! item (the task, or the detour it was opened inside) with a resume cue written at that
//! moment; any suspended item can be resumed later, in any order. Every unit gets one
//! **label** — on-task, detour or drift — and the time of consecutive units that are not
//! on-task is the session's **away time**.

pub mod ops;
pub mod project;
pub mod tolerance;

use rusqlite::{Connection, OptionalExtension, Row, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;

pub use ops::{
    CloseRequest, Closed, DetourOutcome, LabelRequest, MAX_CUE_CHARS, Promoted, StartRequest,
    Started, close_detour, item_display, label_unit, promote_detour, resume, start_detour,
};
pub use project::{
    ConfigChanged, DetourClosed, DetourOpened, DetourPromoted, DriftFlagged, ItemResumed,
    ItemSuspended, NavStateChanged, NoteSent, OwnerAction, TANGENT_KINDS, UnitLabeled, project,
};
pub use tolerance::{NavEval, drift_pause, evaluate, tolerance_secs};

/// What a unit of work was, relative to the session's task (spec §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    /// Advances the task.
    #[serde(alias = "on-task")]
    OnTask,
    /// Not the task, but it blocks the task.
    Detour,
    /// Neither advances nor blocks the task.
    Drift,
}

impl Label {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OnTask => "on_task",
            Self::Detour => "detour",
            Self::Drift => "drift",
        }
    }

    /// How people write it: `on-task`, `detour`, `drift`.
    pub fn display(self) -> &'static str {
        match self {
            Self::OnTask => "on-task",
            Self::Detour => "detour",
            Self::Drift => "drift",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "on_task" | "on-task" => Some(Self::OnTask),
            "detour" => Some(Self::Detour),
            "drift" => Some(Self::Drift),
            _ => None,
        }
    }
}

/// Who decided a unit's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSource {
    /// A detour was open during the unit.
    Declared,
    /// The drift score alone.
    Heuristic,
    /// The agent, through `label_unit`.
    Agent,
    /// The owner (CLI or dashboard).
    Owner,
}

impl LabelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Heuristic => "heuristic",
            Self::Agent => "agent",
            Self::Owner => "owner",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "declared" => Some(Self::Declared),
            "heuristic" => Some(Self::Heuristic),
            "agent" => Some(Self::Agent),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

/// The navigator light for a session (spec §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavState {
    /// Inside tolerance.
    Green,
    /// Outside tolerance.
    Amber,
    /// Far outside tolerance, or drift with no declared reason.
    Red,
}

impl NavState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Amber => "amber",
            Self::Red => "red",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "green" => Some(Self::Green),
            "amber" => Some(Self::Amber),
            "red" => Some(Self::Red),
            _ => None,
        }
    }
}

/// The cut-offs in force for a repo: config defaults, overridden by tuned values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Scores below this are on-task.
    pub lower: f64,
    /// Scores above this are possible drift.
    pub upper: f64,
    pub tolerance_min_minutes: f64,
    pub tolerance_pct: f64,
}

impl Thresholds {
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        Self {
            lower: cfg.drift_lower,
            upper: cfg.drift_upper,
            tolerance_min_minutes: f64::from(cfg.tolerance_min_minutes),
            tolerance_pct: f64::from(cfg.tolerance_pct),
        }
    }

    /// The label a score implies on its own; `None` in the band where it can't tell.
    pub fn heuristic(&self, score: f64) -> Option<Label> {
        if score < self.lower {
            Some(Label::OnTask)
        } else if score > self.upper {
            Some(Label::Drift)
        } else {
            None
        }
    }
}

/// The thresholds for `repo_id`: tuned values where the repo has them (spec §7.6).
pub fn thresholds(
    conn: &Connection,
    repo_id: &str,
    cfg: &crate::config::Config,
) -> Result<Thresholds> {
    let mut th = Thresholds::from_config(cfg);
    let tuned: Option<(Option<f64>, Option<f64>, Option<f64>)> = conn
        .prepare_cached(
            "SELECT drift_lower, drift_upper, tolerance_pct FROM repo_tuning WHERE repo_id = ?1",
        )?
        .query_row([repo_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    if let Some((lower, upper, pct)) = tuned {
        th.lower = lower.unwrap_or(th.lower);
        th.upper = upper.unwrap_or(th.upper);
        th.tolerance_pct = pct.unwrap_or(th.tolerance_pct);
    }
    Ok(th)
}

/// What a suspended item points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A plan task (`ref_id` is its plan id).
    Task,
    /// A detour (`ref_id` is its detour id).
    Detour,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Detour => "detour",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "task" => Some(Self::Task),
            "detour" => Some(Self::Detour),
            _ => None,
        }
    }
}

/// One row of the `detours` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detour {
    pub detour_id: String,
    pub session_id: String,
    pub repo_id: String,
    /// Short per-session number, shown as `D<no>`.
    pub no: i64,
    pub blocks_task: Option<String>,
    pub reason: String,
    pub resume_cue: String,
    pub opened_at: String,
    pub opened_unit: Option<i64>,
    /// The detour this one was opened inside.
    pub parent_detour: Option<String>,
    pub closed_at: Option<String>,
    /// done, abandoned or promoted.
    pub outcome: Option<String>,
    pub active_secs: i64,
    pub promoted_task: Option<String>,
}

impl Detour {
    /// `D3`.
    pub fn label(&self) -> String {
        format!("D{}", self.no)
    }
}

/// One row of the `suspended_items` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuspendedItem {
    pub item_id: String,
    pub session_id: String,
    pub kind: ItemKind,
    pub ref_id: String,
    pub resume_cue: String,
    /// The detour whose opening suspended this item.
    pub by_detour: Option<String>,
    pub suspended_at: String,
    pub resumed_at: Option<String>,
}

/// What the session is working on right now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Focus {
    pub task: Option<String>,
    pub detour: Option<Detour>,
}

const DETOUR_COLUMNS: &str = "detour_id, session_id, repo_id, no, blocks_task, reason, resume_cue,
     opened_at, opened_unit, parent_detour, closed_at, outcome, active_secs, promoted_task";

const ITEM_COLUMNS: &str =
    "item_id, session_id, kind, ref_id, resume_cue, by_detour, suspended_at, resumed_at";

fn read_detour(r: &Row) -> rusqlite::Result<Detour> {
    Ok(Detour {
        detour_id: r.get(0)?,
        session_id: r.get(1)?,
        repo_id: r.get(2)?,
        no: r.get(3)?,
        blocks_task: r.get(4)?,
        reason: r.get(5)?,
        resume_cue: r.get(6)?,
        opened_at: r.get(7)?,
        opened_unit: r.get(8)?,
        parent_detour: r.get(9)?,
        closed_at: r.get(10)?,
        outcome: r.get(11)?,
        active_secs: r.get(12)?,
        promoted_task: r.get(13)?,
    })
}

fn read_item(r: &Row) -> rusqlite::Result<SuspendedItem> {
    let kind: String = r.get(2)?;
    Ok(SuspendedItem {
        item_id: r.get(0)?,
        session_id: r.get(1)?,
        kind: ItemKind::parse(&kind).unwrap_or(ItemKind::Task),
        ref_id: r.get(3)?,
        resume_cue: r.get(4)?,
        by_detour: r.get(5)?,
        suspended_at: r.get(6)?,
        resumed_at: r.get(7)?,
    })
}

/// One detour by id.
pub fn get_detour(conn: &Connection, detour_id: &str) -> Result<Option<Detour>> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {DETOUR_COLUMNS} FROM detours WHERE detour_id = ?1"
        ))?
        .query_row([detour_id], read_detour)
        .optional()?)
}

/// The session's detours that are still open, oldest first.
pub fn open_detours(conn: &Connection, session_id: &str) -> Result<Vec<Detour>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {DETOUR_COLUMNS} FROM detours WHERE session_id = ?1 AND closed_at IS NULL
         ORDER BY no"
    ))?;
    Ok(stmt
        .query_map([session_id], read_detour)?
        .collect::<rusqlite::Result<_>>()?)
}

/// Every detour opened against a task, oldest first.
pub fn detours_for_task(conn: &Connection, repo_id: &str, task_id: &str) -> Result<Vec<Detour>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DETOUR_COLUMNS} FROM detours WHERE repo_id = ?1 AND blocks_task = ?2
         ORDER BY opened_at, no"
    ))?;
    Ok(stmt
        .query_map(params![repo_id, task_id], read_detour)?
        .collect::<rusqlite::Result<_>>()?)
}

/// Items still waiting to be resumed, oldest first. An item pointing at a detour that has
/// since closed is gone: there is nothing left to return to.
pub fn suspended(conn: &Connection, session_id: &str) -> Result<Vec<SuspendedItem>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {ITEM_COLUMNS} FROM suspended_items i
         WHERE session_id = ?1 AND resumed_at IS NULL
           AND NOT (kind = 'detour' AND EXISTS (
               SELECT 1 FROM detours d WHERE d.detour_id = i.ref_id AND d.closed_at IS NOT NULL))
         ORDER BY suspended_at, rowid"
    ))?;
    Ok(stmt
        .query_map([session_id], read_item)?
        .collect::<rusqlite::Result<_>>()?)
}

/// One suspended item by id.
pub fn get_item(conn: &Connection, item_id: &str) -> Result<Option<SuspendedItem>> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {ITEM_COLUMNS} FROM suspended_items WHERE item_id = ?1"
        ))?
        .query_row([item_id], read_item)
        .optional()?)
}

/// The session's current focus: its active task and, if it is on one, its current detour.
pub fn focus(conn: &Connection, session_id: &str) -> Result<Focus> {
    let Some(session) = crate::session::get(conn, session_id)? else {
        return Ok(Focus::default());
    };
    let detour = match &session.current_detour {
        Some(id) => get_detour(conn, id)?,
        None => None,
    };
    Ok(Focus {
        task: session.active_task,
        detour,
    })
}

#[cfg(test)]
mod tests;
