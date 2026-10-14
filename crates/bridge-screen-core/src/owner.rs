//! Owner actions (spec §10.1–10.5): every click on a needs-you item — on the dashboard,
//! in `watch`, on the desktop card — and every owner CLI command goes through [`act`], so
//! each records the same events wherever it came from.
//!
//! Each action first checks, inside its write transaction, that the thing it answers is
//! still open; a stale or double click is refused with [`Error::Stale`] and records
//! nothing.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, Transaction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::claim;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::handoff::{LeftoverAction, LeftoverState, ResolvedBy};
use crate::navigator::{self, Channel};
use crate::paths::Home;
use crate::plan::{NodeStatus, get_node};
use crate::recovery::{self, Caller, Resolution};
use crate::session::{self, Session};
use crate::store::{Actor, EventDraft, OWNER_SESSION, Store, append_in, repo_name};
use crate::tangent::{self, Label, LabelSource, NavState};
use crate::verify;

/// The owner's answer to a drift question (spec §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriftAnswer {
    /// Keep going: the away time so far is acknowledged.
    Continue,
    /// Promote the current detour to a task (needs `parent` and `title`).
    #[serde(alias = "make-task")]
    MakeTask,
    /// Stop the detour and return to the task.
    Stop,
}

impl DriftAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::MakeTask => "make_task",
            Self::Stop => "stop",
        }
    }
}

fn level_four() -> i64 {
    4
}

/// One owner action. The JSON form (`{"kind": "drift", …}`) is what the screens send
/// back: each needs-you item carries the exact actions it offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnerAction {
    /// Answer a drift question about a live session (id or unique prefix).
    Drift {
        session: String,
        answer: DriftAnswer,
        /// make_task: the new task's parent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// make_task: the new task's title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// make_task: estimate in agent-minutes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        estimate: Option<u32>,
        /// continue: why, in the owner's words (optional; at least 8 characters and 2
        /// words when given).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Set a unit's label (overrides the agent and the heuristic).
    Label {
        session: String,
        unit_no: i64,
        label: Label,
    },
    /// Resolve leftovers of a dead session.
    Resolve {
        /// The repo; found from the leftovers when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
        leftover_ids: Vec<String>,
        action: LeftoverAction,
    },
    /// It works: the owner's own check (L4 by default).
    Verify {
        repo: String,
        task: String,
        #[serde(default = "level_four")]
        level: i64,
    },
    /// It doesn't work: back to failed, with the owner's note.
    Broken {
        repo: String,
        task: String,
        note: String,
    },
    /// Put the repo's auto-tuned drift thresholds back to the defaults.
    ResetDrift { repo: String },
}

/// Where an owner action runs.
#[derive(Debug, Clone, Copy)]
pub struct ActCtx<'a> {
    pub home: &'a Home,
    pub cfg: &'a Config,
    pub now: Timestamp,
    /// The CLI's `-C` directory: the worktree whose HEAD a verdict is tied to. Screens
    /// leave it out; the task's worktree (or the repo root) is used.
    pub dir: Option<&'a Path>,
}

/// Longest reason kept, in characters.
pub const MAX_REASON: usize = 200;

/// Words that say nothing on their own.
const FILLER: &[&str] = &["ok", "okay", "fine", "yes", "sure", "go", "continue", "idk"];

const REASON_HINT: &str = "a reason needs at least 8 characters and 2 words saying why (or leave it empty); nothing was recorded";

/// An optional reason, checked: `None` when empty; refused when it says nothing.
pub fn check_reason(reason: Option<&str>) -> Result<Option<String>> {
    let Some(text) = reason.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if text.chars().count() > MAX_REASON {
        return Err(Error::Invalid(format!(
            "the reason is longer than {MAX_REASON} characters; nothing was recorded"
        )));
    }
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let says_something = words.iter().any(|w| !FILLER.contains(&w.as_str()));
    if text.chars().count() < 8 || words.len() < 2 || !says_something {
        return Err(Error::Invalid(REASON_HINT.into()));
    }
    Ok(Some(text.to_string()))
}

fn stale(msg: impl Into<String>) -> Error {
    Error::Stale(msg.into())
}

/// The live session whose id is `prefix` or starts with it.
pub fn find_session(conn: &Connection, prefix: &str) -> Result<Session> {
    let found: Vec<Session> = session::list_live(conn, None)?
        .into_iter()
        .filter(|s| s.session_id.starts_with(prefix))
        .collect();
    match found.len() {
        0 => Err(Error::Claim(format!(
            "no live session starts with {prefix}"
        ))),
        1 => Ok(found.into_iter().next().expect("one session")),
        _ => {
            let ids: Vec<&str> = found.iter().map(|s| s.session_id.as_str()).collect();
            Err(Error::Claim(format!(
                "session prefix {prefix} is ambiguous: {}",
                ids.join(", ")
            )))
        }
    }
}

/// Record an `owner_action` (session `owner`).
fn record(tx: &Transaction, repo_id: &str, data: &tangent::OwnerAction) -> Result<()> {
    append_in(
        tx,
        EventDraft::new(
            repo_id,
            OWNER_SESSION,
            "owner_action",
            serde_json::to_value(data)?,
        ),
    )?;
    Ok(())
}

fn owner_data(action: &str, answer: Option<&str>, target: Option<String>) -> tangent::OwnerAction {
    tangent::OwnerAction {
        action: action.into(),
        answer: answer.map(Into::into),
        target,
        reason: None,
    }
}

/// Re-check the session's tolerance band after an owner action (state events only).
fn recheck(tx: &Transaction, cfg: &Config, session_id: &str, now: Timestamp) -> Result<()> {
    let Some(s) = session::get(tx, session_id)? else {
        return Ok(());
    };
    let th = tangent::thresholds(tx, &s.repo_id, cfg)?;
    let step = navigator::nav_step(tx, session_id, Channel::ToolResult, &th, now)?;
    for draft in step.events {
        append_in(tx, draft)?;
    }
    Ok(())
}

/// Perform one owner action and return the line to show. A stale action returns
/// [`Error::Stale`] and records nothing.
pub fn act(store: &mut Store, ctx: &ActCtx, action: &OwnerAction) -> Result<String> {
    match action {
        OwnerAction::Drift {
            session,
            answer,
            parent,
            title,
            estimate,
            reason,
        } => {
            let nonempty = |v: &Option<String>| v.clone().filter(|t| !t.trim().is_empty());
            let task = nonempty(parent).zip(nonempty(title));
            drift(
                store,
                ctx,
                session,
                *answer,
                task.as_ref(),
                *estimate,
                reason.as_deref(),
            )
        }
        OwnerAction::Label {
            session,
            unit_no,
            label,
        } => label_unit(store, ctx, session, *unit_no, *label),
        OwnerAction::Resolve {
            repo,
            leftover_ids,
            action,
        } => resolve(store, ctx, repo.as_deref(), leftover_ids, *action),
        OwnerAction::Verify { repo, task, level } => verdict(store, ctx, repo, task, *level, None),
        OwnerAction::Broken { repo, task, note } => verdict(store, ctx, repo, task, 4, Some(note)),
        OwnerAction::ResetDrift { repo } => reset_drift(store, ctx, repo),
    }
}

fn drift(
    store: &mut Store,
    ctx: &ActCtx,
    prefix: &str,
    answer: DriftAnswer,
    task: Option<&(String, String)>,
    estimate: Option<u32>,
    reason: Option<&str>,
) -> Result<String> {
    let reason = check_reason(reason)?;
    if answer == DriftAnswer::MakeTask && task.is_none() {
        return Err(Error::Invalid(
            "make-task needs --parent and --title".into(),
        ));
    }
    store.write(|tx| {
        let s = find_session(tx, prefix)?;
        // An answer needs an open question: the session is (or was last seen) outside
        // tolerance, and the owner hasn't answered it since it was flagged.
        let th = tangent::thresholds(tx, &s.repo_id, ctx.cfg)?;
        let live = tangent::evaluate(tx, &s, &th, ctx.now)?.state;
        if live == NavState::Green && s.nav_state == NavState::Green {
            return Err(stale(format!(
                "already resolved: {} is inside its tolerance band; there is no drift to answer",
                s.session_id
            )));
        }
        if crate::view::drift_answered(tx, &s)? {
            return Err(stale(format!(
                "already resolved: the owner already answered {}'s drift",
                s.session_id
            )));
        }
        let mut reply = match &reason {
            Some(r) => format!("Told {}: continue — \"{r}\".", s.session_id),
            None => format!(
                "Told {}: {}.",
                s.session_id,
                answer.as_str().replace('_', " ")
            ),
        };
        if answer == DriftAnswer::MakeTask {
            let (parent, title) = task.expect("checked above");
            let out = tangent::promote_detour_in(
                tx,
                &s.session_id,
                parent,
                title,
                estimate,
                &Actor::default(),
            )?;
            reply = format!(
                "Detour {} is now task {}, claimed by {}.",
                out.detour.label(),
                out.task_id,
                s.session_id
            );
        }
        let mut data = owner_data("drift", Some(answer.as_str()), Some(s.session_id.clone()));
        data.reason = reason.clone().filter(|_| answer == DriftAnswer::Continue);
        record(tx, &s.repo_id, &data)?;
        recheck(tx, ctx.cfg, &s.session_id, ctx.now)?;
        Ok(reply)
    })
}

fn label_unit(
    store: &mut Store,
    ctx: &ActCtx,
    prefix: &str,
    unit_no: i64,
    label: Label,
) -> Result<String> {
    store.write(|tx| {
        let s = find_session(tx, prefix)?;
        let finished =
            session::unit(tx, &s.session_id, unit_no)?.is_some_and(|u| u.stopped_at.is_some());
        if !finished {
            return Err(stale(format!(
                "no longer needed: no finished unit {unit_no} in this session"
            )));
        }
        let th = tangent::thresholds(tx, &s.repo_id, ctx.cfg)?;
        tangent::label_unit_in(
            tx,
            &s.session_id,
            Some(unit_no),
            label,
            LabelSource::Owner,
            None,
            &th,
            &Actor::default(),
        )?;
        record(
            tx,
            &s.repo_id,
            &owner_data(
                "label",
                Some(label.as_str()),
                Some(format!("{}:{unit_no}", s.session_id)),
            ),
        )?;
        recheck(tx, ctx.cfg, &s.session_id, ctx.now)?;
        Ok(format!(
            "Labelled unit {unit_no} of {} as {}.",
            s.session_id,
            label.display()
        ))
    })
}

/// The repo of a leftover id (exact) when the action doesn't name one.
fn leftover_repo(conn: &Connection, ids: &[String]) -> Result<String> {
    let first = ids
        .first()
        .ok_or_else(|| Error::Invalid("give at least one leftover id".into()))?;
    crate::handoff::get_leftover(conn, first)?
        .map(|l| l.repo_id)
        .ok_or_else(|| stale(format!("no longer needed: no leftover {first}")))
}

fn resolve(
    store: &mut Store,
    ctx: &ActCtx,
    repo: Option<&str>,
    ids: &[String],
    action: LeftoverAction,
) -> Result<String> {
    let repo = match repo {
        Some(r) => r.to_string(),
        None => leftover_repo(store.conn(), ids)?,
    };
    // Still open? (The first resolution stands, an owner's above all.)
    for l in recovery::find(store.conn(), &repo, ids)? {
        if l.state == LeftoverState::Resolved {
            let by = if l.resolved_by == Some(ResolvedBy::Owner) {
                " by the owner"
            } else {
                ""
            };
            return Err(stale(format!(
                "already resolved: leftover {} was already resolved{by}",
                l.leftover_id
            )));
        }
    }
    // The owner action is recorded in the resolution's own write, which checks again that
    // the leftovers are still open (after the git step): racing clicks record one.
    let res = recovery::resolve_then(
        store,
        &repo,
        &ctx.home.snapshots_dir(&repo),
        ids,
        action,
        &Caller::Owner,
        // The owner's choice needs no waiting.
        &recovery::Takeover::new(ctx.now, 0),
        |tx, res| {
            record(
                tx,
                &repo,
                &owner_data("resolve", Some(action.as_str()), Some(res.ids.join(" "))),
            )
        },
    )?;
    resolution_text(store.conn(), &repo, &res)
}

/// What a leftover resolution did, in one line.
pub fn resolution_text(conn: &Connection, repo_id: &str, res: &Resolution) -> Result<String> {
    let n = res.ids.len();
    let what = format!("{n} leftover{}", if n == 1 { "" } else { "s" });
    let short = |sha: &str| sha.chars().take(8).collect::<String>();
    Ok(match res.action {
        LeftoverAction::Adopt => match &res.commit {
            Some(sha) => format!("Adopted {what}: committed {}.", short(sha)),
            None => format!("Adopted {what}: nothing was left uncommitted."),
        },
        LeftoverAction::Undo => match &res.stash {
            Some(st) => format!(
                "Undid {what}: stashed as {} (`git stash apply {}` in {} brings them back).",
                short(&st.sha),
                st.sha,
                res.worktree
            ),
            None => format!("Undid {what}: nothing was left uncommitted."),
        },
        LeftoverAction::Resume => match &res.claimed {
            Some(task) => {
                let title = claim::task_title(conn, repo_id, task)?.unwrap_or_default();
                format!(
                    "Resumed {task} {title}: {what} in {} are yours to finish.",
                    res.worktree
                )
            }
            None => format!(
                "Marked {what} to resume: the next session in {} continues them.",
                res.worktree
            ),
        },
    })
}

/// The worktree a verdict is tied to: the CLI's directory, else the worktree of the
/// session that last claimed the task, else the repo root.
fn verdict_worktree(conn: &Connection, ctx: &ActCtx, repo: &str, task: &str) -> Result<PathBuf> {
    if let Some(dir) = ctx.dir {
        return crate::git::toplevel(dir);
    }
    if let Some(claim) = crate::handoff::latest_claim(conn, repo, task)?
        && let Some(s) = session::get(conn, &claim.session_id)?
    {
        let wt = PathBuf::from(s.worktree.unwrap_or(s.cwd));
        if wt.is_dir() {
            return Ok(wt);
        }
    }
    let root: Option<String> = conn
        .query_row("SELECT root FROM repos WHERE id = ?1", [repo], |r| r.get(0))
        .optional()?;
    root.map(PathBuf::from)
        .filter(|p| p.is_dir())
        .ok_or_else(|| Error::Invalid(format!("no worktree found for {repo}")))
}

fn verdict(
    store: &mut Store,
    ctx: &ActCtx,
    repo: &str,
    task: &str,
    level: i64,
    broken: Option<&str>,
) -> Result<String> {
    let wt = verdict_worktree(store.conn(), ctx, repo, task)?;
    // Checked inside the verdict's write transaction (after HEAD is read), so racing
    // clicks record one verdict.
    let fresh = |conn: &Connection, head: Option<&str>| -> Result<()> {
        let node = get_node(conn, repo, task)?;
        if broken.is_some() && node.is_some_and(|n| n.status == NodeStatus::Failed) {
            return Err(stale(format!(
                "already resolved: {task} is already marked broken"
            )));
        }
        if broken.is_none()
            && level == 4
            && let Some(top) = verify::top_verification(conn, repo, task)?
            && top.level == 4
            && top.state == verify::VerifState::Current
            && head.is_some_and(|h| h == top.sha)
        {
            return Err(stale(format!(
                "already resolved: {task} is already verified L4 at this commit"
            )));
        }
        Ok(())
    };
    verify::owner_verdict(store, repo, task, &wt, level, broken, fresh)
}

fn reset_drift(store: &mut Store, ctx: &ActCtx, repo: &str) -> Result<String> {
    let name = repo_name(store.conn(), repo)?.unwrap_or_else(|| repo.to_string());
    store.write(|tx| {
        let changes = crate::tuning::reset(tx, repo, ctx.cfg)?;
        if changes.is_empty() {
            return Err(stale(format!(
                "Drift tuning for {name} is already at the defaults."
            )));
        }
        record(
            tx,
            repo,
            &owner_data("reset_drift", None, Some(repo.to_string())),
        )?;
        let parts: Vec<String> = changes
            .iter()
            .map(|c| format!("{} {} → {}", c.key, c.old, c.new))
            .collect();
        Ok(format!(
            "Reset drift tuning for {name}: {}.",
            parts.join(", ")
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::check_reason;
    use crate::error::Error;

    #[test]
    fn reasons_must_say_something() {
        assert_eq!(check_reason(None).unwrap(), None);
        assert_eq!(check_reason(Some("   ")).unwrap(), None);
        for bad in [
            "ok",
            "fine.",
            "ok fine",
            "sure, go",
            "continue continue",
            "a b",
            "!!! ???",
        ] {
            assert!(
                matches!(check_reason(Some(bad)), Err(Error::Invalid(_))),
                "{bad}"
            );
        }
        assert_eq!(
            check_reason(Some("  the CI fix unblocks the release ")).unwrap(),
            Some("the CI fix unblocks the release".into())
        );
        assert!(matches!(
            check_reason(Some(&"why ".repeat(60))),
            Err(Error::Invalid(_))
        ));
    }
}
