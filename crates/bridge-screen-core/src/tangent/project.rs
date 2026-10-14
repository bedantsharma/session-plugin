//! Tangent event payloads and the projector that keeps `detours`, `suspended_items`, unit
//! labels, away time, `notes_sent`, `drift_samples` and `repo_tuning` up to date.

use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use super::{ItemKind, Label, LabelSource, NavState, get_detour, get_item};
use crate::error::Result;
use crate::session::{self, current_unit};
use crate::store::Event;

/// Event kinds routed to this projector.
pub const TANGENT_KINDS: &[&str] = &[
    "detour_opened",
    "detour_closed",
    "detour_promoted",
    "item_suspended",
    "item_resumed",
    "unit_labeled",
    "drift_flagged",
    "nav_state_changed",
    "owner_action",
    "note_sent",
    "config_changed",
    "tuning_ran",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetourOpened {
    pub detour_id: String,
    pub reason: String,
    /// The task it blocks; `None` when the session has no task.
    #[serde(default)]
    pub blocks: Option<String>,
    pub resume_cue: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetourClosed {
    pub detour_id: String,
    /// done or abandoned.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetourPromoted {
    pub detour_id: String,
    pub task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSuspended {
    pub item_id: String,
    pub kind: ItemKind,
    pub ref_id: String,
    pub resume_cue: String,
    /// The detour whose opening suspended the item.
    #[serde(default)]
    pub by_detour: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemResumed {
    pub item_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitLabeled {
    pub unit_no: i64,
    pub label: Label,
    pub source: LabelSource,
    /// The unit's drift score, when known (otherwise the one stored with the unit is kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// The label the score alone implies under the thresholds in force; `None` in the
    /// band where the heuristic can't tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heuristic: Option<Label>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftFlagged {
    pub unit_no: i64,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavStateChanged {
    pub from: NavState,
    pub to: NavState,
    pub away_secs: i64,
}

/// Something the owner did. For drift answers: `action = "drift"`, `answer` is
/// `continue`, `make_task` or `stop`, and `target` is the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerAction {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteSent {
    pub unit_no: i64,
    /// A `navigator::NoteKind`, snake_case.
    pub kind: String,
    /// A `navigator::Channel`, snake_case.
    pub channel: String,
    /// Identifies the situation the note was about; a new key means the state changed.
    #[serde(default)]
    pub state_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigChanged {
    /// Always `repo` for now.
    pub scope: String,
    /// drift_lower, drift_upper or tolerance_pct.
    pub key: String,
    #[serde(default)]
    pub old: Option<f64>,
    pub new: f64,
    /// Drift samples the repo had when the change was made.
    #[serde(default)]
    pub samples: i64,
}

/// A tuning run that changed nothing: the next run waits for `RUN_EVERY` samples more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuningRan {
    /// Drift samples the repo had when the run was made.
    pub samples: i64,
}

/// Keys of `repo_tuning` a `config_changed` may set.
pub const TUNING_KEYS: &[&str] = &["drift_lower", "drift_upper", "tolerance_pct"];

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

/// Apply one tangent event to the tangent views.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    match event.kind.as_str() {
        "owner_action" => owner_action(tx, event, &payload(event)?),
        "config_changed" => {
            // Per-repo test settings share the event; their values aren't numbers.
            if event.data["key"]
                .as_str()
                .is_some_and(|k| crate::verify::SETTING_KEYS.contains(&k))
            {
                return crate::verify::project::setting_changed(tx, event);
            }
            config_changed(tx, event, &payload(event)?)
        }
        "tuning_ran" => {
            let d: TuningRan = payload(event)?;
            tx.execute(
                "INSERT INTO repo_tuning (repo_id, samples_at_last_run) VALUES (?1, ?2)
                 ON CONFLICT(repo_id) DO UPDATE SET samples_at_last_run = excluded.samples_at_last_run",
                params![event.repo_id, d.samples],
            )?;
            Ok(())
        }
        kind => {
            // Everything else belongs to a session bridge-screen has seen.
            if session::get(tx, &event.session_id)?.is_none() {
                return Ok(());
            }
            match kind {
                "detour_opened" => detour_opened(tx, event, &payload(event)?),
                "detour_closed" => {
                    let d: DetourClosed = payload(event)?;
                    close_detour(tx, event, &d.detour_id, &d.outcome)
                }
                "detour_promoted" => {
                    let d: DetourPromoted = payload(event)?;
                    tx.execute(
                        "UPDATE detours SET promoted_task = ?2 WHERE detour_id = ?1",
                        params![d.detour_id, d.task_id],
                    )?;
                    let current =
                        session::get(tx, &event.session_id)?.and_then(|s| s.current_detour);
                    close_detour(tx, event, &d.detour_id, "promoted")?;
                    // The session now works on the promoted task itself.
                    if current.as_deref() == Some(d.detour_id.as_str()) {
                        set_current_detour(tx, &event.session_id, None)?;
                    }
                    Ok(())
                }
                "item_suspended" => {
                    let d: ItemSuspended = payload(event)?;
                    tx.execute(
                        "INSERT OR REPLACE INTO suspended_items
                             (item_id, session_id, kind, ref_id, resume_cue, by_detour, suspended_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            d.item_id,
                            event.session_id,
                            d.kind.as_str(),
                            d.ref_id,
                            d.resume_cue,
                            d.by_detour,
                            event.ts
                        ],
                    )?;
                    Ok(())
                }
                "item_resumed" => item_resumed(tx, event, &payload(event)?),
                "unit_labeled" => unit_labeled(tx, event, &payload(event)?),
                "drift_flagged" => {
                    let d: DriftFlagged = payload(event)?;
                    tx.execute(
                        "UPDATE units SET flagged = 1, drift_score = ?3
                         WHERE session_id = ?1 AND unit_no = ?2",
                        params![event.session_id, d.unit_no, d.score],
                    )?;
                    Ok(())
                }
                "nav_state_changed" => {
                    let d: NavStateChanged = payload(event)?;
                    tx.execute(
                        "UPDATE sessions SET nav_state = ?2 WHERE session_id = ?1",
                        params![event.session_id, d.to.as_str()],
                    )?;
                    Ok(())
                }
                "note_sent" => note_sent(tx, event, &payload(event)?),
                _ => Ok(()),
            }
        }
    }
}

fn set_current_detour(tx: &Transaction, session_id: &str, detour: Option<&str>) -> Result<()> {
    tx.execute(
        "UPDATE sessions SET current_detour = ?2 WHERE session_id = ?1",
        params![session_id, detour],
    )?;
    Ok(())
}

/// The open unit (if any) now counts as time on `detour`.
fn tag_open_unit(tx: &Transaction, session_id: &str, detour: &str) -> Result<()> {
    if let Some(u) = current_unit(tx, session_id)? {
        tx.execute(
            "UPDATE units SET detour_id = ?3 WHERE session_id = ?1 AND unit_no = ?2",
            params![session_id, u.unit_no, detour],
        )?;
    }
    Ok(())
}

fn detour_opened(tx: &Transaction, event: &Event, d: &DetourOpened) -> Result<()> {
    let sid = &event.session_id;
    let parent = session::get(tx, sid)?.and_then(|s| s.current_detour);
    let open_unit = current_unit(tx, sid)?.map(|u| u.unit_no);
    let no: i64 = tx.query_row(
        "SELECT count(*) + 1 FROM detours WHERE session_id = ?1",
        [sid],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO detours (detour_id, session_id, repo_id, no, blocks_task, reason,
                                        resume_cue, opened_at, opened_unit, parent_detour)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            d.detour_id,
            sid,
            event.repo_id,
            no,
            d.blocks,
            d.reason,
            d.resume_cue,
            event.ts,
            open_unit,
            parent
        ],
    )?;
    set_current_detour(tx, sid, Some(&d.detour_id))?;
    tag_open_unit(tx, sid, &d.detour_id)
}

/// Close a detour. If it was the focus, focus moves to the nearest detour it was opened
/// inside that is still open, or to the task.
fn close_detour(tx: &Transaction, event: &Event, detour_id: &str, outcome: &str) -> Result<()> {
    tx.execute(
        "UPDATE detours SET closed_at = COALESCE(closed_at, ?2), outcome = COALESCE(outcome, ?3)
         WHERE detour_id = ?1",
        params![detour_id, event.ts, outcome],
    )?;
    let current = session::get(tx, &event.session_id)?.and_then(|s| s.current_detour);
    if current.as_deref() != Some(detour_id) {
        return Ok(());
    }
    let mut next = get_detour(tx, detour_id)?.and_then(|d| d.parent_detour);
    while let Some(id) = next.clone() {
        match get_detour(tx, &id)? {
            Some(d) if d.closed_at.is_none() => break,
            Some(d) => next = d.parent_detour,
            None => next = None,
        }
    }
    set_current_detour(tx, &event.session_id, next.as_deref())
}

fn item_resumed(tx: &Transaction, event: &Event, d: &ItemResumed) -> Result<()> {
    let Some(item) = get_item(tx, &d.item_id)?.filter(|i| i.session_id == event.session_id) else {
        return Ok(());
    };
    tx.execute(
        "UPDATE suspended_items SET resumed_at = ?2 WHERE item_id = ?1 AND resumed_at IS NULL",
        params![d.item_id, event.ts],
    )?;
    match item.kind {
        ItemKind::Task => set_current_detour(tx, &event.session_id, None),
        ItemKind::Detour => {
            set_current_detour(tx, &event.session_id, Some(&item.ref_id))?;
            tag_open_unit(tx, &event.session_id, &item.ref_id)
        }
    }
}

fn unit_labeled(tx: &Transaction, event: &Event, d: &UnitLabeled) -> Result<()> {
    let sid = &event.session_id;
    let Some(unit) = session::unit(tx, sid, d.unit_no)? else {
        return Ok(());
    };
    let score = d.score.or(unit.drift_score);
    let judged = matches!(d.source, LabelSource::Agent | LabelSource::Owner);
    let disagreement = judged && d.heuristic.is_some_and(|h| h != d.label);
    tx.execute(
        "UPDATE units SET label = ?3, label_source = ?4, drift_score = ?5, disagreement = ?6
         WHERE session_id = ?1 AND unit_no = ?2",
        params![
            sid,
            d.unit_no,
            d.label.as_str(),
            d.source.as_str(),
            score,
            disagreement
        ],
    )?;
    // Training signal for auto-tuning (spec §7.6): every owner label, and every agent
    // label the heuristic didn't already give (including the band where it can't tell).
    let sample = d.source == LabelSource::Owner
        || (d.source == LabelSource::Agent && d.heuristic != Some(d.label));
    if let Some(score) = score.filter(|_| sample) {
        tx.execute(
            "INSERT INTO drift_samples (repo_id, unit_ref, score, label, source, ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.repo_id,
                format!("{sid}:{}", d.unit_no),
                score,
                d.label.as_str(),
                d.source.as_str(),
                event.ts
            ],
        )?;
    }
    recompute_away(tx, sid)
}

/// Away time is the run of most recent labelled units that are not on-task (unlabelled
/// units are still waiting for their label and are skipped). Recomputed from the units,
/// so a late label or a correction lands in the right place.
fn recompute_away(tx: &Transaction, session_id: &str) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "SELECT started_at, active_secs, label FROM units
         WHERE session_id = ?1 AND stopped_at IS NOT NULL ORDER BY unit_no DESC",
    )?;
    let mut rows = stmt.query([session_id])?;
    let mut away = 0_i64;
    let mut since: Option<String> = None;
    while let Some(row) = rows.next()? {
        let label: Option<String> = row.get(2)?;
        match label.as_deref().and_then(Label::parse) {
            None => {}
            Some(Label::OnTask) => break,
            Some(_) => {
                away += row.get::<_, i64>(1)?;
                since = Some(row.get(0)?);
            }
        }
    }
    tx.execute(
        "UPDATE sessions SET away_secs = ?2, away_since = ?3,
                             ack_away_secs = CASE WHEN ?2 = 0 THEN 0 ELSE ack_away_secs END
         WHERE session_id = ?1",
        params![session_id, away, since],
    )?;
    Ok(())
}

fn owner_action(tx: &Transaction, event: &Event, d: &OwnerAction) -> Result<()> {
    if d.action != "drift" {
        return Ok(());
    }
    let (Some(target), Some(answer)) = (&d.target, &d.answer) else {
        return Ok(());
    };
    if session::get(tx, target)?.is_none() {
        return Ok(());
    }
    tx.execute(
        "UPDATE sessions SET pending_reply = ?2 WHERE session_id = ?1",
        params![target, format!("{answer}:{}", event.id)],
    )?;
    if answer == "continue" {
        tx.execute(
            "UPDATE sessions SET ack_away_secs = away_secs,
                 ack_unit = (SELECT COALESCE(MAX(unit_no), 0) FROM units
                             WHERE session_id = ?1 AND stopped_at IS NOT NULL)
             WHERE session_id = ?1",
            [target],
        )?;
    }
    Ok(())
}

fn note_sent(tx: &Transaction, event: &Event, d: &NoteSent) -> Result<()> {
    tx.execute(
        "INSERT INTO notes_sent (session_id, unit_no, kind, state_key, channel, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event.session_id,
            d.unit_no,
            d.kind,
            d.state_key,
            d.channel,
            event.ts
        ],
    )?;
    if d.kind == "checkpoint" {
        tx.execute(
            "UPDATE sessions SET checkpoint_asked_unit = ?2 WHERE session_id = ?1",
            params![event.session_id, d.unit_no],
        )?;
    }
    if d.kind == "owner_reply" {
        tx.execute(
            "UPDATE sessions SET pending_reply = NULL WHERE session_id = ?1",
            [&event.session_id],
        )?;
    }
    Ok(())
}

fn config_changed(tx: &Transaction, event: &Event, d: &ConfigChanged) -> Result<()> {
    let Some(column) = TUNING_KEYS.iter().find(|k| **k == d.key) else {
        return Ok(());
    };
    tx.execute(
        &format!(
            "INSERT INTO repo_tuning (repo_id, {column}, samples_at_last_run) VALUES (?1, ?2, ?3)
             ON CONFLICT(repo_id) DO UPDATE SET {column} = excluded.{column},
                 samples_at_last_run = excluded.samples_at_last_run"
        ),
        params![event.repo_id, d.new, d.samples],
    )?;
    Ok(())
}
