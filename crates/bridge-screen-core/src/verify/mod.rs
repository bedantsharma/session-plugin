//! The verification ladder (spec §8.5): L0 claimed → L1 code exists at HEAD → L2 unit
//! tests pass (re-run when the owner set a test command) → L3 end-to-end → L4 owner.
//!
//! Every verification is tied to a commit SHA and to the files it depended on; when later
//! commits touch those files it drops one level (`verification_stale`). The task's plan
//! status is derived from its verifications by [`refresh_status`].

pub mod l1;
pub mod ladder;
pub mod nudge;
pub mod project;
pub mod rerun;
pub mod stale;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;

pub use l1::{
    L1Outcome, check_l1, depended_files, l1_draft, missing_text, uncommitted, unverified_claims,
    verified_draft,
};
pub use ladder::{VerifyOutcome, VerifyRequest, change_setting, owner_verdict, verify};
pub use nudge::{ESCALATE_AFTER, nudge_drafts, overdue, pending_claims};
pub use project::{
    SettingChanged, VERIFY_KINDS, VerificationFailed, VerificationStale, Verified, VerifyNudged,
    project,
};
pub use stale::{StaleChange, scan_stale, stale_note};

/// Longest evidence kept, in characters.
pub const MAX_EVIDENCE: usize = 500;

/// Keys of `repo_settings` a `config_changed` may set. Only the owner sets them (CLI).
pub const SETTING_KEYS: &[&str] = &["test_command", "test_timeout_s"];

/// Who (or what) reached a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// bridge-screen itself (L1 at HEAD).
    Auto,
    /// As the agent reported it.
    Agent,
    /// bridge-screen re-ran the repo's test command.
    Rerun,
    /// The owner, from the CLI or dashboard.
    Owner,
    /// The owner's words, relayed by the agent.
    OwnerViaAgent,
}

impl By {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Agent => "agent",
            Self::Rerun => "rerun",
            Self::Owner => "owner",
            Self::OwnerViaAgent => "owner_via_agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "agent" => Some(Self::Agent),
            "rerun" => Some(Self::Rerun),
            "owner" => Some(Self::Owner),
            "owner_via_agent" => Some(Self::OwnerViaAgent),
            _ => None,
        }
    }
}

/// What the L2 re-run of the test command said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rerun {
    Passed,
    Failed,
    TimedOut,
    /// No test command is set (or it couldn't be started): recorded as reported.
    NotConfigured,
}

impl Rerun {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::NotConfigured => "not_configured",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "passed" => Some(Self::Passed),
            "failed" => Some(Self::Failed),
            "timed_out" => Some(Self::TimedOut),
            "not_configured" => Some(Self::NotConfigured),
            _ => None,
        }
    }
}

/// A verification row's standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifState {
    Current,
    /// Dropped one or more levels since it was made; `level` holds the dropped level.
    Stale,
    /// A later attempt failed: it counts for nothing.
    Failed,
}

impl VerifState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "current" => Some(Self::Current),
            "stale" => Some(Self::Stale),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One row of `verifications`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verification {
    pub verif_id: String,
    pub repo_id: String,
    pub task_id: String,
    /// The level it counts for now (stale rows hold the dropped level; 0 counts nothing).
    pub level: i64,
    pub orig_level: i64,
    pub sha: String,
    pub files: Vec<String>,
    pub evidence: String,
    pub by: By,
    pub rerun: Option<Rerun>,
    pub session_id: String,
    pub state: VerifState,
    /// The commit staleness was last checked against.
    pub stale_checked_sha: String,
    pub ts: String,
}

const VERIF_COLUMNS: &str = "verif_id, repo_id, task_id, level, orig_level, sha, files,
     evidence, verified_by, rerun, session_id, state, stale_checked_sha, ts";

fn read_verif(r: &Row) -> rusqlite::Result<(Verification, String)> {
    let by: String = r.get(8)?;
    let state: String = r.get(11)?;
    Ok((
        Verification {
            verif_id: r.get(0)?,
            repo_id: r.get(1)?,
            task_id: r.get(2)?,
            level: r.get(3)?,
            orig_level: r.get(4)?,
            sha: r.get(5)?,
            files: Vec::new(),
            evidence: r.get(7)?,
            by: By::parse(&by).unwrap_or(By::Agent),
            rerun: r
                .get::<_, Option<String>>(9)?
                .as_deref()
                .and_then(Rerun::parse),
            session_id: r.get(10)?,
            state: VerifState::parse(&state).unwrap_or(VerifState::Current),
            stale_checked_sha: r.get(12)?,
            ts: r.get(13)?,
        },
        r.get(6)?,
    ))
}

fn with_files((mut v, files): (Verification, String)) -> Result<Verification> {
    v.files = serde_json::from_str(&files)?;
    Ok(v)
}

/// A task's verifications, oldest first.
pub fn verifications(conn: &Connection, repo_id: &str, task_id: &str) -> Result<Vec<Verification>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {VERIF_COLUMNS} FROM verifications WHERE repo_id = ?1 AND task_id = ?2
         ORDER BY rowid"
    ))?;
    let rows = stmt.query_map(params![repo_id, task_id], read_verif)?;
    rows.map(|r| with_files(r?)).collect()
}

/// The repo's verifications that still count (current or stale), newest first, at most
/// `limit`.
pub fn live_verifications(
    conn: &Connection,
    repo_id: &str,
    limit: usize,
) -> Result<Vec<Verification>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {VERIF_COLUMNS} FROM verifications
         WHERE repo_id = ?1 AND state IN ('current', 'stale')
         ORDER BY rowid DESC LIMIT ?2"
    ))?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = stmt.query_map(params![repo_id, limit], read_verif)?;
    rows.map(|r| with_files(r?)).collect()
}

/// The highest level a task's current or stale verifications count for (0 when none).
pub fn current_level(conn: &Connection, repo_id: &str, task_id: &str) -> Result<i64> {
    Ok(conn
        .prepare_cached(
            "SELECT COALESCE(MAX(level), 0) FROM verifications
             WHERE repo_id = ?1 AND task_id = ?2 AND state IN ('current', 'stale')",
        )?
        .query_row(params![repo_id, task_id], |r| r.get(0))?)
}

/// The verification that gives the task its level, if any (the newest at that level).
pub fn top_verification(
    conn: &Connection,
    repo_id: &str,
    task_id: &str,
) -> Result<Option<Verification>> {
    conn.prepare_cached(&format!(
        "SELECT {VERIF_COLUMNS} FROM verifications
         WHERE repo_id = ?1 AND task_id = ?2 AND state IN ('current', 'stale') AND level >= 1
         ORDER BY level DESC, rowid DESC LIMIT 1"
    ))?
    .query_row(params![repo_id, task_id], read_verif)
    .optional()?
    .map(with_files)
    .transpose()
}

/// Set a task's plan status from its verifications and claims (never a dropped node):
/// - `failed` when this is a `verification_failed`;
/// - else `l<max level>` of its current and stale rows (a row at 0 counts for nothing);
/// - else `claimed` when it was claimed done;
/// - else the status is left alone (todo or active).
///
/// `claim_task` moves `failed` back to `active` (see `plan::project::mark_claimed`).
pub fn refresh_status(tx: &Transaction, repo_id: &str, task_id: &str, failed: bool) -> Result<()> {
    let status = if failed {
        Some("failed".to_string())
    } else {
        let level = current_level(tx, repo_id, task_id)?;
        if level >= 1 {
            Some(format!("l{}", level.min(4)))
        } else {
            let claimed: i64 = tx
                .prepare_cached(
                    "SELECT count(*) FROM claims_done WHERE repo_id = ?1 AND task_id = ?2",
                )?
                .query_row(params![repo_id, task_id], |r| r.get(0))?;
            (claimed > 0).then(|| "claimed".to_string())
        }
    };
    if let Some(status) = status {
        tx.execute(
            "UPDATE plan_nodes SET status = ?3
             WHERE repo_id = ?1 AND id = ?2 AND status != 'dropped'",
            params![repo_id, task_id, status],
        )?;
    }
    Ok(())
}

/// Plan-tree markers from verifications: `stale` when a task's newest verification went
/// stale, `✓owner` when its L4 came from the owner.
pub fn plan_marks(
    conn: &Connection,
    repo_id: &str,
) -> Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT task_id FROM verifications
         WHERE repo_id = ?1 AND state IN ('current', 'stale')",
    )?;
    let tasks = stmt
        .query_map([repo_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut marks = std::collections::HashMap::new();
    for task in tasks {
        if stale_note(conn, repo_id, &task)?.is_some() {
            marks.insert(task, "stale".to_string());
        } else if top_verification(conn, repo_id, &task)?
            .is_some_and(|v| v.level == 4 && v.by == By::Owner)
        {
            marks.insert(task, "✓owner".to_string());
        }
    }
    Ok(marks)
}

/// Per-repo test settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoSettings {
    pub test_command: Option<String>,
    pub test_timeout_s: Option<u64>,
}

pub fn settings(conn: &Connection, repo_id: &str) -> Result<RepoSettings> {
    Ok(conn
        .prepare_cached(
            "SELECT test_command, test_timeout_s FROM repo_settings WHERE repo_id = ?1",
        )?
        .query_row([repo_id], |r| {
            Ok(RepoSettings {
                test_command: r.get(0)?,
                test_timeout_s: r
                    .get::<_, Option<i64>>(1)?
                    .and_then(|s| u64::try_from(s).ok()),
            })
        })
        .optional()?
        .unwrap_or_default())
}

/// Progress of a repo's plan (spec §5.6), over live leaves (tasks and subtasks with no
/// live children). Leaves without an estimate count 0 minutes and are counted apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// Percent of the estimate at L2 or above.
    pub done_pct: u32,
    /// Percent claimed or at L1: shown apart, never part of done%.
    pub claimed_pct: u32,
    pub done_min: u64,
    pub claimed_min: u64,
    pub total_min: u64,
    pub unestimated: usize,
}

fn pct(part: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    u32::try_from(part * 100 / total).unwrap_or(100)
}

pub fn progress(conn: &Connection, repo_id: &str) -> Result<Progress> {
    let mut stmt = conn.prepare_cached(
        "SELECT n.status, n.estimate_min FROM plan_nodes n
         WHERE n.repo_id = ?1 AND n.kind IN ('task', 'subtask') AND n.status != 'dropped'
           AND NOT EXISTS (SELECT 1 FROM plan_nodes c
                           WHERE c.repo_id = n.repo_id AND c.parent_id = n.id
                             AND c.status != 'dropped')",
    )?;
    let rows = stmt.query_map([repo_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
    })?;
    let mut p = Progress::default();
    for row in rows {
        let (status, estimate) = row?;
        let minutes = estimate.and_then(|m| u64::try_from(m).ok());
        if minutes.is_none() {
            p.unestimated += 1;
        }
        let minutes = minutes.unwrap_or(0);
        p.total_min += minutes;
        match status.as_str() {
            "l2" | "l3" | "l4" => p.done_min += minutes,
            "claimed" | "l1" => p.claimed_min += minutes,
            _ => {}
        }
    }
    p.done_pct = pct(p.done_min, p.total_min);
    p.claimed_pct = pct(p.claimed_min, p.total_min);
    Ok(p)
}

#[cfg(test)]
mod tests;
