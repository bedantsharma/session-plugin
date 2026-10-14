//! Staleness (spec §8.5): a verification depends on files at a commit. When later commits
//! touch one of them, it drops one level (`verification_stale`) and is re-checked at the
//! next session start. Run at SessionStart, where git already runs.
//!
//! Idempotent: a drop moves the row's `stale_checked_sha` to the HEAD it was found at, so
//! a second start at the same HEAD skips it. A row whose files weren't touched keeps its
//! old `stale_checked_sha`: the next start looks from there again. One `git log` of
//! HEAD's newest [`HISTORY`] commits answers every row: a file counts as changed when a
//! commit after the verification's (and not before it) touched it. Commits that aren't
//! in that history (another branch, rewritten history, or older) are left alone.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use super::{VerificationStale, check_l1, current_level, l1_draft, live_verifications};
use crate::error::Result;
use crate::git;
use crate::store::{EventDraft, Store, append_in};

/// How many of the newest verifications a start looks at.
pub const SCAN_LIMIT: usize = 50;

/// How far back from HEAD a start looks: verifications on older commits are left alone.
pub const HISTORY: usize = 1000;

/// The paths touched by the commits in `sha..HEAD` (`history` is HEAD's newest commits,
/// newest first), or `None` when `sha` isn't among them: another branch, rewritten
/// history, or too old. Commits reachable from `sha` are the ones not counted.
fn touched_since<'a>(history: &'a [git::CommitTouch], sha: &str) -> Option<HashSet<&'a str>> {
    let index: HashMap<&str, &git::CommitTouch> =
        history.iter().map(|c| (c.sha.as_str(), c)).collect();
    index.get(sha)?;
    let mut before: HashSet<&str> = HashSet::new();
    let mut queue = vec![sha];
    while let Some(id) = queue.pop() {
        if !before.insert(id) {
            continue;
        }
        if let Some(c) = index.get(id) {
            queue.extend(c.parents.iter().map(String::as_str));
        }
    }
    Some(
        history
            .iter()
            .filter(|c| !before.contains(c.sha.as_str()))
            .flat_map(|c| c.files.iter().map(String::as_str))
            .collect(),
    )
}

/// One drop a scan recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleChange {
    pub task_id: String,
    pub from_level: i64,
    pub to_level: i64,
    pub changed: Vec<String>,
}

/// Find verifications whose files changed between their commit and HEAD of `wt`, drop
/// each one level, then re-check L1 at HEAD for tasks left with nothing. Events go out as
/// `session_id`. Git failures mean "nothing known": that commit is skipped.
pub fn scan_stale(
    store: &mut Store,
    repo_id: &str,
    session_id: &str,
    wt: &Path,
) -> Result<Vec<StaleChange>> {
    let Ok(Some(head)) = git::head(wt) else {
        return Ok(Vec::new());
    };
    let rows = live_verifications(store.conn(), repo_id, SCAN_LIMIT)?;
    let mut groups: BTreeMap<&str, Vec<&super::Verification>> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.level >= 1) {
        groups
            .entry(row.stale_checked_sha.as_str())
            .or_default()
            .push(row);
    }
    if groups.keys().all(|sha| *sha == head) {
        return Ok(Vec::new());
    }
    let Ok(history) = git::history(wt, &head, HISTORY) else {
        return Ok(Vec::new());
    };
    let mut drafts = Vec::new();
    let mut changes = Vec::new();
    for (sha, group) in groups {
        if sha == head {
            continue;
        }
        let Some(touched) = touched_since(&history, sha) else {
            continue;
        };
        for row in group {
            let hit: Vec<String> = row
                .files
                .iter()
                .filter(|f| touched.contains(f.as_str()))
                .cloned()
                .collect();
            if hit.is_empty() {
                continue;
            }
            let data = VerificationStale {
                verif_id: row.verif_id.clone(),
                task_id: row.task_id.clone(),
                from_level: row.level,
                to_level: row.level - 1,
                head_sha: head.clone(),
                changed: hit.clone(),
            };
            drafts.push(
                EventDraft::new(
                    repo_id,
                    session_id,
                    "verification_stale",
                    serde_json::to_value(data)?,
                )
                .task(&row.task_id),
            );
            changes.push(StaleChange {
                task_id: row.task_id.clone(),
                from_level: row.level,
                to_level: row.level - 1,
                changed: hit,
            });
        }
    }
    if drafts.is_empty() {
        return Ok(changes);
    }
    store.write(|tx| {
        for d in drafts {
            append_in(tx, d)?;
        }
        Ok(())
    })?;
    // Tasks a drop left with nothing: their code may still be there at HEAD.
    let mut emptied: Vec<&str> = changes
        .iter()
        .filter(|c| c.to_level < 1)
        .map(|c| c.task_id.as_str())
        .collect();
    emptied.sort_unstable();
    emptied.dedup();
    for task in emptied {
        if current_level(store.conn(), repo_id, task)? >= 1 {
            continue;
        }
        let Ok(outcome) = check_l1(store.conn(), wt, repo_id, task) else {
            continue;
        };
        if let Some(draft) = l1_draft(repo_id, session_id, task, &outcome)? {
            store.append(draft)?;
        }
    }
    Ok(changes)
}

/// `stale: src/a.rs changed @abc1234` when the task's newest verification went stale, for
/// the briefing.
pub fn stale_note(conn: &Connection, repo_id: &str, task_id: &str) -> Result<Option<String>> {
    let newest: Option<(String, String)> = conn
        .prepare_cached(
            "SELECT verif_id, state FROM verifications
             WHERE repo_id = ?1 AND task_id = ?2 AND state IN ('current', 'stale')
             ORDER BY rowid DESC LIMIT 1",
        )?
        .query_row(params![repo_id, task_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some(verif_id) = newest
        .filter(|(_, state)| state == "stale")
        .map(|(id, _)| id)
    else {
        return Ok(None);
    };
    let data: Option<String> = conn
        .prepare_cached(
            "SELECT data FROM events WHERE repo_id = ?1 AND kind = 'verification_stale'
               AND json_extract(data, '$.verif_id') = ?2
             ORDER BY pos DESC LIMIT 1",
        )?
        .query_row(params![repo_id, verif_id], |r| r.get(0))
        .optional()?;
    let Some(data) = data else {
        return Ok(None);
    };
    let d: VerificationStale = serde_json::from_str(&data)?;
    let short: String = d.head_sha.chars().take(7).collect();
    Ok(Some(format!(
        "stale: {} changed @{short}",
        d.changed.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::touched_since;
    use crate::git::CommitTouch;

    fn c(sha: &str, parents: &[&str], files: &[&str]) -> CommitTouch {
        CommitTouch {
            sha: sha.into(),
            parents: parents.iter().map(|p| (*p).to_string()).collect(),
            files: files.iter().map(|f| (*f).to_string()).collect(),
        }
    }

    #[test]
    fn only_commits_after_the_verification_count() {
        // a ← b ← d (merge of b and c) ← e;  a ← c (a side branch).
        let history = vec![
            c("e", &["d"], &["e.rs"]),
            c("d", &["b", "c"], &[]),
            c("c", &["a"], &["c.rs"]),
            c("b", &["a"], &["b.rs"]),
            c("a", &[], &["a.rs"]),
        ];
        let set = |sha: &str| {
            let mut v: Vec<&str> = touched_since(&history, sha).unwrap().into_iter().collect();
            v.sort_unstable();
            v
        };
        assert_eq!(set("e"), Vec::<&str>::new());
        assert_eq!(set("b"), vec!["c.rs", "e.rs"]);
        assert_eq!(set("c"), vec!["b.rs", "e.rs"]);
        assert_eq!(set("a"), vec!["b.rs", "c.rs", "e.rs"]);
        assert_eq!(touched_since(&history, "zzz"), None);
    }
}
