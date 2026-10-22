//! "Waiting on" (spec §9.1, §11.2): an agent says its session waits on a task — here or in
//! another repo — or on something free-text. A task target clears by itself, in the same
//! transaction as the verification that takes it to L2 or above, and the waiting session
//! gets the same "done on `<branch>` @sha" note plan links get. Free-text waits are
//! cleared by the agent or the owner. A wait never blocks anything and is not a needs-you
//! item: the session isn't blocked on the owner.

pub mod project;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::navigator::{Candidate, Channel, NoteKind, done_text, note_was_sent};
use crate::plan::{EditContext, NodeKind, NodeStatus, PlanOp, get_node, plan_edit_in};
use crate::session::{self, Session};
use crate::store::{Actor, EventDraft, append_in, repo_name};

pub use project::{ClearedWhy, WAIT_KINDS, WaitCleared, WaitingOn, project};

/// Longest reason or free-text target kept.
pub const MAX_TEXT: usize = 200;

/// One wait, as the `waits` view has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wait {
    pub wait_id: String,
    pub repo_id: String,
    pub session_id: String,
    pub target_repo_id: Option<String>,
    pub target_task: Option<String>,
    pub target_text: String,
    pub reason: String,
    pub opened_at: String,
    pub cleared_at: Option<String>,
    pub cleared_why: Option<ClearedWhy>,
    pub cleared_sha: Option<String>,
}

impl Wait {
    pub fn is_open(&self) -> bool {
        self.cleared_at.is_none()
    }

    /// Minutes since it opened, at `now` (0 when the time is unreadable).
    pub fn since_min(&self, now: jiff::Timestamp) -> i64 {
        self.opened_at
            .parse::<jiff::Timestamp>()
            .map_or(0, |t| now.duration_since(t).as_secs() / 60)
            .max(0)
    }
}

const COLUMNS: &str = "wait_id, repo_id, session_id, target_repo_id, target_task, target_text,
                       reason, opened_at, cleared_at, cleared_why, cleared_sha";

fn read(r: &rusqlite::Row) -> rusqlite::Result<Wait> {
    let why: Option<String> = r.get(9)?;
    Ok(Wait {
        wait_id: r.get(0)?,
        repo_id: r.get(1)?,
        session_id: r.get(2)?,
        target_repo_id: r.get(3)?,
        target_task: r.get(4)?,
        target_text: r.get(5)?,
        reason: r.get(6)?,
        opened_at: r.get(7)?,
        cleared_at: r.get(8)?,
        cleared_why: why.as_deref().and_then(ClearedWhy::parse),
        cleared_sha: r.get(10)?,
    })
}

/// Every wait of a session, oldest first.
pub fn of_session(conn: &Connection, session_id: &str) -> Result<Vec<Wait>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM waits WHERE session_id = ?1 ORDER BY opened_at, wait_id"
    ))?;
    let rows = stmt.query_map([session_id], read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A session's open waits.
pub fn open_of(conn: &Connection, session_id: &str) -> Result<Vec<Wait>> {
    Ok(of_session(conn, session_id)?
        .into_iter()
        .filter(Wait::is_open)
        .collect())
}

/// Every wait in a repo (the CLI's `wait list`), open first.
pub fn of_repo(conn: &Connection, repo_id: &str) -> Result<Vec<Wait>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM waits WHERE repo_id = ?1
         ORDER BY cleared_at IS NOT NULL, opened_at, wait_id"
    ))?;
    let rows = stmt.query_map([repo_id], read)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(conn: &Connection, wait_id: &str) -> Result<Option<Wait>> {
    Ok(conn
        .prepare_cached(&format!("SELECT {COLUMNS} FROM waits WHERE wait_id = ?1"))?
        .query_row([wait_id], read)
        .optional()?)
}

/// The wait `id` names: exactly, or (owner CLI) case-insensitively.
pub fn find(conn: &Connection, id: &str) -> Result<Wait> {
    let id = id.trim();
    if let Some(w) = get(conn, id)? {
        return Ok(w);
    }
    get(conn, &id.to_ascii_uppercase())?.ok_or_else(|| Error::NotFound(format!("no wait {id}")))
}

/// What a wait target names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Task { repo_id: String, id: String },
    Text(String),
}

fn is_task_id(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// `2.4` (this repo), `<repo>:<task id>`, or anything else as free text. A task target
/// must exist.
pub fn parse_target(conn: &Connection, repo_id: &str, text: &str) -> Result<Target> {
    let t = text.trim();
    if t.is_empty() {
        return Err(Error::Invalid(
            "say what you are waiting on: a task id like 2.4, <repo>:<task id>, or a few words"
                .into(),
        ));
    }
    let (repo, id) = match t.split_once(':') {
        Some((r, id))
            if is_task_id(id.trim())
                && !r.trim().is_empty()
                && !r.trim().contains(char::is_whitespace) =>
        {
            let repo = crate::plan::resolve_repo(conn, r.trim()).map_err(Error::Invalid)?;
            (repo, id.trim().to_string())
        }
        _ if is_task_id(t) => (repo_id.to_string(), t.to_string()),
        _ => return Ok(Target::Text(t.chars().take(MAX_TEXT).collect())),
    };
    match get_node(conn, &repo, &id)? {
        Some(n) if n.status != NodeStatus::Dropped => {
            if matches!(n.kind, NodeKind::Goal | NodeKind::Milestone) {
                return Err(Error::Invalid(format!(
                    "{id} is a {}; wait on a task",
                    n.kind.as_str()
                )));
            }
            Ok(Target::Task { repo_id: repo, id })
        }
        _ => Err(Error::NotFound(format!(
            "no task {} in that plan",
            shown(conn, repo_id, &repo, &id)?
        ))),
    }
}

/// `2.4` in the same repo, `api:2.4` in another.
pub fn shown(conn: &Connection, here: &str, repo: &str, id: &str) -> Result<String> {
    if repo == here {
        return Ok(id.to_string());
    }
    Ok(format!(
        "{}:{id}",
        repo_name(conn, repo)?.unwrap_or_else(|| repo.to_string())
    ))
}

/// The next wait id: `W1`, `W2`, … across all repos.
fn next_id(conn: &Connection) -> Result<String> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM events WHERE kind = 'waiting_on'",
        [],
        |r| r.get(0),
    )?;
    Ok(format!("W{}", n + 1))
}

/// What `waiting_on` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub wait: Wait,
    /// The target's title and branch: `2.4 Login form (feat/auth)`.
    pub display: String,
    /// A task target that isn't already a plan link of the session's active task.
    pub offer_link: bool,
}

/// Open a wait for `session` (spec §11.2 `waiting_on`). A task already at L2 or above is
/// refused: there's nothing to wait for.
pub fn open(
    tx: &Transaction,
    session: &Session,
    target: &str,
    reason: &str,
    actor: &Actor,
) -> Result<Opened> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(Error::Invalid(
            "give a reason: why you are waiting on it".into(),
        ));
    }
    let parsed = parse_target(tx, &session.repo_id, target)?;
    let wait_id = next_id(tx)?;
    let (data, display, offer_link) = match &parsed {
        Target::Task { repo_id, id } => {
            if let Some(v) =
                crate::verify::top_verification(tx, repo_id, id)?.filter(|v| v.level >= 2)
            {
                let short: String = v.sha.chars().take(7).collect();
                return Err(Error::Invalid(format!(
                    "{} is already done @{short} — merge or rebase to pick it up",
                    shown(tx, &session.repo_id, repo_id, id)?
                )));
            }
            let node = get_node(tx, repo_id, id)?
                .ok_or_else(|| Error::NotFound(format!("no task {id}")))?;
            let label = format!(
                "{} {}",
                shown(tx, &session.repo_id, repo_id, id)?,
                node.title
            );
            let display = match &node.branch {
                Some(b) => format!("{label} ({b})"),
                None => label.clone(),
            };
            let linked = match session.active_task.as_deref() {
                Some(active) => get_node(tx, &session.repo_id, active)?.is_some_and(|n| {
                    if repo_id == &session.repo_id {
                        n.depends_on.contains(id)
                    } else {
                        n.external_deps
                            .iter()
                            .any(|d| &d.repo_id == repo_id && &d.id == id)
                    }
                }),
                None => false,
            };
            (
                WaitingOn {
                    wait_id: wait_id.clone(),
                    target_repo_id: Some(repo_id.clone()),
                    target_task: Some(id.clone()),
                    target_text: label,
                    reason: reason.chars().take(MAX_TEXT).collect(),
                },
                display,
                !linked,
            )
        }
        Target::Text(text) => (
            WaitingOn {
                wait_id: wait_id.clone(),
                target_repo_id: None,
                target_task: None,
                target_text: text.clone(),
                reason: reason.chars().take(MAX_TEXT).collect(),
            },
            text.clone(),
            false,
        ),
    };
    append_in(
        tx,
        EventDraft::new(
            &session.repo_id,
            &session.session_id,
            "waiting_on",
            serde_json::to_value(data)?,
        )
        .actor(actor.clone()),
    )?;
    let wait = get(tx, &wait_id)?.ok_or_else(|| Error::NotFound(wait_id.clone()))?;
    Ok(Opened {
        wait,
        display,
        offer_link,
    })
}

fn cleared_draft(
    w: &Wait,
    why: ClearedWhy,
    sha: Option<String>,
    session_id: &str,
    actor: &Actor,
) -> Result<EventDraft> {
    Ok(EventDraft::new(
        &w.repo_id,
        session_id,
        "wait_cleared",
        serde_json::to_value(WaitCleared {
            wait_id: w.wait_id.clone(),
            why,
            sha,
        })?,
    )
    .actor(actor.clone()))
}

/// Clear a wait by hand: the waiting agent (`by_session` must be the wait's own session)
/// or the owner (`by_session` `None`). An already cleared wait is [`Error::Stale`].
pub fn clear(
    tx: &Transaction,
    wait_id: &str,
    by_session: Option<&str>,
    actor: &Actor,
) -> Result<Wait> {
    let w = find(tx, wait_id)?;
    if let Some(sid) = by_session
        && sid != w.session_id
    {
        return Err(Error::Invalid(format!(
            "{} is another session's wait; only that session (or the owner) can clear it",
            w.wait_id
        )));
    }
    if !w.is_open() {
        return Err(Error::Stale(format!("{} is already cleared", w.wait_id)));
    }
    let (why, session) = match by_session {
        Some(sid) => (ClearedWhy::Agent, sid.to_string()),
        None => (ClearedWhy::Owner, crate::store::OWNER_SESSION.to_string()),
    };
    append_in(tx, cleared_draft(&w, why, None, &session, actor)?)?;
    Ok(w)
}

/// Turn a task wait into a plan link: the waiting session's active task depends on the
/// target (`dep_added`), and the wait clears as `promoted`. Only with the owner's
/// agreement; free-text waits can't be promoted.
pub fn promote(
    tx: &Transaction,
    wait_id: &str,
    by_session: Option<&str>,
    actor: &Actor,
) -> Result<String> {
    let w = find(tx, wait_id)?;
    if let Some(sid) = by_session
        && sid != w.session_id
    {
        return Err(Error::Invalid(format!(
            "{} is another session's wait",
            w.wait_id
        )));
    }
    if !w.is_open() {
        return Err(Error::Stale(format!("{} is already cleared", w.wait_id)));
    }
    let (Some(target_repo), Some(target)) = (&w.target_repo_id, &w.target_task) else {
        return Err(Error::Invalid(format!(
            "{} waits on \"{}\", not a task; only a task can become a plan link",
            w.wait_id, w.target_text
        )));
    };
    let task = session::get(tx, &w.session_id)?
        .and_then(|s| s.active_task)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{}'s session has no active task to link",
                w.wait_id
            ))
        })?;
    let session = by_session.unwrap_or(crate::store::OWNER_SESSION);
    let ctx = EditContext {
        repo_id: w.repo_id.clone(),
        session_id: session.to_string(),
        actor: actor.clone(),
        reason: Some(format!("the wait on {} became a plan link", w.target_text)),
        replan: false,
    };
    plan_edit_in(
        tx,
        &ctx,
        &[PlanOp::AddDep {
            id: task.clone(),
            depends_on: target.clone(),
            repo: (target_repo != &w.repo_id).then(|| target_repo.clone()),
        }],
    )?;
    append_in(
        tx,
        cleared_draft(&w, ClearedWhy::Promoted, None, session, actor)?,
    )?;
    Ok(format!(
        "{task} now depends on {}; the wait {} is closed.",
        shown(tx, &w.repo_id, target_repo, target)?,
        w.wait_id
    ))
}

/// Every open wait (any repo) on `repo_id:task` clears as `target_done` at `sha`. Called in
/// the same transaction right after a `verified` at L2 or above.
pub fn clear_for(
    conn: &Connection,
    repo_id: &str,
    task: &str,
    sha: &str,
) -> Result<Vec<EventDraft>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM waits
         WHERE target_repo_id = ?1 AND target_task = ?2 AND cleared_at IS NULL
         ORDER BY wait_id"
    ))?;
    let open: Vec<Wait> = stmt
        .query_map(params![repo_id, task], read)?
        .collect::<rusqlite::Result<_>>()?;
    open.iter()
        .map(|w| {
            cleared_draft(
                w,
                ClearedWhy::TargetDone,
                Some(sha.to_string()),
                &w.session_id,
                &Actor::default(),
            )
        })
        .collect()
}

/// Append [`clear_for`]'s events inside `tx`.
pub fn clear_for_in(tx: &Transaction, repo_id: &str, task: &str, sha: &str) -> Result<()> {
    for draft in clear_for(tx, repo_id, task, sha)? {
        append_in(tx, draft)?;
    }
    Ok(())
}

/// The note for each of this session's waits whose target was done, not sent yet (key
/// `wait:<wait_id>`). Free-text waits never produce one.
pub fn cleared_notes(conn: &Connection, session: &Session) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for w in of_session(conn, &session.session_id)? {
        if w.cleared_why != Some(ClearedWhy::TargetDone) {
            continue;
        }
        let (Some(repo), Some(task), Some(sha)) =
            (&w.target_repo_id, &w.target_task, &w.cleared_sha)
        else {
            continue;
        };
        let key = format!("wait:{}", w.wait_id);
        // A plan link on the same task already said it (same commit).
        let link = crate::navigator::link_key(&session.repo_id, repo, task, sha);
        if note_was_sent(conn, &session.session_id, &key)?
            || note_was_sent(conn, &session.session_id, &link)?
            || noted_by_wait(conn, session, repo, task, sha)?
        {
            continue;
        }
        let by = crate::verify::top_verification(conn, repo, task)?
            .map(|v| v.session_id)
            .unwrap_or_default();
        let title = crate::claim::task_title(conn, repo, task)?.unwrap_or_default();
        let text = done_text(
            conn,
            &shown(conn, &session.repo_id, repo, task)?,
            &title,
            &by,
            sha,
        )?;
        out.push(Candidate::new(
            NoteKind::DependencyReady,
            Channel::PromptSubmit,
            text,
            key,
        ));
    }
    Ok(out)
}

/// Did one of this session's waits on `repo:task` already get its "done" note for `sha`?
/// (Then the plan link's note for the same task would say it twice.)
pub fn noted_by_wait(
    conn: &Connection,
    session: &Session,
    repo: &str,
    task: &str,
    sha: &str,
) -> Result<bool> {
    for w in of_session(conn, &session.session_id)? {
        if w.cleared_why == Some(ClearedWhy::TargetDone)
            && w.target_repo_id.as_deref() == Some(repo)
            && w.target_task.as_deref() == Some(task)
            && w.cleared_sha.as_deref() == Some(sha)
            && note_was_sent(conn, &session.session_id, &format!("wait:{}", w.wait_id))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `waiting on 2.4 Login form (since 40m)` for each open wait, then
/// `2.4 Login form done @abc1234` for each one cleared by its target since `since`.
pub fn briefing_items(
    conn: &Connection,
    sessions: &[&Session],
    since: Option<&str>,
    now: jiff::Timestamp,
) -> Result<Vec<String>> {
    let mut open = Vec::new();
    let mut done = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for s in sessions {
        for w in of_session(conn, &s.session_id)? {
            if seen.contains(&w.wait_id) {
                continue;
            }
            seen.push(w.wait_id.clone());
            if w.is_open() {
                open.push(format!(
                    "waiting on {} (since {}m)",
                    w.target_text,
                    w.since_min(now)
                ));
            } else if w.cleared_why == Some(ClearedWhy::TargetDone)
                && since.is_none_or(|t| w.cleared_at.as_deref().is_some_and(|c| c > t))
            {
                let short: String = w
                    .cleared_sha
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(7)
                    .collect();
                done.push(format!("{} done @{short}", w.target_text));
            }
        }
    }
    open.extend(done);
    Ok(open)
}

#[cfg(test)]
mod tests;
