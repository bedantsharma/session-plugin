//! The tolerance band (spec §7.3): how long a session may be away from its task before
//! the navigator says something, and when the owner is flagged.

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension};

use super::{Label, NavState, Thresholds};
use crate::error::Result;
use crate::plan::get_node;
use crate::session::{self, Session};

/// Seconds of away time tolerated: `max(floor, pct% of the task estimate)`; the floor
/// alone without an estimate.
pub fn tolerance_secs(th: &Thresholds, estimate_min: Option<u32>) -> i64 {
    let floor = th.tolerance_min_minutes * 60.0;
    let share = estimate_min.map_or(0.0, |e| th.tolerance_pct / 100.0 * f64::from(e) * 60.0);
    #[allow(clippy::cast_possible_truncation)] // minutes of tolerance, far below i64::MAX
    let secs = floor.max(share).round() as i64;
    secs
}

/// Where a session stands against its tolerance band right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavEval {
    pub state: NavState,
    /// Away time not yet acknowledged by the owner, including the live part of an open
    /// unit inside a detour.
    pub away_secs: i64,
    pub tol_secs: i64,
    /// The active task's estimate, if it has one.
    pub estimate_min: Option<u32>,
    /// The last finished unit is labelled drift with no declared reason.
    pub drift_flag: bool,
}

/// Evaluate the session's state at `now`. `now` is used for the decision only; it never
/// reaches a view (spec: projectors use event timestamps).
pub fn evaluate(
    conn: &Connection,
    session: &Session,
    th: &Thresholds,
    now: Timestamp,
) -> Result<NavEval> {
    let mut away = (session.away_secs - session.ack_away_secs).max(0);
    if session.current_detour.is_some()
        && let Some(unit) = session::current_unit(conn, &session.session_id)?
        && let Ok(start) = unit.started_at.parse::<Timestamp>()
    {
        let mut waited = unit.waited_secs;
        if let Some(since) = unit.wait_started_at.as_deref()
            && let Ok(since) = since.parse::<Timestamp>()
        {
            waited += now.duration_since(since).as_secs().max(0);
        }
        away += (now.duration_since(start).as_secs() - waited).max(0);
    }
    let drift_flag = last_unit_drift(conn, session)?;
    let floor = tolerance_secs(th, None);
    // The plan is only read when the floor alone doesn't decide.
    let estimate_min = if away > floor {
        match &session.active_task {
            Some(task) => get_node(conn, &session.repo_id, task)?.and_then(|n| n.estimate_min),
            None => None,
        }
    } else {
        None
    };
    let tol = tolerance_secs(th, estimate_min);
    let state = if away > 2 * tol || drift_flag {
        NavState::Red
    } else if away > tol {
        NavState::Amber
    } else {
        NavState::Green
    };
    Ok(NavEval {
        state,
        away_secs: away,
        tol_secs: tol,
        estimate_min,
        drift_flag,
    })
}

/// The last finished unit is labelled drift (by the agent, the owner or the heuristic
/// fallback), had no declared detour, and the owner hasn't said "continue" since. A flagged
/// unit the agent calls on-task is not drift: the disagreement is recorded, never argued
/// (spec §7.2).
fn last_unit_drift(conn: &Connection, session: &Session) -> Result<bool> {
    let row: Option<(i64, Option<String>, Option<String>)> = conn
        .prepare_cached(
            "SELECT unit_no, label, detour_id FROM units
             WHERE session_id = ?1 AND stopped_at IS NOT NULL ORDER BY unit_no DESC LIMIT 1",
        )?
        .query_row([&session.session_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    Ok(row.is_some_and(|(unit_no, label, detour)| {
        detour.is_none()
            && label.as_deref().and_then(Label::parse) == Some(Label::Drift)
            && unit_no > session.ack_unit
    }))
}

/// The repo's `drift_pause` setting: on red, the agent finishes its step, commits and
/// waits for the owner.
pub fn drift_pause(conn: &Connection, repo_id: &str) -> Result<bool> {
    let settings: Option<String> = conn
        .prepare_cached("SELECT settings FROM repos WHERE id = ?1")?
        .query_row([repo_id], |r| r.get(0))
        .optional()?;
    Ok(settings
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .is_some_and(|v| v["drift_pause"] == true))
}
