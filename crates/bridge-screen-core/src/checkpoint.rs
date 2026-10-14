//! Checkpoint commits at task boundaries (spec §8.1).
//!
//! A boundary — a task claimed done, a detour closed, a switch to another task — makes a
//! checkpoint due (`checkpoint_due`, in the same transaction as the boundary). At Stop, a
//! dirty worktree gets one request to commit (the highest-priority Stop continuation); a
//! second Stop in the same unit without the commit is recorded as `checkpoint_missed`
//! instead of asking again. Commits are detected after a Bash `git commit` and at Stop.

use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::claim::task_title;
use crate::error::{Error, Result};
use crate::git::HeadCommit;
use crate::handoff::{
    CheckpointDue, CheckpointMissed, CheckpointReason, CheckpointRecorded, ClaimedDone,
};
use crate::navigator::{Candidate, Channel, NoteKind, Step, note_sent_draft};
use crate::plan::{NodeKind, NodeStatus, get_node};
use crate::session::{self, Session, SessionState};
use crate::store::{Actor, EventDraft, Store, append_in};

/// Subject prefix of checkpoint commits.
pub const WIP_PREFIX: &str = "wip(bridge):";

/// Trailer naming the task a checkpoint commit belongs to.
pub const TASK_TRAILER: &str = "Bridge-Task";
/// Trailer with the one-line next action (the resume cue a crash can't destroy).
pub const NEXT_TRAILER: &str = "Bridge-Next";
pub const SESSION_TRAILER: &str = "Bridge-Session";

fn err(msg: impl Into<String>) -> Error {
    Error::Claim(msg.into())
}

/// The `checkpoint_due` event for a session.
pub fn due_draft(session: &Session, reason: CheckpointReason, actor: &Actor) -> Result<EventDraft> {
    Ok(EventDraft::new(
        &session.repo_id,
        &session.session_id,
        "checkpoint_due",
        serde_json::to_value(CheckpointDue { reason })?,
    )
    .actor(actor.clone()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimDoneRequest {
    pub session_id: String,
    /// Defaults to the session's active task.
    pub task_id: Option<String>,
    /// What shows the task is done (tests run, files changed).
    pub evidence: String,
    pub actor: Actor,
}

/// The agent says a task is done (L0, spec §8.5): `claimed_done` plus `checkpoint_due`,
/// in one transaction. Returns the task id.
pub fn claim_done(store: &mut Store, req: &ClaimDoneRequest) -> Result<String> {
    store.write(|tx| claim_done_in(tx, req))
}

fn claim_done_in(tx: &Transaction, req: &ClaimDoneRequest) -> Result<String> {
    let session = session::get(tx, &req.session_id)?
        .ok_or_else(|| err(format!("unknown session {}", req.session_id)))?;
    if session.state == SessionState::Ended {
        return Err(err(format!("session {} has ended", req.session_id)));
    }
    let task = req
        .task_id
        .clone()
        .or_else(|| session.active_task.clone())
        .ok_or_else(|| err("no active task; give the id of the task you finished"))?;
    let node = get_node(tx, &session.repo_id, &task)?
        .ok_or_else(|| err(format!("no task {task} in this plan")))?;
    if node.status == NodeStatus::Dropped {
        return Err(err(format!("{task} was dropped")));
    }
    if matches!(node.kind, NodeKind::Goal | NodeKind::Milestone) {
        return Err(err(format!(
            "{task} is a {}; claim a task or subtask done",
            node.kind.as_str()
        )));
    }
    let evidence = req.evidence.trim();
    if evidence.is_empty() {
        return Err(err("evidence is required: what shows the task is done"));
    }
    let data = ClaimedDone {
        task_id: task.clone(),
        evidence: evidence.chars().take(500).collect(),
    };
    append_in(
        tx,
        EventDraft::new(
            &session.repo_id,
            &session.session_id,
            "claimed_done",
            serde_json::to_value(data)?,
        )
        .actor(req.actor.clone())
        .task(&task),
    )?;
    append_in(
        tx,
        due_draft(&session, CheckpointReason::ClaimedDone, &req.actor)?,
    )?;
    Ok(task)
}

/// Does this shell command look like it makes a commit?
pub fn is_commit_command(command: &str) -> bool {
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().enumerate().any(|(i, w)| {
        (*w == "git" || w.ends_with("/git"))
            && words[i + 1..].iter().take(6).any(|next| *next == "commit")
    })
}

/// When the session's pending checkpoint became due (its latest `checkpoint_due`).
fn pending_since(conn: &Connection, session_id: &str) -> Result<Option<jiff::Timestamp>> {
    let ts: Option<String> = conn
        .prepare_cached(
            "SELECT ts FROM events WHERE session_id = ?1 AND kind = 'checkpoint_due'
             ORDER BY seq DESC LIMIT 1",
        )?
        .query_row([session_id], |r| r.get(0))
        .optional()?;
    Ok(ts.and_then(|t| t.parse().ok()))
}

fn trailer<'a>(head: &'a HeadCommit, key: &str) -> Option<&'a str> {
    head.trailers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

/// The `checkpoint` event for HEAD, if HEAD is a new checkpoint for this session:
/// - not the session's last checkpoint and not recorded before;
/// - made during the session;
/// - a checkpoint commit (`wip(bridge):` subject or a `Bridge-Task` trailer), or any
///   commit made while a checkpoint is pending.
pub fn detect(
    conn: &Connection,
    session: &Session,
    head: &HeadCommit,
) -> Result<Option<EventDraft>> {
    if session.last_checkpoint.as_deref() == Some(head.sha.as_str()) {
        return Ok(None);
    }
    let known: Option<i64> = conn
        .prepare_cached("SELECT 1 FROM checkpoints WHERE sha = ?1")?
        .query_row([&head.sha], |r| r.get(0))
        .optional()?;
    if known.is_some() {
        return Ok(None);
    }
    // Commit times are whole seconds; event times are not.
    let made_after = |t: jiff::Timestamp| head.time >= t.as_second();
    let started = session.started_at.parse::<jiff::Timestamp>().ok();
    if started.is_some_and(|t| !made_after(t)) {
        return Ok(None);
    }
    let bridge = head.subject.starts_with(WIP_PREFIX) || trailer(head, TASK_TRAILER).is_some();
    let pending = session.checkpoint_pending.is_some()
        // Strictly after: a commit in the same second as the boundary may predate it.
        && pending_since(conn, &session.session_id)?
            .is_none_or(|t| head.time > t.as_second());
    if !bridge && !pending {
        return Ok(None);
    }
    let task = trailer(head, TASK_TRAILER)
        .map(str::to_string)
        .or_else(|| session.active_task.clone())
        .or_else(|| session.last_task.clone());
    let data = CheckpointRecorded {
        sha: head.sha.clone(),
        task_id: task.clone(),
        next_action: trailer(head, NEXT_TRAILER).map(str::to_string),
    };
    let draft = EventDraft::new(
        &session.repo_id,
        &session.session_id,
        "checkpoint",
        serde_json::to_value(data)?,
    );
    Ok(Some(match task {
        Some(t) => draft.task(t),
        None => draft,
    }))
}

/// What Stop knows from git when a checkpoint is pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopGit {
    pub dirty: bool,
    pub head: Option<String>,
}

/// The request sent at Stop.
pub fn request_text(conn: &Connection, session: &Session) -> Result<String> {
    let task = session
        .active_task
        .clone()
        .or_else(|| session.last_task.clone());
    let what = match &task {
        Some(id) => {
            let title = task_title(conn, &session.repo_id, id)?.unwrap_or_default();
            format!("{id} {title}").trim_end().to_string()
        }
        None => "<task id> <title>".to_string(),
    };
    let id = task.as_deref().unwrap_or("<task id>");
    Ok(format!(
        "bridge-screen: checkpoint before stopping — commit as `{WIP_PREFIX} {what}` with trailers {TASK_TRAILER}: {id}, {NEXT_TRAILER}: <one-line next action>, {SESSION_TRAILER}: {}.",
        session.session_id
    ))
}

/// The checkpoint part of a Stop (after commit detection), for unit `unit_no`:
/// - nothing pending → nothing;
/// - clean worktree → `checkpoint` at HEAD, clearing it silently;
/// - dirty, not yet asked in this unit and not `stop_hook_active` → the request, as a
///   Stop note (it outranks every other note: one note per unit means the label request
///   falls back to the heuristic);
/// - dirty otherwise → `checkpoint_missed`, once per unit.
pub fn stop_step(
    conn: &Connection,
    session_id: &str,
    unit_no: i64,
    stop_hook_active: bool,
    git: &StopGit,
) -> Result<Step> {
    let mut step = Step::default();
    let Some(s) = session::get(conn, session_id)? else {
        return Ok(step);
    };
    if s.checkpoint_pending.is_none() || s.state == SessionState::Ended {
        return Ok(step);
    }
    let draft = |kind: &str, data: serde_json::Value| {
        EventDraft::new(&s.repo_id, &s.session_id, kind, data)
    };
    if !git.dirty {
        if let Some(head) = &git.head {
            let task = s.active_task.clone().or_else(|| s.last_task.clone());
            let data = CheckpointRecorded {
                sha: head.clone(),
                task_id: task,
                next_action: None,
            };
            step.events
                .push(draft("checkpoint", serde_json::to_value(data)?));
        }
        return Ok(step);
    }
    if stop_hook_active || s.checkpoint_asked_unit == Some(unit_no) {
        if s.checkpoint_missed != unit_no {
            step.events.push(draft(
                "checkpoint_missed",
                serde_json::to_value(CheckpointMissed { unit_no })?,
            ));
        }
        return Ok(step);
    }
    let note = Candidate::new(
        NoteKind::Checkpoint,
        Channel::Stop,
        request_text(conn, &s)?,
        format!("checkpoint:{unit_no}"),
    );
    step.events
        .push(note_sent_draft(&s.repo_id, &s.session_id, unit_no, &note)?);
    step.note = Some(note.text);
    Ok(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        for (id, parent, kind) in [
            ("G", None, "goal"),
            ("1", Some("G"), "milestone"),
            ("1.1", Some("1"), "task"),
        ] {
            s.append(EventDraft::new(
                "r",
                "owner",
                "node_added",
                json!({"id": id, "parent_id": parent, "kind": kind, "title": format!("T{id}"),
                       "intent": "why", "expected_files": []}),
            ))
            .unwrap();
        }
        s.append(EventDraft::new(
            "r",
            "s1",
            "session_started",
            json!({"cwd": "/w", "source": "startup"}),
        ))
        .unwrap();
        s
    }

    fn req(task: Option<&str>) -> ClaimDoneRequest {
        ClaimDoneRequest {
            session_id: "s1".into(),
            task_id: task.map(str::to_string),
            evidence: "tests pass".into(),
            actor: Actor::default(),
        }
    }

    #[test]
    fn claim_done_needs_a_real_task() {
        let mut s = store();
        let msg = |r: Result<String>| r.unwrap_err().to_string();
        assert_eq!(
            msg(claim_done(&mut s, &req(None))),
            "no active task; give the id of the task you finished"
        );
        assert_eq!(
            msg(claim_done(&mut s, &req(Some("1")))),
            "1 is a milestone; claim a task or subtask done"
        );
        assert_eq!(
            msg(claim_done(&mut s, &req(Some("9")))),
            "no task 9 in this plan"
        );
        let mut empty = req(Some("1.1"));
        empty.evidence = "  ".into();
        assert!(msg(claim_done(&mut s, &empty)).starts_with("evidence is required"));
        assert_eq!(claim_done(&mut s, &req(Some("1.1"))).unwrap(), "1.1");
        let kinds: Vec<String> = crate::store::read_all(s.conn(), None)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .skip(4)
            .collect();
        assert_eq!(kinds, vec!["claimed_done", "checkpoint_due"]);
    }

    #[test]
    fn commit_commands() {
        for yes in [
            "git commit -m x",
            "git add -A && git commit -q -m 'wip(bridge): 1.1 x'",
            "git -C /x commit --amend",
            "cd a; /usr/bin/git commit",
        ] {
            assert!(is_commit_command(yes), "{yes}");
        }
        for no in [
            "git status",
            "cargo test",
            "echo commit",
            "git log --grep commit-ish",
        ] {
            assert!(!is_commit_command(no), "{no}");
        }
    }

    fn head(sha: &str, time: i64, subject: &str, trailers: &[(&str, &str)]) -> HeadCommit {
        HeadCommit {
            sha: sha.into(),
            time,
            subject: subject.into(),
            trailers: trailers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    #[test]
    fn detection_rules() {
        let s = store();
        let session = session::get(s.conn(), "s1").unwrap().unwrap();
        let started = session
            .started_at
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_second();
        // A plain commit with nothing pending isn't a checkpoint.
        assert_eq!(
            detect(s.conn(), &session, &head("a", started + 5, "fix", &[])).unwrap(),
            None
        );
        // A wip commit is.
        let d = detect(
            s.conn(),
            &session,
            &head(
                "b",
                started + 5,
                "x",
                &[("Bridge-Task", "1.1"), ("Bridge-Next", "go on")],
            ),
        )
        .unwrap()
        .unwrap();
        assert_eq!(d.data["task_id"], "1.1");
        assert_eq!(d.data["next_action"], "go on");
        // One made before the session started isn't this session's.
        assert_eq!(
            detect(
                s.conn(),
                &session,
                &head("c", started - 100, "wip(bridge): 1.1", &[])
            )
            .unwrap(),
            None
        );
        // While pending, any new commit counts.
        let mut pending = session.clone();
        pending.checkpoint_pending = Some("claimed_done".into());
        assert!(
            detect(s.conn(), &pending, &head("d", started + 5, "fix", &[]))
                .unwrap()
                .is_some()
        );
        // The last checkpoint isn't recorded twice.
        pending.last_checkpoint = Some("d".into());
        assert_eq!(
            detect(s.conn(), &pending, &head("d", started + 5, "fix", &[])).unwrap(),
            None
        );
    }

    /// A plain commit in the same second the checkpoint became due may be the commit
    /// from before the boundary (HEAD unchanged): it must not clear the request. A
    /// real checkpoint commit in that second is caught at Stop by the clean worktree.
    #[test]
    fn a_plain_commit_in_the_due_second_is_not_the_checkpoint() {
        let mut s = store();
        s.append(EventDraft::new(
            "r",
            "s1",
            "task_claimed",
            json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        ))
        .unwrap();
        claim_done(&mut s, &req(None)).unwrap();
        let session = session::get(s.conn(), "s1").unwrap().unwrap();
        assert!(session.checkpoint_pending.is_some());
        let due = pending_since(s.conn(), "s1").unwrap().unwrap().as_second();
        let started = session
            .started_at
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_second();
        assert!(due >= started);
        assert_eq!(
            detect(s.conn(), &session, &head("old", due, "init", &[])).unwrap(),
            None
        );
        assert!(
            detect(s.conn(), &session, &head("new", due + 1, "fix", &[]))
                .unwrap()
                .is_some()
        );
    }
}
