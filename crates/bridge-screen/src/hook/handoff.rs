//! The handoff part of a hook (spec §8): checkpoint commits at Stop and after
//! `git commit`. Git runs only here, and only when a checkpoint is pending or a commit
//! was just made, so the common hook path stays git-free.

use std::path::{Path, PathBuf};

use bridge_screen_core::checkpoint::{self, StopGit};
use bridge_screen_core::config::Config;
use bridge_screen_core::error::Result;
use bridge_screen_core::git::{self, HeadCommit};
use bridge_screen_core::handoff::EndKind;
use bridge_screen_core::recovery;
use bridge_screen_core::session::{self, Session};
use bridge_screen_core::store::Store;
use bridge_screen_core::store::{Actor, append_in};
use rusqlite::Transaction;

use super::{HookEvent, HookInput};

/// What git said, read before the write transaction so the lock isn't held while git runs.
#[derive(Debug, Clone, Default)]
pub struct CheckpointGit {
    pub head: Option<HeadCommit>,
    /// Read at Stop only.
    pub dirty: Option<bool>,
}

/// The session's worktree: recorded at start, else its working directory.
pub fn worktree_of(session: &Session) -> PathBuf {
    PathBuf::from(session.worktree.as_deref().unwrap_or(&session.cwd))
}

/// The Bash command of a tool call, if it is one.
pub fn bash_command(input: &HookInput) -> Option<&str> {
    if input.tool_name.as_deref() != Some("Bash") {
        return None;
    }
    input.tool_input.as_ref()?.get("command")?.as_str()
}

/// Read git if this hook needs it: after a Bash `git commit`, or at Stop with a
/// checkpoint pending. A git failure (e.g. the worktree is gone) means "nothing known".
pub fn gather(
    event: HookEvent,
    known: Option<&Session>,
    input: &HookInput,
) -> Option<CheckpointGit> {
    let session = known?;
    let wt = worktree_of(session);
    match event {
        HookEvent::PostToolUse
            if bash_command(input).is_some_and(checkpoint::is_commit_command) =>
        {
            Some(CheckpointGit {
                head: head(&wt),
                dirty: None,
            })
        }
        HookEvent::Stop if session.checkpoint_pending.is_some() => Some(CheckpointGit {
            head: head(&wt),
            dirty: git::is_dirty(&wt).ok(),
        }),
        _ => None,
    }
}

fn head(wt: &Path) -> Option<HeadCommit> {
    git::head_commit(wt).ok().flatten()
}

/// Inside the hook's transaction, after its own events: record a new checkpoint commit,
/// then at Stop ask for the checkpoint (or record the miss). Returns the Stop note.
pub fn apply(
    tx: &Transaction,
    input: &HookInput,
    found: &CheckpointGit,
    stopped_unit: Option<i64>,
    actor: &Actor,
) -> Result<Option<String>> {
    let Some(s) = session::get(tx, &input.session_id)? else {
        return Ok(None);
    };
    if let Some(head) = &found.head
        && let Some(draft) = checkpoint::detect(tx, &s, head)?
    {
        append_in(tx, draft.actor(actor.clone()))?;
    }
    let Some(dirty) = found.dirty else {
        return Ok(None);
    };
    let unit_no = match stopped_unit {
        Some(n) => n,
        None => session::last_unit(tx, &s.session_id)?.map_or(0, |u| u.unit_no),
    };
    let git = StopGit {
        dirty,
        head: found.head.as_ref().map(|h| h.sha.clone()),
    };
    let step = checkpoint::stop_step(tx, &s.session_id, unit_no, input.stop_hook_active, &git)?;
    for draft in step.events {
        append_in(tx, draft.actor(actor.clone()))?;
    }
    Ok(step.note)
}

/// At SessionEnd: the end kind and uncommitted files, from `git status` in the session's
/// worktree; nothing known if git can't read it.
pub fn end_state(conn: &rusqlite::Connection, session: &Session) -> (Option<EndKind>, Vec<String>) {
    let Ok(status) = git::status(&worktree_of(session)) else {
        return (None, Vec::new());
    };
    let dirty = status.dirty_paths();
    let kind = recovery::end_kind(conn, session, &dirty).ok();
    (kind, dirty)
}

/// At SessionStart: mark silent sessions crashed, then analyse dead sessions' leftovers.
pub fn recover(store: &mut Store, config: &Config, repo_id: &str, session_id: &str) -> Result<()> {
    recovery::scan_crashed(
        store,
        repo_id,
        jiff::Timestamp::now(),
        config.crash_timeout_min,
    )?;
    let Some(me) = session::get(store.conn(), session_id)? else {
        return Ok(());
    };
    recovery::analyse(store, repo_id, session_id, &worktree_of(&me))?;
    Ok(())
}
