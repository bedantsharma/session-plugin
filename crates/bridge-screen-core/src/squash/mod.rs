//! Squashing a branch's WIP checkpoint commits into one (spec §8.1): by hand with
//! `bridge-screen squash`, and automatically before a PR opens.
//!
//! Safety: a dirty worktree, no WIP commits, a detached HEAD, an unknown base or a remote
//! branch that moved since the last fetch all refuse with history untouched. A squash
//! always leaves a backup ref at the old HEAD first, and pushes only with
//! `--force-with-lease` against the upstream value we last fetched.

use std::fmt::Write as _;
use std::path::Path;

use rusqlite::Connection;

use crate::checkpoint::{TASK_TRAILER, WIP_PREFIX};
use crate::claim::task_title;
use crate::error::{Error, Result};
use crate::git::{self, CommitInfo};
use crate::handoff::Squashed;

/// Trailer on the squashed commit naming the backup ref.
pub const SQUASHED_FROM: &str = "Bridge-Squashed-From";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// The base branch; defaults to `origin/HEAD`'s target, else `main`, else `master`.
    pub onto: Option<String>,
    pub dry_run: bool,
}

/// Why a squash didn't happen (history untouched).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Dirty,
    NoWip,
    Detached,
    NoBase(String),
    /// The upstream branch moved on the remote since our last fetch.
    RemoteMoved(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dirty => {
                f.write_str("the worktree has uncommitted changes; commit or stash them first")
            }
            Self::NoWip => f.write_str("no wip(bridge) commits to squash on this branch"),
            Self::Detached => f.write_str("HEAD is detached; check out a branch first"),
            Self::NoBase(b) => write!(f, "no merge base with {b}"),
            Self::RemoteMoved(u) => write!(
                f,
                "{u} moved on the remote since the last fetch; pull first, then squash"
            ),
        }
    }
}

/// What a squash would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub branch: String,
    pub base: String,
    pub merge_base: String,
    pub commits: Vec<CommitInfo>,
    /// How many of `commits` are WIP checkpoints.
    pub wips: usize,
    pub message: String,
    /// The upstream (`origin/feat`) and the value we last fetched, when there is one.
    pub upstream: Option<(String, String)>,
}

/// What a squash did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub branch: String,
    pub old_shas: Vec<String>,
    pub new_sha: String,
    pub backup_ref: String,
    /// `None` when the branch has no upstream; the push error otherwise.
    pub push: Option<std::result::Result<(), String>>,
}

impl Done {
    /// The `squashed` event payload.
    pub fn event(&self) -> Squashed {
        Squashed {
            branch: self.branch.clone(),
            old_shas: self.old_shas.clone(),
            new_sha: self.new_sha.clone(),
            backup_ref: self.backup_ref.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Refused(Refusal),
    DryRun(Plan),
    Squashed(Done),
}

fn is_wip(c: &CommitInfo) -> bool {
    c.subject.starts_with(WIP_PREFIX) || c.trailers.iter().any(|(k, _)| k == TASK_TRAILER)
}

/// The task a WIP commit belongs to: its `Bridge-Task`, else the id after `wip(bridge):`.
fn wip_task(c: &CommitInfo) -> Option<String> {
    c.trailers
        .iter()
        .find(|(k, _)| k == TASK_TRAILER)
        .map(|(_, v)| v.clone())
        .or_else(|| {
            c.subject
                .strip_prefix(WIP_PREFIX)
                .and_then(|rest| rest.split_whitespace().next())
                .map(str::to_string)
        })
}

/// `<branch>: <N> commits squashed`, a blank line, one line per task touched, one per
/// regular commit.
fn message(
    conn: &Connection,
    repo_id: &str,
    branch: &str,
    commits: &[CommitInfo],
) -> Result<String> {
    let mut tasks: Vec<String> = Vec::new();
    for c in commits.iter().filter(|c| is_wip(c)) {
        if let Some(t) = wip_task(c)
            && !tasks.contains(&t)
        {
            tasks.push(t);
        }
    }
    let mut text = format!("{branch}: {} commits squashed\n", commits.len());
    for t in &tasks {
        let title = task_title(conn, repo_id, t)?.unwrap_or_default();
        let _ = write!(text, "\n{}", format!("{t} {title}").trim_end());
    }
    for c in commits.iter().filter(|c| !is_wip(c)) {
        let _ = write!(text, "\n{}", c.subject);
    }
    Ok(text)
}

/// Check everything and build the plan, or say why not.
fn prepare(
    wt: &Path,
    opts: &Options,
    conn: &Connection,
    repo_id: &str,
) -> Result<std::result::Result<Plan, Refusal>> {
    let Some(branch) = git::current_branch(wt)? else {
        return Ok(Err(Refusal::Detached));
    };
    if git::is_dirty(wt)? {
        return Ok(Err(Refusal::Dirty));
    }
    let base = match &opts.onto {
        Some(b) => b.clone(),
        None => git::default_base(wt)?,
    };
    let Some(merge_base) = git::merge_base(wt, &base)? else {
        return Ok(Err(Refusal::NoBase(base)));
    };
    let commits = git::commits_since(wt, &merge_base)?;
    let wips = commits.iter().filter(|c| is_wip(c)).count();
    if wips == 0 {
        return Ok(Err(Refusal::NoWip));
    }
    let upstream = match git::upstream(wt)? {
        Some(u) => git::rev_parse(wt, &u)?.map(|fetched| (u, fetched)),
        None => None,
    };
    if let Some((u, fetched)) = &upstream {
        // Someone else pushed: what we last fetched isn't what the remote has, or our
        // branch doesn't contain it. Rewriting now would drop their work.
        let remote = git::upstream_remote_sha(wt).ok().flatten();
        let moved = remote.as_ref().is_some_and(|r| r != fetched)
            || !git::is_ancestor(wt, fetched, "HEAD")?;
        if moved {
            return Ok(Err(Refusal::RemoteMoved(u.clone())));
        }
    }
    let message = message(conn, repo_id, &branch, &commits)?;
    Ok(Ok(Plan {
        branch,
        base,
        merge_base,
        commits,
        wips,
        message,
        upstream,
    }))
}

/// Squash the branch's commits since its merge base with the base into one commit
/// (spec §8.1). See the module docs for the refusals. On success the backup ref
/// `refs/bridge-screen/backup/<branch>/<unix ts>` points at the old HEAD; a branch with an
/// upstream is pushed with `--force-with-lease` (a failed push keeps the local result and
/// is reported in [`Done::push`]).
pub fn squash(wt: &Path, opts: &Options, conn: &Connection, repo_id: &str) -> Result<Outcome> {
    let plan = match prepare(wt, opts, conn, repo_id)? {
        Ok(p) => p,
        Err(r) => return Ok(Outcome::Refused(r)),
    };
    if opts.dry_run {
        return Ok(Outcome::DryRun(plan));
    }
    let old_head = git::head(wt)?.ok_or_else(|| Error::Git("HEAD has no commit".into()))?;
    let backup_ref = format!(
        "refs/bridge-screen/backup/{}/{}",
        plan.branch,
        jiff::Timestamp::now().as_second()
    );
    git::create_ref(wt, &backup_ref, &old_head)?;
    // The squashed commit has exactly HEAD's tree (the worktree is clean), so it holds no
    // content that isn't already committed on the branch: the commits it replaces went
    // through the repo's commit hooks when they were made. Making it with plumbing
    // (`commit-tree`, then moving the branch with `update-ref`) re-checks nothing new by
    // skipping them — this is not `--no-verify` on new content — and it keeps a
    // PreToolUse hook from running the repo's (possibly slow) commit hooks.
    // The index and worktree are never touched, and the branch moves only if it is still
    // at the old HEAD, so a concurrent commit is never overwritten.
    let new_sha = git::commit_tree(
        wt,
        &old_head,
        &plan.merge_base,
        &plan.message,
        &[(SQUASHED_FROM.to_string(), backup_ref.clone())],
    )?;
    git::move_ref(
        wt,
        &format!("refs/heads/{}", plan.branch),
        &new_sha,
        &old_head,
        "bridge-screen squash",
    )?;
    let push = plan
        .upstream
        .as_ref()
        .map(|(_, fetched)| git::push_force_with_lease(wt, fetched).map_err(|e| e.to_string()));
    Ok(Outcome::Squashed(Done {
        branch: plan.branch,
        old_shas: plan.commits.into_iter().map(|c| c.sha).collect(),
        new_sha,
        backup_ref,
        push,
    }))
}

/// Does this shell command open a pull request (`gh pr create`)?
pub fn is_pr_command(command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words
        .windows(3)
        .any(|w| (w[0] == "gh" || w[0].ends_with("/gh")) && w[1] == "pr" && w[2] == "create")
}

/// The PR base named in a `gh pr create` command (`--base X`, `-B X`, `--base=X`).
pub fn pr_base(command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if let Some(b) = w.strip_prefix("--base=") {
            return Some(b.trim_matches(['"', '\'']).to_string());
        }
        if (*w == "--base" || *w == "-B")
            && let Some(b) = words.get(i + 1)
        {
            return Some(b.trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests;
