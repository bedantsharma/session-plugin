//! L1 — the code exists at HEAD (spec §8.5): the files named when the task was claimed
//! done (else the files its units changed) are in HEAD's tree, committed, and the named
//! symbols appear in them as whole words. Read-only: the caller records the outcome.

use std::path::Path;

use rusqlite::{Connection, params};

use super::{By, Verified};
use crate::error::Result;
use crate::git;
use crate::handoff::latest_claim;
use crate::store::EventDraft;

/// What an L1 check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum L1Outcome {
    /// Every file is in HEAD and every symbol in one of them.
    Passed { sha: String, files: Vec<String> },
    /// Some files aren't in HEAD's tree, or some symbols aren't in the files.
    Missing {
        sha: String,
        paths: Vec<String>,
        symbols: Vec<String>,
    },
    /// Some of the files have uncommitted changes: the check waits for the commit.
    NotCommitted { paths: Vec<String> },
    /// Nothing names the files the work is in.
    NoEvidence,
}

/// A path as the agent may write it, relative to the worktree root: trimmed, without a
/// leading `./`. Absolute paths inside `wt` become relative.
pub fn normalize(path: &str, wt: Option<&Path>) -> String {
    let p = path.trim();
    if let Some(root) = wt
        && let Ok(rel) = Path::new(p).strip_prefix(root)
    {
        return rel.to_string_lossy().into_owned();
    }
    p.trim_start_matches("./").to_string()
}

/// Relative paths recorded on the task's units, in any session (sorted, unique).
fn unit_files(conn: &Connection, repo_id: &str, task_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT u.files FROM units u JOIN sessions s ON s.session_id = u.session_id
         WHERE s.repo_id = ?1 AND u.task_id = ?2",
    )?;
    let rows = stmt.query_map(params![repo_id, task_id], |r| r.get::<_, String>(0))?;
    let mut files = Vec::new();
    for row in rows {
        let list: Vec<String> = serde_json::from_str(&row?)?;
        files.extend(list.into_iter().filter(|f| !Path::new(f).is_absolute()));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// The files (and symbols) L1 checks: the latest claim's, else the task's unit files
/// that exist in the worktree now.
pub fn evidence(
    conn: &Connection,
    wt: &Path,
    repo_id: &str,
    task_id: &str,
) -> Result<(Vec<String>, Vec<String>)> {
    let claim = latest_claim(conn, repo_id, task_id)?;
    let symbols = claim
        .as_ref()
        .map(|c| c.symbols.clone())
        .unwrap_or_default();
    if let Some(c) = claim.filter(|c| !c.files.is_empty()) {
        return Ok((c.files, symbols));
    }
    let files = unit_files(conn, repo_id, task_id)?
        .into_iter()
        .filter(|f| wt.join(f).exists())
        .collect();
    Ok((files, symbols))
}

/// Everything a verification of the task depends on: the claimed files plus every
/// relative file its units changed (sorted, unique).
pub fn depended_files(conn: &Connection, repo_id: &str, task_id: &str) -> Result<Vec<String>> {
    let mut files = unit_files(conn, repo_id, task_id)?;
    if let Some(c) = latest_claim(conn, repo_id, task_id)? {
        files.extend(c.files);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Of `files`, those with uncommitted changes in `wt` (sorted).
pub fn uncommitted(wt: &Path, files: &[String]) -> Result<Vec<String>> {
    let dirty = git::status(wt)?.dirty_paths();
    Ok(files
        .iter()
        .filter(|f| dirty.binary_search(f).is_ok())
        .cloned()
        .collect())
}

/// Check L1 for `task_id` at HEAD of `wt`. Runs git; records nothing.
pub fn check_l1(conn: &Connection, wt: &Path, repo_id: &str, task_id: &str) -> Result<L1Outcome> {
    let (files, symbols) = evidence(conn, wt, repo_id, task_id)?;
    if files.is_empty() {
        return Ok(L1Outcome::NoEvidence);
    }
    let status = git::status(wt)?;
    let dirty = status.dirty_paths();
    let pending: Vec<String> = files
        .iter()
        .filter(|f| dirty.binary_search(f).is_ok())
        .cloned()
        .collect();
    let Some(sha) = status.head else {
        return Ok(L1Outcome::NotCommitted { paths: files });
    };
    if !pending.is_empty() {
        return Ok(L1Outcome::NotCommitted { paths: pending });
    }
    // Two git calls whatever the number of files and symbols: hooks run this.
    let missing = git::missing_at(wt, &sha, &files)?;
    let present: Vec<String> = files
        .iter()
        .filter(|f| !missing.contains(f))
        .cloned()
        .collect();
    let missing_symbols = git::words_missing_at(wt, &sha, &symbols, &present)?;
    if missing.is_empty() && missing_symbols.is_empty() {
        return Ok(L1Outcome::Passed { sha, files });
    }
    Ok(L1Outcome::Missing {
        sha,
        paths: missing,
        symbols: missing_symbols,
    })
}

/// Tasks this session claimed done that are still at `claimed` (no L1 yet).
pub fn unverified_claims(conn: &Connection, session_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT c.task_id FROM claims_done c
         JOIN plan_nodes n ON n.repo_id = c.repo_id AND n.id = c.task_id
         WHERE c.session_id = ?1 AND n.status = 'claimed'
         ORDER BY c.task_id",
    )?;
    let rows = stmt.query_map([session_id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A `verified` event, with a fresh id.
#[allow(clippy::too_many_arguments)] // one per payload field
pub fn verified_draft(
    repo_id: &str,
    session_id: &str,
    task_id: &str,
    level: i64,
    sha: &str,
    files: Vec<String>,
    evidence: &str,
    by: By,
    rerun: Option<super::Rerun>,
) -> Result<EventDraft> {
    let data = Verified {
        verif_id: ulid::Ulid::generate().to_string(),
        task_id: task_id.to_string(),
        level,
        sha: sha.to_string(),
        files,
        evidence: evidence.chars().take(super::MAX_EVIDENCE).collect(),
        by,
        rerun,
    };
    Ok(EventDraft::new(repo_id, session_id, "verified", serde_json::to_value(data)?).task(task_id))
}

/// The `verified {level 1, by auto}` event for a passed check.
pub fn l1_draft(
    repo_id: &str,
    session_id: &str,
    task_id: &str,
    outcome: &L1Outcome,
) -> Result<Option<EventDraft>> {
    let L1Outcome::Passed { sha, files } = outcome else {
        return Ok(None);
    };
    Ok(Some(verified_draft(
        repo_id,
        session_id,
        task_id,
        1,
        sha,
        files.clone(),
        "code exists at HEAD",
        By::Auto,
        None,
    )?))
}

/// `src/a.rs, fn login` — what an L1 check didn't find.
pub fn missing_text(paths: &[String], symbols: &[String]) -> String {
    paths
        .iter()
        .cloned()
        .chain(symbols.iter().map(|s| format!("symbol {s}")))
        .collect::<Vec<_>>()
        .join(", ")
}
