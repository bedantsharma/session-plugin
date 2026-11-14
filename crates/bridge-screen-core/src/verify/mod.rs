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
pub mod withdraw;

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
    SettingChanged, VERIFY_KINDS, VerificationFailed, VerificationStale, VerificationWithdrawn,
    Verified, VerifyNudged, project,
};
pub use stale::{StaleChange, scan_stale, stale_note};
pub use withdraw::{Withdrawable, withdraw, withdrawable};

/// What a level of the ladder means, in the owner's words (L-codes stay in the model).
pub fn level_words(level: i64) -> &'static str {
    match level {
        ..=0 => "said done, not checked yet",
        1 => "code written, tests not run",
        2 => "tests pass",
        3 => "tried end to end",
        _ => "you signed it off",
    }
}

/// Longest evidence kept, in characters.
pub const MAX_EVIDENCE: usize = 500;

/// Keys of `repo_settings` a `config_changed` may set. Only the owner sets them (CLI).
pub const SETTING_KEYS: &[&str] = &["test_command", "test_timeout_s", "publish_reports"];

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
    /// The owner took this sign-off back: it counts for nothing.
    Withdrawn,
}

impl VerifState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Failed => "failed",
            Self::Withdrawn => "withdrawn",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "current" => Some(Self::Current),
            "stale" => Some(Self::Stale),
            "failed" => Some(Self::Failed),
            "withdrawn" => Some(Self::Withdrawn),
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

/// Fewest words and characters of a claim's summary that count as a saved result for a
/// review task that names no files.
const REVIEW_MIN_WORDS: usize = 10;
const REVIEW_MIN_CHARS: usize = 60;

/// Is the task checked by review (not tests)?
pub fn is_review(conn: &Connection, repo_id: &str, task_id: &str) -> Result<bool> {
    let kind: Option<String> = conn
        .prepare_cached("SELECT check_kind FROM plan_nodes WHERE repo_id = ?1 AND id = ?2")?
        .query_row(params![repo_id, task_id], |r| r.get(0))
        .optional()?;
    Ok(kind.as_deref() == Some("review"))
}

/// Has a review task's result been saved? The newest claim names files and the L1 check
/// found them committed; or it names none and its summary is a real one (at least
/// [`REVIEW_MIN_WORDS`] words and [`REVIEW_MIN_CHARS`] characters). Files named but not
/// yet committed leave it claimed until the checkpoint commit.
pub fn review_saved(conn: &Connection, repo_id: &str, task_id: &str) -> Result<bool> {
    let Some(claim) = crate::handoff::latest_claim(conn, repo_id, task_id)? else {
        return Ok(false);
    };
    if !claim.files.is_empty() {
        return Ok(current_level(conn, repo_id, task_id)? >= 1);
    }
    Ok(review_text_is_real(&claim.evidence))
}

/// Is a claim's summary a real review result (not a one-liner)?
pub fn review_text_is_real(evidence: &str) -> bool {
    let text = evidence.trim();
    text.split_whitespace().count() >= REVIEW_MIN_WORDS && text.chars().count() >= REVIEW_MIN_CHARS
}

/// Set a task's plan status from its verifications and claims (never a dropped node):
/// - `failed` when this is a `verification_failed`;
/// - else `l2`..`l4` by the highest level of its current and stale rows (a row at 0 counts
///   for nothing);
/// - else `reviewed` for a review task whose result is saved;
/// - else `l1` at level 1;
/// - else `claimed` when it was claimed done;
/// - else `active` when a live session holds it, or `todo` (after a withdrawn sign-off or
///   a verification that dropped to nothing).
///
/// `claim_task` moves `failed` back to `active` (see `plan::project::mark_claimed`).
pub fn refresh_status(tx: &Transaction, repo_id: &str, task_id: &str, failed: bool) -> Result<()> {
    let status = if failed {
        "failed".to_string()
    } else {
        let level = current_level(tx, repo_id, task_id)?;
        if level >= 2 {
            format!("l{}", level.min(4))
        } else if is_review(tx, repo_id, task_id)? && review_saved(tx, repo_id, task_id)? {
            "reviewed".to_string()
        } else if level >= 1 {
            "l1".to_string()
        } else {
            let claimed: i64 = tx
                .prepare_cached(
                    "SELECT count(*) FROM claims_done WHERE repo_id = ?1 AND task_id = ?2",
                )?
                .query_row(params![repo_id, task_id], |r| r.get(0))?;
            if claimed > 0 {
                "claimed".to_string()
            } else {
                let held: i64 = tx
                    .prepare_cached(
                        "SELECT count(*) FROM sessions
                         WHERE repo_id = ?1 AND active_task = ?2 AND state != 'ended'",
                    )?
                    .query_row(params![repo_id, task_id], |r| r.get(0))?;
                if held > 0 { "active" } else { "todo" }.to_string()
            }
        }
    };
    if status == "reviewed" {
        // Nothing is left to nudge about.
        tx.execute(
            "DELETE FROM verify_nudges WHERE repo_id = ?1 AND task_id = ?2",
            params![repo_id, task_id],
        )?;
    }
    tx.execute(
        "UPDATE plan_nodes SET status = ?3
         WHERE repo_id = ?1 AND id = ?2 AND status != 'dropped'",
        params![repo_id, task_id, status],
    )?;
    Ok(())
}

/// The task's `check` changed: a task that was `reviewed` may no longer be done, and one
/// that was claimed may now be. A failed task stays failed.
pub fn refresh_after_check_switch(tx: &Transaction, repo_id: &str, task_id: &str) -> Result<()> {
    let status: Option<String> = tx
        .prepare_cached("SELECT status FROM plan_nodes WHERE repo_id = ?1 AND id = ?2")?
        .query_row(params![repo_id, task_id], |r| r.get(0))
        .optional()?;
    match status.as_deref() {
        None | Some("dropped" | "todo" | "active") => Ok(()),
        Some(s) => refresh_status(tx, repo_id, task_id, s == "failed"),
    }
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

/// Per-repo settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoSettings {
    pub test_command: Option<String>,
    pub test_timeout_s: Option<u64>,
    /// `never` makes `progress_report` refuse; unset means `ask` (spec §10.6).
    pub publish_reports: Option<String>,
}

pub fn settings(conn: &Connection, repo_id: &str) -> Result<RepoSettings> {
    Ok(conn
        .prepare_cached(
            "SELECT test_command, test_timeout_s, publish_reports FROM repo_settings
             WHERE repo_id = ?1",
        )?
        .query_row([repo_id], |r| {
            Ok(RepoSettings {
                test_command: r.get(0)?,
                test_timeout_s: r
                    .get::<_, Option<i64>>(1)?
                    .and_then(|s| u64::try_from(s).ok()),
                publish_reports: r.get(2)?,
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
    /// How many of the done leaves are done without tests (review tasks with a saved
    /// result); they count in `done_pct`.
    pub reviewed_tasks: usize,
}

fn pct(part: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    u32::try_from(part * 100 / total).unwrap_or(100)
}

impl Progress {
    /// Done% and claimed% over `leaves` (live tasks and subtasks with no live children).
    pub fn of_leaves(leaves: &[&crate::plan::PlanNode]) -> Self {
        let mut p = Progress::default();
        for leaf in leaves {
            let minutes = leaf.estimate_min.map(u64::from);
            if minutes.is_none() {
                p.unestimated += 1;
            }
            let minutes = minutes.unwrap_or(0);
            p.total_min += minutes;
            if leaf.status.counts_as_done() {
                p.done_min += minutes;
                if leaf.status == crate::plan::NodeStatus::Reviewed {
                    p.reviewed_tasks += 1;
                }
            } else if leaf.status.is_claimed_not_done() {
                p.claimed_min += minutes;
            }
        }
        p.done_pct = pct(p.done_min, p.total_min);
        p.claimed_pct = pct(p.claimed_min, p.total_min);
        p
    }
}

/// Done% and claimed% of the **current run of one track** (spec §10.1); closed runs are
/// left out.
pub fn progress(conn: &Connection, repo_id: &str, track: u32) -> Result<Progress> {
    let tree = crate::plan::runs::run_tree(conn, repo_id, track)?;
    Ok(Progress::of_leaves(&crate::plan::runs::leaf_nodes(&tree)))
}

/// [`progress`] pooled over every open track (estimate-weighted). With one open track it
/// is that track's.
pub fn progress_all(conn: &Connection, repo_id: &str) -> Result<Progress> {
    let tree = crate::plan::runs::open_run_tree(conn, repo_id)?;
    Ok(Progress::of_leaves(&crate::plan::runs::leaf_nodes(&tree)))
}

#[cfg(test)]
mod tests;
