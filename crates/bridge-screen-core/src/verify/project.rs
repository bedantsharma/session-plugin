//! Verification event payloads and the projector for `verifications`, `verify_nudges`
//! and `repo_settings` (spec §8.5).
//!
//! `claimed_done` and `squashed` belong to the handoff projector, `config_changed` to the
//! tangent one; they call [`refresh_status`](super::refresh_status), [`repoint`] and
//! [`setting_changed`] for their verification side.

use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{By, Rerun, SETTING_KEYS, refresh_status};
use crate::error::Result;
use crate::store::Event;

/// Event kinds routed to this projector.
pub const VERIFY_KINDS: &[&str] = &[
    "verified",
    "verification_failed",
    "verification_stale",
    "verify_nudged",
    "verification_withdrawn",
];

/// The owner took back a verdict (owner only): `verification_id` is the `verif_id` of a
/// sign-off, or the event id of a "broken" (`verification_failed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationWithdrawn {
    pub task_id: String,
    pub verification_id: String,
}

/// A task reached a level of the ladder, at commit `sha`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verified {
    pub verif_id: String,
    pub task_id: String,
    /// 1–4.
    pub level: i64,
    pub sha: String,
    /// What the verification depended on (relative paths): a later commit touching one
    /// of them drops it a level.
    #[serde(default)]
    pub files: Vec<String>,
    pub evidence: String,
    pub by: By,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerun: Option<Rerun>,
}

/// A verification attempt showed the task isn't done: back to work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationFailed {
    pub task_id: String,
    pub level: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    pub evidence: String,
    pub by: By,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerun: Option<Rerun>,
}

/// Commits since the verification touched files it depended on: one level down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationStale {
    pub verif_id: String,
    pub task_id: String,
    pub from_level: i64,
    pub to_level: i64,
    /// The HEAD the change was found at; the next check starts from here.
    pub head_sha: String,
    /// The depended-on files that changed.
    #[serde(default)]
    pub changed: Vec<String>,
}

/// The agent was reminded that a task is claimed but not verified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyNudged {
    pub task_id: String,
    /// The task's level when nudged (0 = claimed).
    pub level: i64,
}

/// A per-repo setting changed (`config_changed` with a [`SETTING_KEYS`] key); `new` is
/// `null` when it was unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingChanged {
    /// Always `repo`.
    pub scope: String,
    pub key: String,
    #[serde(default)]
    pub old: Value,
    #[serde(default)]
    pub new: Value,
}

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

/// Apply one verification event to the verification views.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    let repo = event.repo_id.as_str();
    match event.kind.as_str() {
        "verified" => {
            let d: Verified = payload(event)?;
            tx.execute(
                "INSERT OR IGNORE INTO verifications
                     (verif_id, repo_id, task_id, level, orig_level, sha, files, evidence,
                      verified_by, rerun, session_id, state, stale_checked_sha, ts)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'current', ?5, ?11)",
                params![
                    d.verif_id,
                    repo,
                    d.task_id,
                    d.level,
                    d.sha,
                    serde_json::to_string(&d.files)?,
                    d.evidence,
                    d.by.as_str(),
                    d.rerun.map(Rerun::as_str),
                    event.session_id,
                    event.ts
                ],
            )?;
            if d.level >= 2 {
                // The nudges were answered.
                clear_nudges(tx, repo, &d.task_id)?;
            }
            refresh_status(tx, repo, &d.task_id, false)?;
        }
        "verification_failed" => {
            let d: VerificationFailed = payload(event)?;
            // Remember which failure wiped each row and what it was, so the owner can take a
            // "broken" back.
            tx.execute(
                "UPDATE verifications SET failed_from = state, failed_by = ?3, state = 'failed'
                 WHERE repo_id = ?1 AND task_id = ?2 AND state IN ('current', 'stale')",
                params![repo, d.task_id, event.id],
            )?;
            clear_nudges(tx, repo, &d.task_id)?;
            refresh_status(tx, repo, &d.task_id, true)?;
        }
        "verification_stale" => {
            let d: VerificationStale = payload(event)?;
            let dropped = tx.execute(
                "UPDATE verifications SET level = ?2, state = 'stale', stale_checked_sha = ?3
                 WHERE verif_id = ?1 AND state != 'failed'",
                params![d.verif_id, d.to_level.max(0), d.head_sha],
            )?;
            // A drop of a row that failed meanwhile changes nothing (the task stays failed).
            if dropped > 0 {
                refresh_status(tx, repo, &d.task_id, false)?;
            }
        }
        "verification_withdrawn" => {
            let d: VerificationWithdrawn = payload(event)?;
            let signoff = tx.execute(
                "UPDATE verifications SET state = 'withdrawn'
                 WHERE verif_id = ?1 AND repo_id = ?2 AND state IN ('current', 'stale')",
                params![d.verification_id, repo],
            )?;
            if signoff == 0 {
                // A "broken" taken back: what it wiped counts again.
                tx.execute(
                    "UPDATE verifications SET state = COALESCE(failed_from, 'current'),
                         failed_from = NULL, failed_by = NULL
                     WHERE repo_id = ?1 AND task_id = ?2 AND failed_by = ?3",
                    params![repo, d.task_id, d.verification_id],
                )?;
            }
            refresh_status(tx, repo, &d.task_id, false)?;
        }
        "verify_nudged" => {
            let d: VerifyNudged = payload(event)?;
            // Nudges count while the level stays put; a level reached since starts the
            // count again there (that nudge wasn't ignored).
            tx.execute(
                "INSERT INTO verify_nudges (repo_id, task_id, nudges, level_at_first)
                 VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(repo_id, task_id) DO UPDATE SET
                     nudges = CASE WHEN level_at_first = excluded.level_at_first
                                   THEN nudges + 1 ELSE 1 END,
                     level_at_first = excluded.level_at_first",
                params![repo, d.task_id, d.level],
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn clear_nudges(tx: &Transaction, repo: &str, task: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM verify_nudges WHERE repo_id = ?1 AND task_id = ?2",
        params![repo, task],
    )?;
    Ok(())
}

/// After a squash: verifications made on any of `old_shas` (in this repo) now point at
/// `new_sha`, which has the same tree (spec §8.1).
pub fn repoint(tx: &Transaction, repo: &str, old_shas: &[String], new_sha: &str) -> Result<()> {
    let mut sha =
        tx.prepare_cached("UPDATE verifications SET sha = ?3 WHERE repo_id = ?1 AND sha = ?2")?;
    let mut checked = tx.prepare_cached(
        "UPDATE verifications SET stale_checked_sha = ?3
         WHERE repo_id = ?1 AND stale_checked_sha = ?2",
    )?;
    for old in old_shas {
        sha.execute(params![repo, old, new_sha])?;
        checked.execute(params![repo, old, new_sha])?;
    }
    Ok(())
}

/// A `config_changed` for a per-repo test setting: `repo_settings` takes the new value
/// (`null` clears it).
pub fn setting_changed(tx: &Transaction, event: &Event) -> Result<()> {
    let d: SettingChanged = payload(event)?;
    let Some(column) = SETTING_KEYS.iter().find(|k| **k == d.key) else {
        return Ok(());
    };
    let value: Option<rusqlite::types::Value> = match &d.new {
        Value::String(s) => Some(s.clone().into()),
        Value::Number(n) => n.as_i64().map(Into::into),
        _ => None,
    };
    tx.execute(
        &format!(
            "INSERT INTO repo_settings (repo_id, {column}) VALUES (?1, ?2)
             ON CONFLICT(repo_id) DO UPDATE SET {column} = excluded.{column}"
        ),
        params![event.repo_id, value],
    )?;
    Ok(())
}
