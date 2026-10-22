//! The handoff part of a hook (spec §8): checkpoint commits at Stop and after
//! `git commit`. Git runs only here, and only when a checkpoint is pending or a commit
//! was just made, so the common hook path stays git-free.

use std::path::{Path, PathBuf};

use bridge_screen_core::checkpoint::{self, StopGit};
use bridge_screen_core::config::Config;
use bridge_screen_core::decision;
use bridge_screen_core::error::Result;
use bridge_screen_core::git::{self, HeadCommit};
use bridge_screen_core::handoff::briefing::MAX_LINES;
use bridge_screen_core::handoff::{self, EndKind, ReadbackState};
use bridge_screen_core::navigator::{Candidate, Channel, NoteKind, note_sent_draft};
use bridge_screen_core::pages;
use bridge_screen_core::paths::Home;
use bridge_screen_core::recovery;
use bridge_screen_core::session::{self, Session};
use bridge_screen_core::session::{NeedsOwner, NeedsOwnerKind};
use bridge_screen_core::squash::{self, Outcome, Refusal};
use bridge_screen_core::store::Store;
use bridge_screen_core::store::{Actor, EventDraft, append_in};
use bridge_screen_core::verify::{self, L1Outcome};
use rusqlite::Transaction;

use super::{HookEvent, HookInput};

/// What git said, read before the write transaction so the lock isn't held while git runs.
#[derive(Debug, Clone, Default)]
pub struct CheckpointGit {
    pub head: Option<HeadCommit>,
    /// Read at Stop only.
    pub dirty: Option<bool>,
    /// L1 checks of the session's claimed tasks that have no L1 yet (spec §8.5), for
    /// when a checkpoint gets recorded.
    pub l1: Vec<(String, L1Outcome)>,
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
/// Either way, the session's claimed tasks without L1 are checked too: a checkpoint
/// commit is when their files become committed.
pub fn gather(
    conn: &rusqlite::Connection,
    event: HookEvent,
    known: Option<&Session>,
    input: &HookInput,
) -> Option<CheckpointGit> {
    let session = known?;
    let wt = worktree_of(session);
    let mut found = match event {
        HookEvent::PostToolUse
            if bash_command(input).is_some_and(checkpoint::is_commit_command) =>
        {
            CheckpointGit {
                head: head(&wt),
                ..CheckpointGit::default()
            }
        }
        HookEvent::Stop if session.checkpoint_pending.is_some() => CheckpointGit {
            head: head(&wt),
            dirty: git::is_dirty(&wt).ok(),
            l1: Vec::new(),
        },
        _ => return None,
    };
    // Like the git reads, a failure here only means no L1 checks: the hook's own events
    // must still be recorded.
    for task in verify::unverified_claims(conn, &session.session_id).unwrap_or_default() {
        if let Ok(outcome) = verify::check_l1(conn, &wt, &session.repo_id, &task) {
            found.l1.push((task, outcome));
        }
    }
    Some(found)
}

fn head(wt: &Path) -> Option<HeadCommit> {
    git::head_commit(wt).ok().flatten()
}

/// Inside the hook's transaction, after its own events: record a new checkpoint commit,
/// then at Stop ask for the checkpoint (or record the miss). When a checkpoint was
/// recorded, claimed tasks whose files now exist at HEAD reach L1; after a commit, files
/// that don't exist are named in the context. Returns the note for the agent.
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
    let mut recorded = false;
    if let Some(head) = &found.head
        && let Some(draft) = checkpoint::detect(tx, &s, head)?
    {
        append_in(tx, draft.actor(actor.clone()))?;
        recorded = true;
    }
    let mut note = None;
    if let Some(dirty) = found.dirty {
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
            recorded |= draft.kind == "checkpoint";
            append_in(tx, draft.actor(actor.clone()))?;
        }
        note = step.note;
    }
    if recorded {
        let missing = l1_at_checkpoint(tx, &s, &found.l1, actor)?;
        // Only the commit's PostToolUse says what's missing: a Stop note would force a turn.
        if found.dirty.is_none() {
            note = note.or(missing);
        }
    }
    Ok(note)
}

/// L1 for claimed tasks at a checkpoint: `verified {level 1, by auto}` for each pass, and
/// the context line for files or symbols not found.
fn l1_at_checkpoint(
    tx: &Transaction,
    s: &Session,
    checks: &[(String, L1Outcome)],
    actor: &Actor,
) -> Result<Option<String>> {
    let mut missing = Vec::new();
    for (task, outcome) in checks {
        if let Some(draft) = verify::l1_draft(&s.repo_id, &s.session_id, task, outcome)? {
            append_in(tx, draft.actor(actor.clone()))?;
        } else if let L1Outcome::Missing {
            sha,
            paths,
            symbols,
        } = outcome
        {
            let short: String = sha.chars().take(7).collect();
            missing.push(format!(
                "bridge-screen: L1 check — {} not found at {short}. Fix the evidence with claim_done.",
                verify::missing_text(paths, symbols)
            ));
        }
    }
    Ok((!missing.is_empty()).then(|| missing.join("\n")))
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

/// Any other note goes out with the decision ask when the session's capture flag is set
/// (after a compaction or a claim): one extra line, once per flag (spec §8.6).
pub fn with_decision_ask(store: &mut Store, session_id: &str, note: String) -> Result<String> {
    let Some(s) = session::get(store.conn(), session_id)? else {
        return Ok(note);
    };
    if s.decision_capture.is_none() {
        return Ok(note);
    }
    store.append(decision::asked_draft(&s.repo_id, &s.session_id, "note"))?;
    Ok(format!("{note}\nbridge-screen: {}", decision::DECISION_ASK))
}

/// At SessionStart (after recovery, before the briefing): drop verifications whose files
/// changed since their commit, in the starting session's worktree.
pub fn staleness(store: &mut Store, repo_id: &str, session_id: &str) -> Result<()> {
    let Some(me) = session::get(store.conn(), session_id)? else {
        return Ok(());
    };
    verify::scan_stale(store, repo_id, session_id, &worktree_of(&me))?;
    Ok(())
}

/// At SessionStart: the briefing (recorded as `briefed`), or the short re-briefing after
/// a compaction.
///
/// `imported` are the plan-import lines to show after the goal line. The briefing ends with
/// the project-pages pointer or, once per session up to three times, the offer to set them
/// up (spec 2026-10-22); a problem with the pages folder is logged and the note left out.
pub fn start_context(
    store: &mut Store,
    input: &HookInput,
    home: &Home,
    imported: &[String],
) -> Result<Option<String>> {
    let Some(me) = session::get(store.conn(), &input.session_id)? else {
        return Ok(None);
    };
    if input.source.as_deref() == Some("compact") {
        let text = handoff::compact_rebriefing(store.conn(), &me)?;
        if me.decision_capture.is_some() {
            store.append(decision::asked_draft(
                &me.repo_id,
                &me.session_id,
                "compact",
            ))?;
        }
        // After a compaction: the pointer only, never the offer.
        return Ok(Some(with_pages_note(home, &me.repo_id, text, true)));
    }
    let b = handoff::briefing(store.conn(), &me)?;
    let data = handoff::Briefed {
        status: b.status,
        readback_required: b.readback_required,
    };
    store.write(|tx| {
        append_in(
            tx,
            EventDraft::new(
                &me.repo_id,
                &me.session_id,
                "briefed",
                serde_json::to_value(data)?,
            ),
        )?;
        // Each unverified task the briefing shows is a nudge (spec §8.5).
        for (task, level) in &b.nudges {
            for draft in verify::nudge_drafts(tx, &me.repo_id, &me.session_id, task, *level)? {
                append_in(tx, draft)?;
            }
        }
        Ok(())
    })?;
    // A brand-new repo ("no plan yet") still gets the offer: it is the best moment for it.
    let onboarding = !imported.is_empty();
    let text = with_imports(&b.text, imported);
    Ok(Some(with_pages_note(home, &me.repo_id, text, onboarding)))
}

/// The briefing plus the project-pages note, if there is room and anything to say.
fn with_pages_note(home: &Home, repo_id: &str, text: String, onboarding: bool) -> String {
    let room = MAX_LINES.saturating_sub(text.lines().count());
    match pages::briefing_note(home, repo_id, room, onboarding) {
        Ok(Some(note)) => format!("{text}\n{note}"),
        Ok(None) => text,
        Err(e) => {
            super::log_error(home, "session-start", &format!("project pages: {e}"));
            text
        }
    }
}

/// The briefing with the import lines after its goal line.
fn with_imports(text: &str, imported: &[String]) -> String {
    if imported.is_empty() {
        return text.to_string();
    }
    let mut lines: Vec<&str> = text.lines().collect();
    let at = lines.len().min(2);
    for (i, line) in imported.iter().enumerate() {
        lines.insert(at + i, line);
    }
    lines.join("\n")
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
                task_id: None,
            };
            append_in(tx, draft("needs_owner", serde_json::to_value(owner)?))?;
            Ok(())
        })?;
        Ok(Some(format!(
            "bridge-screen: WIP commits weren't squashed ({reason}); the PR will show them. Run bridge-screen squash later if you want them collapsed."
        )))
    };
    // Changes inside the range can't be told apart once it is one commit: drop what they
    // made stale first (a failure here never stops the squash).
    let _ = verify::scan_stale(store, &s.repo_id, &s.session_id, &wt);
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
