//! The handoff part of a hook (spec §8): checkpoint commits at Stop and after
//! `git commit`. Git runs only here, and only when a checkpoint is pending or a commit
//! was just made, so the common hook path stays git-free.

use std::path::{Path, PathBuf};

use bridge_screen_core::checkpoint::{self, StopGit};
use bridge_screen_core::config::Config;
use bridge_screen_core::error::Result;
use bridge_screen_core::git::{self, HeadCommit};
use bridge_screen_core::handoff::{self, EndKind, ReadbackState};
use bridge_screen_core::navigator::{Candidate, Channel, NoteKind, note_sent_draft};
use bridge_screen_core::recovery;
use bridge_screen_core::session::{self, Session};
use bridge_screen_core::session::{NeedsOwner, NeedsOwnerKind};
use bridge_screen_core::squash::{self, Outcome, Refusal};
use bridge_screen_core::store::Store;
use bridge_screen_core::store::{Actor, EventDraft, append_in};
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
    let now = jiff::Timestamp::now();
    recovery::scan_crashed(store, repo_id, now, config.crash_timeout_min)?;
    let Some(me) = session::get(store.conn(), session_id)? else {
        return Ok(());
    };
    let takeover = recovery::Takeover::new(now, config.crash_takeover_hours);
    recovery::analyse(store, repo_id, session_id, &worktree_of(&me), &takeover)?;
    Ok(())
}

/// At SessionStart: the briefing (recorded as `briefed`), or the short re-briefing after
/// a compaction.
pub fn start_context(store: &mut Store, input: &HookInput) -> Result<Option<String>> {
    let Some(me) = session::get(store.conn(), &input.session_id)? else {
        return Ok(None);
    };
    if input.source.as_deref() == Some("compact") {
        return Ok(Some(handoff::compact_rebriefing(store.conn(), &me)?));
    }
    let b = handoff::briefing(store.conn(), &me)?;
    let data = handoff::Briefed {
        status: b.status,
        readback_required: b.readback_required,
    };
    store.append(EventDraft::new(
        &me.repo_id,
        &me.session_id,
        "briefed",
        serde_json::to_value(data)?,
    ))?;
    Ok(Some(b.text))
}

/// PreToolUse on an edit tool while a read-back is pending: the nudge, once per session
/// (not blocking). Returns its `note_sent` event and text.
pub fn readback_nudge(
    conn: &rusqlite::Connection,
    known: Option<&Session>,
    is_edit: bool,
) -> Result<Option<(EventDraft, String)>> {
    let Some(s) = known.filter(|s| is_edit && s.readback == ReadbackState::Pending) else {
        return Ok(None);
    };
    let sent: i64 = conn
        .prepare_cached(
            "SELECT count(*) FROM notes_sent WHERE session_id = ?1 AND kind = 'read_back'",
        )?
        .query_row([&s.session_id], |r| r.get(0))?;
    if sent > 0 {
        return Ok(None);
    }
    let unit_no = session::last_unit(conn, &s.session_id)?.map_or(0, |u| u.unit_no);
    let note = Candidate::new(
        NoteKind::ReadBack,
        Channel::PreToolUse,
        handoff::READBACK_NUDGE,
        "readback",
    );
    let draft = note_sent_draft(&s.repo_id, &s.session_id, unit_no, &note)?;
    Ok(Some((draft, note.text)))
}

/// Is this tool call about to open a pull request (`gh pr create`, or a GitHub MCP
/// `create_pull_request` tool)?
pub fn opens_pr(input: &HookInput) -> bool {
    bash_command(input).is_some_and(squash::is_pr_command)
        || input
            .tool_name
            .as_deref()
            .is_some_and(|t| t.ends_with("create_pull_request"))
}

/// The PR's base branch, as the command or tool names it; resolved against `origin/` when
/// there is no local branch of that name.
fn pr_base(wt: &Path, input: &HookInput) -> Option<String> {
    let named = match bash_command(input) {
        Some(cmd) => squash::pr_base(cmd),
        None => input
            .tool_input
            .as_ref()
            .and_then(|t| t.get("base"))
            .and_then(|b| b.as_str())
            .map(str::to_string),
    }?;
    if git::rev_parse(wt, &named).ok().flatten().is_some() {
        return Some(named);
    }
    let remote = format!("origin/{named}");
    git::rev_parse(wt, &remote)
        .ok()
        .flatten()
        .map(|_| remote)
        .or(Some(named))
}

/// Before a PR opens (spec §8.1): squash the branch's WIP commits (and force-push with
/// lease if it was pushed). Never blocks: no WIP commits → nothing; any failure →
/// `squash_failed`, a needs-you item, and a note that the PR will show the WIP commits.
pub fn before_pr(store: &mut Store, input: &HookInput) -> Result<Option<String>> {
    if !opens_pr(input) {
        return Ok(None);
    }
    let Some(s) = session::get(store.conn(), &input.session_id)? else {
        return Ok(None);
    };
    let wt = worktree_of(&s);
    let opts = squash::Options {
        onto: pr_base(&wt, input),
        dry_run: false,
    };
    let draft = |kind: &str, data: serde_json::Value| {
        EventDraft::new(&s.repo_id, &s.session_id, kind, data)
    };
    let failed = |store: &mut Store, reason: String| -> Result<Option<String>> {
        store.write(|tx| {
            append_in(
                tx,
                draft("squash_failed", serde_json::json!({ "reason": reason })),
            )?;
            let owner = NeedsOwner {
                kind: NeedsOwnerKind::Squash,
            };
            append_in(tx, draft("needs_owner", serde_json::to_value(owner)?))?;
            Ok(())
        })?;
        Ok(Some(format!(
            "bridge-screen: WIP commits weren't squashed ({reason}); the PR will show them. Run bridge-screen squash later if you want them collapsed."
        )))
    };
    match squash::squash(&wt, &opts, store.conn(), &s.repo_id) {
        Ok(Outcome::Refused(Refusal::NoWip) | Outcome::DryRun(_)) => Ok(None),
        Ok(Outcome::Refused(r)) => failed(store, r.to_string()),
        Err(e) => failed(store, e.to_string()),
        Ok(Outcome::Squashed(done)) => {
            store.append(draft("squashed", serde_json::to_value(done.event())?))?;
            let n = done.old_shas.len();
            if let Some(Err(e)) = &done.push {
                return failed(
                    store,
                    format!("squashed {n} commits locally but the push failed: {e}"),
                );
            }
            Ok(Some(format!(
                "bridge-screen: squashed {n} commits into one before the PR (backup: {}).",
                done.backup_ref
            )))
        }
    }
}
