//! Recovery (spec §8.2–8.3): how a session ended, crashed sessions, and the leftovers a
//! dead session left in its worktree — found, attributed to tasks, and resolved by adopt,
//! resume or undo.
//!
//! Never lose work: undo is always `git stash push` (untracked files included) plus a
//! snapshot record; nothing is reset, cleaned or checked out. Never touch another live
//! session's worktree: its leftovers are reported only.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::claim::{self, ClaimMode, ClaimRequest, ClaimTarget};
use crate::error::{Error, Result};
use crate::git::{self, StashRef, WorktreeStatus};
use crate::handoff::{
    EndKind, Leftover, LeftoverAction, LeftoverFound, LeftoverResolved, LeftoverState,
    LeftoversAnalysed, ResolvedBy, get_leftover, leftovers,
};
use crate::plan::{NodeStatus, get_node};
use crate::session::{self, Session, SessionCrashed};
use crate::store::{Actor, EventDraft, OWNER_SESSION, Store, append_in};
use crate::tangent::Label;

fn err(msg: impl Into<String>) -> Error {
    Error::Claim(msg.into())
}

/// Clean when nothing is uncommitted and the session's task isn't left half-done
/// (status still `active`, i.e. not claimed done or verified).
pub fn end_kind(conn: &Connection, session: &Session, dirty_files: &[String]) -> Result<EndKind> {
    if !dirty_files.is_empty() {
        return Ok(EndKind::Dirty);
    }
    if let Some(task) = &session.active_task
        && get_node(conn, &session.repo_id, task)?.is_some_and(|n| n.status == NodeStatus::Active)
    {
        return Ok(EndKind::Dirty);
    }
    Ok(EndKind::Clean)
}

/// The session's last activity: its last event, not counting idle notifications.
fn last_activity(conn: &Connection, session_id: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT ts FROM events WHERE session_id = ?1
           AND NOT (kind = 'needs_owner' AND json_extract(data, '$.kind') = 'idle_prompt')
         ORDER BY seq DESC LIMIT 1",
    )?;
    let mut rows = stmt.query([session_id])?;
    Ok(match rows.next()? {
        Some(r) => Some(r.get(0)?),
        None => None,
    })
}

/// Mark every live session of the repo with no activity for `timeout_min` minutes as
/// crashed (`session_crashed`, carrying its last activity time). Idempotent: crashed
/// sessions have ended. Returns the ids marked.
pub fn scan_crashed(
    store: &mut Store,
    repo_id: &str,
    now: Timestamp,
    timeout_min: u32,
) -> Result<Vec<String>> {
    let cutoff = now - SignedDuration::from_mins(i64::from(timeout_min));
    store.write(|tx| {
        let mut marked = Vec::new();
        for s in session::list_live(tx, Some(repo_id))? {
            let Some(last) = last_activity(tx, &s.session_id)? else {
                continue;
            };
            let Ok(at) = last.parse::<Timestamp>() else {
                continue;
            };
            if at >= cutoff {
                continue;
            }
            let data = SessionCrashed {
                last_event_at: at.to_string(),
            };
            append_in(
                tx,
                EventDraft::new(
                    repo_id,
                    &s.session_id,
                    "session_crashed",
                    serde_json::to_value(data)?,
                ),
            )?;
            marked.push(s.session_id);
        }
        Ok(marked)
    })
}

/// Same directory, however it is spelled (`/var` vs `/private/var` on macOS).
pub fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn worktree_path(s: &Session) -> PathBuf {
    PathBuf::from(s.worktree.as_deref().unwrap_or(&s.cwd))
}

/// Dead sessions (dirty or crashed) of the repo whose leftovers no session has analysed
/// yet; those in `first` (the caller's worktree) come first.
fn unanalysed_dead(conn: &Connection, repo_id: &str, first: &Path) -> Result<Vec<Session>> {
    let ids: Vec<String> = conn
        .prepare(
            "SELECT session_id FROM sessions
             WHERE repo_id = ?1 AND state = 'ended' AND end_kind IN ('dirty', 'crashed')
               AND leftovers_analysed = 0
             ORDER BY ended_at, session_id",
        )?
        .query_map([repo_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut dead = Vec::new();
    for id in ids {
        dead.extend(session::get(conn, &id)?);
    }
    dead.sort_by_key(|s| !same_path(&worktree_path(s), first));
    Ok(dead)
}

/// A live session (other than `except`) working in `wt`.
fn live_owner(
    conn: &Connection,
    repo_id: &str,
    wt: &Path,
    except: Option<&str>,
) -> Result<Option<Session>> {
    Ok(session::list_live(conn, Some(repo_id))?
        .into_iter()
        .find(|s| Some(s.session_id.as_str()) != except && same_path(&worktree_path(s), wt)))
}

/// One unit that touched a file: its task, label and detour outcome.
struct Touch {
    task: Option<String>,
    abandoned: bool,
    drift: bool,
}

/// Units that touched `path` in `repo_id`, newest first: from `file_changed` (unit files)
/// and from `tool_intent` of the dead session itself (a crash can cut an edit short).
fn touches(conn: &Connection, repo_id: &str, dead: &str, path: &str) -> Result<Vec<Touch>> {
    let mut stmt = conn.prepare_cached(
        "SELECT u.task_id, u.label, d.outcome FROM units u
         JOIN sessions s ON s.session_id = u.session_id
         LEFT JOIN detours d ON d.detour_id = u.detour_id
         WHERE s.repo_id = ?1 AND (
             EXISTS (SELECT 1 FROM json_each(u.files) WHERE value = ?2)
             OR (u.session_id = ?3 AND EXISTS (
                 SELECT 1 FROM events e, json_each(e.data, '$.files') f
                 WHERE e.session_id = ?3 AND e.kind = 'tool_intent' AND f.value = ?2
                   AND e.ts >= u.started_at AND (u.stopped_at IS NULL OR e.ts <= u.stopped_at))))
         ORDER BY u.started_at DESC, u.unit_no DESC",
    )?;
    let rows = stmt.query_map(params![repo_id, path, dead], |r| {
        let label: Option<String> = r.get(1)?;
        let outcome: Option<String> = r.get(2)?;
        Ok(Touch {
            task: r.get(0)?,
            drift: label.as_deref().and_then(Label::parse) == Some(Label::Drift),
            abandoned: outcome.as_deref() == Some("abandoned"),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The task a leftover file belongs to and what to do with it (spec §8.3):
/// - undo: no unit ever touched it, or only drift units and abandoned detours did;
/// - adopt: its task is claimed done or verified (verify it, then commit), or it has no
///   task to resume;
/// - resume: otherwise (the task is unfinished — continue from its cue).
fn suggest(
    conn: &Connection,
    dead: &Session,
    path: &str,
) -> Result<(Option<String>, LeftoverAction)> {
    let found = touches(conn, &dead.repo_id, &dead.session_id, path)?;
    let task = found
        .iter()
        .find_map(|t| t.task.clone())
        .or_else(|| dead.last_task.clone());
    if found.iter().all(|t| t.drift || t.abandoned) {
        return Ok((task, LeftoverAction::Undo));
    }
    let status = match &task {
        Some(t) => get_node(conn, &dead.repo_id, t)?.map(|n| n.status),
        None => None,
    };
    let action = match status {
        // Work outside any task can't be resumed by claiming it: keep it as a commit.
        None
        | Some(
            NodeStatus::Claimed | NodeStatus::L1 | NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4,
        ) => LeftoverAction::Adopt,
        _ => LeftoverAction::Resume,
    };

    Ok((task, action))
}

/// When an agent may take over a crashed session's leftovers in another worktree.
///
/// A session marked crashed may only be one whose owner stepped away (a long tool run
/// or an unanswered prompt writes no events), so its worktree stays hands-off for agents
/// in other worktrees until it has been silent for `after`; the owner may resolve its
/// leftovers at any time. Leftovers in the caller's own worktree are not affected: the
/// caller is already working on those files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Takeover {
    pub now: Timestamp,
    pub after: SignedDuration,
}

impl Takeover {
    pub fn new(now: Timestamp, hours: u32) -> Self {
        Self {
            now,
            after: SignedDuration::from_hours(i64::from(hours)),
        }
    }
}

/// Is `dead` a crashed session whose worktree `wt` is still hands-off for a caller in
/// `caller_wt` (spec §8.3: never touch a live session's worktree)? Only when `wt` is
/// another worktree and the session hasn't yet been silent for the takeover wait.
fn hands_off(dead: &Session, wt: &Path, caller_wt: Option<&Path>, takeover: &Takeover) -> bool {
    if dead.end_kind != Some(EndKind::Crashed) || dead.state != session::SessionState::Ended {
        return false;
    }
    if caller_wt.is_some_and(|w| same_path(w, wt)) {
        return false;
    }
    // A crashed session's `ended_at` is its last activity.
    match dead.ended_at.as_deref().map(str::parse::<Timestamp>) {
        Some(Ok(last)) => takeover.now < last + takeover.after,
        _ => true,
    }
}

/// What one analysis found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Analysis {
    pub found: Vec<Leftover>,
    /// Dead sessions whose worktree no longer exists: nothing to resolve.
    pub removed: Vec<(String, String)>,
}

/// A short, unique leftover id.
fn new_id(conn: &Connection) -> Result<String> {
    loop {
        let text = ulid::Ulid::generate().to_string().to_lowercase();
        let id = format!("l{}", &text[text.len() - 7..]);
        if get_leftover(conn, &id)?.is_none() {
            return Ok(id);
        }
    }
}

/// Recovery analysis at SessionStart (spec §8.3): for each dead session not yet
/// analysed (this worktree first), record each uncommitted file in its worktree as a
/// `leftover_found` with a task and a suggestion, then `leftovers_analysed`. Files in a
/// live session's worktree are report-only. Git runs before the write transaction.
pub fn analyse(
    store: &mut Store,
    repo_id: &str,
    me: &str,
    my_worktree: &Path,
    takeover: &Takeover,
) -> Result<Analysis> {
    let dead = unanalysed_dead(store.conn(), repo_id, my_worktree)?;
    if dead.is_empty() {
        return Ok(Analysis::default());
    }
    // Read every worktree first; `None` = the worktree is gone.
    let mut scans: Vec<(Session, Option<WorktreeStatus>)> = Vec::new();
    for d in dead {
        let wt = worktree_path(&d);
        if !wt.is_dir() {
            scans.push((d, None));
            continue;
        }
        // Can't read it now (not a repo any more?): leave it for a later start.
        if let Ok(st) = git::status(&wt) {
            scans.push((d, Some(st)));
        }
    }
    store.write(|tx| {
        let mut out = Analysis::default();
        for (d, status) in scans {
            let fresh = session::get(tx, &d.session_id)?.is_some_and(|s| !s.leftovers_analysed);
            if !fresh {
                continue;
            }
            let wt = worktree_path(&d);
            let wt_text = wt.to_string_lossy().into_owned();
            match status {
                None => out.removed.push((d.session_id.clone(), wt_text.clone())),
                Some(st) => {
                    let report_only = live_owner(tx, repo_id, &wt, Some(me))?.is_some()
                        || hands_off(&d, &wt, Some(my_worktree), takeover);
                    let known: HashSet<String> = leftovers(tx, repo_id, true)?
                        .into_iter()
                        .filter(|l| same_path(Path::new(&l.worktree), &wt))
                        .map(|l| l.path)
                        .collect();
                    for path in st.dirty_paths() {
                        if known.contains(&path) {
                            continue;
                        }
                        let (task, suggested) = suggest(tx, &d, &path)?;
                        let leftover_id = new_id(tx)?;
                        let data = LeftoverFound {
                            leftover_id: leftover_id.clone(),
                            from_session: d.session_id.clone(),
                            worktree: wt_text.clone(),
                            path,
                            task_id: task.clone(),
                            suggested,
                            report_only,
                        };
                        let mut draft = EventDraft::new(
                            repo_id,
                            me,
                            "leftover_found",
                            serde_json::to_value(data)?,
                        );
                        if let Some(t) = task {
                            draft = draft.task(t);
                        }
                        append_in(tx, draft)?;
                        out.found.extend(get_leftover(tx, &leftover_id)?);
                    }
                }
            }
            let data = LeftoversAnalysed {
                from_session: d.session_id.clone(),
            };
            append_in(
                tx,
                EventDraft::new(
                    repo_id,
                    me,
                    "leftovers_analysed",
                    serde_json::to_value(data)?,
                ),
            )?;
        }
        Ok(out)
    })
}

/// Who resolves leftovers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// An agent session (MCP `resolve_leftover`).
    Agent { session_id: String },
    /// The owner (CLI or dashboard).
    Owner,
}

/// What a resolution did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolution {
    pub ids: Vec<String>,
    pub action: LeftoverAction,
    pub worktree: String,
    /// adopt: the commit made.
    pub commit: Option<String>,
    /// undo: the stash holding the files.
    pub stash: Option<StashRef>,
    /// resume: the task claimed.
    pub claimed: Option<String>,
}

/// Leftovers by id or unique id prefix, in the given repo.
pub fn find(conn: &Connection, repo_id: &str, which: &[String]) -> Result<Vec<Leftover>> {
    let all = leftovers(conn, repo_id, false)?;
    let mut out: Vec<Leftover> = Vec::new();
    for w in which {
        let hits: Vec<&Leftover> = all
            .iter()
            .filter(|l| l.leftover_id == *w || l.leftover_id.starts_with(w.as_str()))
            .collect();
        match hits.as_slice() {
            [one] => {
                if !out.iter().any(|l| l.leftover_id == one.leftover_id) {
                    out.push((*one).clone());
                }
            }
            [] => return Err(err(format!("no leftover {w} in this repo"))),
            _ => return Err(err(format!("leftover id {w} is ambiguous"))),
        }
    }
    if out.is_empty() {
        return Err(err("give at least one leftover id"));
    }
    Ok(out)
}

/// The snapshot written for an undone leftover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Snapshot<'a> {
    leftover_id: &'a str,
    stash: &'a str,
    stash_message: &'a str,
    worktree: &'a str,
    paths: &'a [String],
    from_session: &'a str,
    ts: String,
}

/// Refusals that leave everything untouched:
/// - already resolved (the first resolution, and an owner's above all, stands);
/// - leftovers of more than one worktree;
/// - an agent resuming work that lives in another worktree;
/// - a worktree where another live session works: report-only leftovers are never
///   touched, and an agent never touches another live session's worktree (the owner may
///   still decide on leftovers that aren't report-only);
/// - an agent acting on a crashed session's files in another worktree before the takeover
///   wait: that session may only have an absent owner (the owner may decide at once).
fn check(
    conn: &Connection,
    items: &[Leftover],
    action: LeftoverAction,
    caller: &Caller,
    caller_worktree: Option<&Path>,
    takeover: &Takeover,
) -> Result<()> {
    for l in items {
        if l.state == LeftoverState::Resolved {
            return Err(err(match l.resolved_by {
                Some(ResolvedBy::Owner) => {
                    format!(
                        "leftover {} was already resolved by the owner",
                        l.leftover_id
                    )
                }
                _ => format!("leftover {} was already resolved", l.leftover_id),
            }));
        }
    }
    let wt = &items[0].worktree;
    if items.iter().any(|l| l.worktree != *wt) {
        return Err(err("resolve leftovers of one worktree at a time"));
    }
    let me = match caller {
        Caller::Agent { session_id } => Some(session_id.as_str()),
        Caller::Owner => None,
    };
    let live_here = |me: Option<&str>| live_owner(conn, &items[0].repo_id, Path::new(wt), me);
    let refuse = |owner: Session| {
        err(format!(
            "{wt} belongs to live session {}; its files are report-only and were left untouched",
            owner.session_id
        ))
    };
    if items.iter().any(|l| l.state == LeftoverState::ReportOnly)
        && let Some(owner) = live_here(me)?
    {
        return Err(refuse(owner));
    }
    if me.is_some()
        && action == LeftoverAction::Resume
        && !caller_worktree.is_some_and(|w| same_path(w, Path::new(wt)))
    {
        return Err(err(format!(
            "resume it from {wt}: these files are in that worktree"
        )));
    }
    if me.is_some()
        && let Some(owner) = live_here(me)?
    {
        return Err(refuse(owner));
    }
    if me.is_some() {
        for l in items {
            if let Some(dead) = session::get(conn, &l.from_session)?
                && hands_off(&dead, Path::new(wt), caller_worktree, takeover)
            {
                return Err(err(format!(
                    "{wt} belongs to session {}, marked crashed but possibly only waiting for its owner; its files stay report-only for agents until it has been silent {} h. The owner can resolve them now with bridge-screen resolve",
                    dead.session_id,
                    takeover.after.as_hours()
                )));
            }
        }
    }
    Ok(())
}

/// Resolve leftovers (spec §8.3; MCP `resolve_leftover`, CLI `resolve`):
/// - adopt: commit exactly their files as `wip(bridge): adopt leftovers from <session>`;
/// - resume: the calling agent claims their task — only from their own worktree;
/// - undo: stash exactly their files (untracked included) and write a snapshot record to
///   `snapshots/<leftover_id>.json`; `git stash apply <sha>` brings them back.
///
/// Refused, with nothing touched, when a leftover is already resolved (an owner's choice
/// stands) or its worktree belongs to another live session.
pub fn resolve(
    store: &mut Store,
    repo_id: &str,
    snapshots: &Path,
    which: &[String],
    action: LeftoverAction,
    caller: &Caller,
    takeover: &Takeover,
) -> Result<Resolution> {
    let items = find(store.conn(), repo_id, which)?;
    let caller_session = match caller {
        Caller::Agent { session_id } => Some(
            session::get(store.conn(), session_id)?
                .ok_or_else(|| err(format!("unknown session {session_id}")))?,
        ),
        Caller::Owner => None,
    };
    let caller_worktree = caller_session.as_ref().map(worktree_path);
    check(
        store.conn(),
        &items,
        action,
        caller,
        caller_worktree.as_deref(),
        takeover,
    )?;
    let wt_text = items[0].worktree.clone();
    let wt = PathBuf::from(&wt_text);
    let ids: Vec<String> = items.iter().map(|l| l.leftover_id.clone()).collect();
    let mut out = Resolution {
        ids,
        action,
        worktree: wt_text.clone(),
        commit: None,
        stash: None,
        claimed: None,
    };
    let task = items.iter().find_map(|l| l.task_id.clone());
    if action == LeftoverAction::Resume && caller_session.is_some() && task.is_none() {
        return Err(err(
            "these leftovers belong to no task; claim one with claim_task, or adopt or undo them",
        ));
    }
    act(&wt, &items, task.as_deref(), snapshots, &mut out)?;
    let by = match caller {
        Caller::Agent { .. } => ResolvedBy::Agent,
        Caller::Owner => ResolvedBy::Owner,
    };
    let session_id = caller_session
        .as_ref()
        .map_or(OWNER_SESSION.to_string(), |s| s.session_id.clone());
    store.write(|tx| {
        if action == LeftoverAction::Resume
            && let (Some(s), Some(t)) = (&caller_session, &task)
            && s.active_task.as_deref() != Some(t.as_str())
        {
            claim::claim_in_with_reason(
                tx,
                &ClaimRequest {
                    session_id: s.session_id.clone(),
                    repo_id: repo_id.to_string(),
                    task: ClaimTarget::Existing(t.clone()),
                    mode: ClaimMode::Confirmed,
                    confidence: 1.0,
                    actor: Actor::default(),
                },
                "resumed from leftovers",
            )?;
        }
        if action == LeftoverAction::Resume && caller_session.is_some() {
            out.claimed.clone_from(&task);
        }
        for l in &items {
            let data = LeftoverResolved {
                leftover_id: l.leftover_id.clone(),
                action,
                stash_ref: out.stash.as_ref().map(|s| s.sha.clone()),
                commit: out.commit.clone(),
                by,
            };
            let mut draft = EventDraft::new(
                repo_id,
                &session_id,
                "leftover_resolved",
                serde_json::to_value(data)?,
            );
            if let Some(t) = &l.task_id {
                draft = draft.task(t);
            }
            append_in(tx, draft)?;
        }
        Ok(())
    })?;
    Ok(out)
}

/// The git side of a resolution: commit (adopt) or stash (undo) exactly the leftovers'
/// paths that are still uncommitted; the rest are resolved as they are.
fn act(
    wt: &Path,
    items: &[Leftover],
    task: Option<&str>,
    snapshots: &Path,
    out: &mut Resolution,
) -> Result<()> {
    let dirty: HashSet<String> = if wt.is_dir() {
        git::status(wt)?.dirty_paths().into_iter().collect()
    } else {
        HashSet::new()
    };
    let paths: Vec<String> = items
        .iter()
        .map(|l| l.path.clone())
        .filter(|p| dirty.contains(p))
        .collect();
    if paths.is_empty() {
        return Ok(());
    }
    match out.action {
        LeftoverAction::Adopt => {
            let from: String = items[0].from_session.chars().take(8).collect();
            let trailers: Vec<(String, String)> = task
                .iter()
                .map(|t| {
                    (
                        crate::checkpoint::TASK_TRAILER.to_string(),
                        (*t).to_string(),
                    )
                })
                .collect();
            let subject = format!(
                "{} adopt leftovers from {from}",
                crate::checkpoint::WIP_PREFIX
            );
            out.commit = Some(git::commit_paths(wt, &paths, &subject, &trailers)?);
        }
        LeftoverAction::Undo => {
            let message = format!("bridge-screen undo {}", out.ids.join(" "));
            let stash = git::stash_paths(wt, &paths, &message)?;
            write_snapshots(snapshots, items, &paths, &stash)?;
            out.stash = Some(stash);
        }
        LeftoverAction::Resume => {}
    }
    Ok(())
}

fn write_snapshots(
    dir: &Path,
    items: &[Leftover],
    paths: &[String],
    stash: &StashRef,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for l in items {
        let snap = Snapshot {
            leftover_id: &l.leftover_id,
            stash: &stash.sha,
            stash_message: &stash.message,
            worktree: &l.worktree,
            paths,
            from_session: &l.from_session,
            ts: Timestamp::now().to_string(),
        };
        std::fs::write(
            dir.join(format!("{}.json", l.leftover_id)),
            serde_json::to_string_pretty(&snap)?,
        )?;
    }
    Ok(())
}

/// Open leftovers of a repo that a session may act on, and the report-only ones.
pub fn open_counts(conn: &Connection, repo_id: &str) -> Result<(usize, usize)> {
    let open = leftovers(conn, repo_id, true)?;
    let report = open
        .iter()
        .filter(|l| l.state == LeftoverState::ReportOnly)
        .count();
    Ok((open.len(), report))
}

#[cfg(test)]
mod tests;
