//! Climbing the ladder above L1 (spec §8.5): the agent's `verify` (L2 with the test
//! re-run, L3 as reported, L4 with the owner's words) and the owner's own verdict.
//!
//! Refusals record nothing. L1–L3 by the agent are tied to HEAD and refused while the
//! files they depend on have uncommitted changes. The re-run happens outside any write
//! transaction (it may take minutes), and only its outcome is recorded.

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use super::rerun::{self, RerunOutcome};
use super::{
    By, L1Outcome, Rerun, VerificationFailed, check_l1, current_level, depended_files, l1_draft,
    missing_text, progress, settings, uncommitted, verified_draft,
};
use crate::error::{Error, Result};
use crate::git;
use crate::plan::{NodeKind, NodeStatus, PlanNode, get_node};
use crate::session::{self, SessionState};
use crate::store::{Actor, EventDraft, OWNER_SESSION, Store, append_in};
use crate::tangent::OwnerAction;

fn refuse(msg: impl Into<String>) -> Error {
    Error::Verify(msg.into())
}

/// Longest `owner_said` kept.
pub const MAX_OWNER_SAID: usize = 200;

/// The agent's `verify` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRequest {
    pub session_id: String,
    /// Defaults to the session's active task.
    pub task_id: Option<String>,
    /// 2, 3 or 4.
    pub level: i64,
    /// What was run or driven and what it showed.
    pub evidence: String,
    /// L2: the exit code of the agent's own test run.
    pub exit_code: Option<i64>,
    /// L2: the test summary line (`12 passed`).
    pub summary: Option<String>,
    /// L4: the owner's words.
    pub owner_said: Option<String>,
    pub actor: Actor,
}

/// What a `verify` call did: the new level (`None` when the re-run failed) and the reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyOutcome {
    pub task_id: String,
    pub level: Option<i64>,
    pub reply: String,
}

/// A task or subtask that is still in the plan.
fn leaf(conn: &Connection, repo_id: &str, task: &str) -> Result<PlanNode> {
    let node = get_node(conn, repo_id, task)?
        .ok_or_else(|| refuse(format!("no task {task} in this plan")))?;
    if node.status == NodeStatus::Dropped {
        return Err(refuse(format!("{task} was dropped")));
    }
    if matches!(node.kind, NodeKind::Goal | NodeKind::Milestone) {
        return Err(refuse(format!(
            "{task} is a {}; verify a task or subtask",
            node.kind.as_str()
        )));
    }
    Ok(node)
}

fn clip(text: &str, max: usize) -> String {
    crate::decision::clip(text, max)
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// ` Project 41% done.`
fn project_line(conn: &Connection, repo_id: &str) -> Result<String> {
    Ok(format!(
        " Project {}% done.",
        progress(conn, repo_id)?.done_pct
    ))
}

/// The commit a verification is tied to: HEAD, with the depended-on files committed.
fn committed_head(wt: &Path, files: &[String]) -> Result<String> {
    let dirty = uncommitted(wt, files)?;
    if !dirty.is_empty() {
        return Err(refuse(format!(
            "commit a checkpoint first; verifications are tied to a commit (uncommitted: {})",
            dirty.join(", ")
        )));
    }
    git::head(wt)?.ok_or_else(|| refuse("the worktree has no commit yet; commit first"))
}

/// Who and what one `verify` call is about.
struct Call<'a> {
    req: &'a VerifyRequest,
    repo: String,
    sid: String,
    task: String,
    evidence: String,
}

impl Call<'_> {
    fn record(&self, store: &mut Store, draft: EventDraft) -> Result<()> {
        store.write(|tx| {
            append_in(tx, draft.actor(self.req.actor.clone()))?;
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)] // the payload, minus what the call knows
    fn verified(
        &self,
        level: i64,
        sha: &str,
        files: Vec<String>,
        evidence: &str,
        by: By,
        rerun: Option<Rerun>,
    ) -> Result<EventDraft> {
        verified_draft(
            &self.repo, &self.sid, &self.task, level, sha, files, evidence, by, rerun,
        )
    }

    /// The reply plus ` Project N% done.`, and the outcome.
    fn done(self, store: &Store, level: i64, reply: &str) -> Result<VerifyOutcome> {
        let line = project_line(store.conn(), &self.repo)?;
        Ok(VerifyOutcome {
            task_id: self.task,
            level: Some(level),
            reply: format!("{reply}{line}"),
        })
    }
}

/// The agent climbs the ladder (spec §8.5, §11.2). `wt` is the session's worktree;
/// `default_timeout` applies unless the repo sets its own.
pub fn verify(
    store: &mut Store,
    req: &VerifyRequest,
    wt: &Path,
    default_timeout: Duration,
) -> Result<VerifyOutcome> {
    let conn = store.conn();
    let s = session::get(conn, &req.session_id)?
        .ok_or_else(|| refuse(format!("unknown session {}", req.session_id)))?;
    if s.state == SessionState::Ended {
        return Err(refuse(format!("session {} has ended", req.session_id)));
    }
    let task = req
        .task_id
        .clone()
        .or_else(|| s.active_task.clone())
        .ok_or_else(|| refuse("no active task; give the id of the task to verify"))?;
    let node = leaf(conn, &s.repo_id, &task)?;
    if !matches!(
        node.status,
        NodeStatus::Claimed | NodeStatus::L1 | NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4
    ) {
        return Err(refuse(format!(
            "{task} isn't claimed done ({}); call claim_done first",
            node.status.as_str()
        )));
    }
    let evidence = clip(&req.evidence, super::MAX_EVIDENCE);
    if evidence.is_empty() {
        return Err(refuse(
            "evidence is required: what you ran or drove and what it showed",
        ));
    }
    let call = Call {
        req,
        repo: s.repo_id,
        sid: s.session_id,
        task,
        evidence,
    };
    match req.level {
        2 => level2(store, call, wt, default_timeout),
        3 => level3(store, call, wt),
        4 => level4(store, call, wt),
        1 => Err(refuse(
            "L1 is automatic: claim_done with the files, committed; verify takes level 2, 3 or 4",
        )),
        n => Err(refuse(format!("level {n}? verify takes level 2, 3 or 4"))),
    }
}

/// L3: the agent drove it end to end; recorded as reported, on top of L1.
fn level3(store: &mut Store, call: Call, wt: &Path) -> Result<VerifyOutcome> {
    let conn = store.conn();
    if current_level(conn, &call.repo, &call.task)? < 1 {
        return Err(refuse(format!(
            "{} has no L1 yet: name its files with claim_done and commit them, then verify level 3",
            call.task
        )));
    }
    let files = depended_files(conn, &call.repo, &call.task)?;
    let sha = committed_head(wt, &files)?;
    let draft = call.verified(3, &sha, files, &call.evidence, By::Agent, None)?;
    call.record(store, draft)?;
    let reply = format!(
        "{} verified L3 (end-to-end, as reported) at {}.",
        call.task,
        short(&sha)
    );
    call.done(store, 3, &reply)
}

/// L4 through the agent: only with the owner's own words.
fn level4(store: &mut Store, call: Call, wt: &Path) -> Result<VerifyOutcome> {
    let said = call
        .req
        .owner_said
        .as_deref()
        .map(|t| clip(t, MAX_OWNER_SAID))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            refuse("owner_said is required for level 4: the owner's own words that it works")
        })?;
    let files = depended_files(store.conn(), &call.repo, &call.task)?;
    let sha = git::head(wt)?.unwrap_or_default();
    let text = clip(
        &format!("owner: \"{said}\" · {}", call.evidence),
        super::MAX_EVIDENCE,
    );
    let draft = call.verified(4, &sha, files, &text, By::OwnerViaAgent, None)?;
    call.record(store, draft)?;
    let reply = format!("{} verified L4 (the owner's word).", call.task);
    call.done(store, 4, &reply)
}

/// Before L2: the task needs L1 at HEAD; check (and record) it now if it hasn't one.
fn ensure_l1(store: &mut Store, call: &Call, wt: &Path) -> Result<()> {
    let conn = store.conn();
    if current_level(conn, &call.repo, &call.task)? >= 1 {
        return Ok(());
    }
    match check_l1(conn, wt, &call.repo, &call.task)? {
        outcome @ L1Outcome::Passed { .. } => {
            if let Some(draft) = l1_draft(&call.repo, &call.sid, &call.task, &outcome)? {
                call.record(store, draft)?;
            }
            Ok(())
        }
        L1Outcome::Missing {
            sha,
            paths,
            symbols,
        } => Err(refuse(format!(
            "L1 check: {} not found at {}. Fix the evidence with claim_done, then verify again.",
            missing_text(&paths, &symbols),
            short(&sha)
        ))),
        L1Outcome::NotCommitted { paths } => Err(refuse(format!(
            "commit a checkpoint first; verifications are tied to a commit (uncommitted: {})",
            paths.join(", ")
        ))),
        L1Outcome::NoEvidence => Err(refuse(
            "name the files that show the work with claim_done (files) first",
        )),
    }
}

/// L2: the agent's tests passed and are committed; bridge-screen re-runs the owner's
/// test command when there is one.
fn level2(
    store: &mut Store,
    call: Call,
    wt: &Path,
    default_timeout: Duration,
) -> Result<VerifyOutcome> {
    let code = call.req.exit_code.ok_or_else(|| {
        refuse("exit_code is required for level 2: the exit code of your test run")
    })?;
    if code != 0 {
        return Err(refuse(format!(
            "Exit code {code} means the tests failed — fix them, commit, and verify again."
        )));
    }
    let files = depended_files(store.conn(), &call.repo, &call.task)?;
    let sha = committed_head(wt, &files)?;
    ensure_l1(store, &call, wt)?;
    let mut evidence = call.evidence.clone();
    if let Some(sum) = call
        .req
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        evidence = format!("{evidence} · {}", clip(sum, rerun::MAX_LAST_LINE));
    }
    let repo_settings = settings(store.conn(), &call.repo)?;
    let timeout = repo_settings
        .test_timeout_s
        .map_or(default_timeout, Duration::from_secs);
    let task = &call.task;
    let (by, rerun, reply) = match &repo_settings.test_command {
        None => (
            By::Agent,
            Rerun::NotConfigured,
            format!(
                "{task} verified L2 (as reported; no test command to re-run). Ask the owner to set a test command: bridge-screen config set test-command '<cmd>'."
            ),
        ),
        Some(cmd) => match rerun::run(wt, cmd, timeout) {
            RerunOutcome::Passed { secs, .. } => {
                evidence = format!("{evidence} · re-run ok in {secs}s");
                (
                    By::Rerun,
                    Rerun::Passed,
                    format!("{task} verified L2 (re-run passed in {secs}s)."),
                )
            }
            RerunOutcome::Failed {
                code, last_line, ..
            } => return rerun_failed(store, call, &sha, code, &last_line),
            RerunOutcome::TimedOut { .. } => (
                By::Agent,
                Rerun::TimedOut,
                format!(
                    "{task} verified L2 (recorded as reported; the re-run took longer than {}s).",
                    timeout.as_secs()
                ),
            ),
            RerunOutcome::SpawnError(e) => (
                By::Agent,
                Rerun::NotConfigured,
                format!(
                    "{task} verified L2 (recorded as reported; the re-run couldn't start: {e})."
                ),
            ),
        },
    };
    let draft = call.verified(2, &sha, files, &evidence, by, Some(rerun))?;
    call.record(store, draft)?;
    call.done(store, 2, &reply)
}

/// The owner's test command failed: `verification_failed`, the task goes back to work.
fn rerun_failed(
    store: &mut Store,
    call: Call,
    sha: &str,
    code: i32,
    last_line: &str,
) -> Result<VerifyOutcome> {
    let data = VerificationFailed {
        task_id: call.task.clone(),
        level: 2,
        sha: Some(sha.to_string()),
        evidence: last_line.to_string(),
        by: By::Rerun,
        rerun: Some(Rerun::Failed),
    };
    let draft = EventDraft::new(
        &call.repo,
        &call.sid,
        "verification_failed",
        serde_json::to_value(data)?,
    )
    .task(&call.task);
    call.record(store, draft)?;
    let task = call.task;
    Ok(VerifyOutcome {
        reply: format!(
            "The re-run failed (exit {code}): {last_line}. {task} is back to failed; fix it and verify again."
        ),
        task_id: task,
        level: None,
    })
}

/// The owner's verdict from the CLI (spec §10.1): it works (`verified` at `level`, by
/// owner, at HEAD of `wt`) or it's broken (`verification_failed` with the note). Also an
/// `owner_action`. Returns the line to print.
pub fn owner_verdict(
    store: &mut Store,
    repo_id: &str,
    task: &str,
    wt: &Path,
    level: i64,
    broken: Option<&str>,
) -> Result<String> {
    leaf(store.conn(), repo_id, task)?;
    if !(1..=4).contains(&level) {
        return Err(refuse(format!("level {level}? use 1 to 4")));
    }
    let sha = git::head(wt)?;
    let files = depended_files(store.conn(), repo_id, task)?;
    let title = crate::claim::task_title(store.conn(), repo_id, task)?.unwrap_or_default();
    store.write(|tx| {
        let (answer, line) = if let Some(note) = broken {
            let note = clip(note, super::MAX_EVIDENCE);
            let data = VerificationFailed {
                task_id: task.to_string(),
                level,
                sha: sha.clone(),
                evidence: if note.is_empty() {
                    "broken".into()
                } else {
                    note
                },
                by: By::Owner,
                rerun: None,
            };
            append_in(
                tx,
                EventDraft::new(
                    repo_id,
                    OWNER_SESSION,
                    "verification_failed",
                    serde_json::to_value(data)?,
                )
                .task(task),
            )?;
            (
                "broken",
                format!("Marked {task} {title} broken; it's back to failed."),
            )
        } else {
            let sha = sha.clone().unwrap_or_default();
            append_in(
                tx,
                verified_draft(
                    repo_id,
                    OWNER_SESSION,
                    task,
                    level,
                    &sha,
                    files.clone(),
                    "verified by the owner",
                    By::Owner,
                    None,
                )?,
            )?;
            let at = if sha.is_empty() {
                String::new()
            } else {
                format!(" at {}", short(&sha))
            };
            ("works", format!("Verified {task} {title} L{level}{at}."))
        };
        let action = OwnerAction {
            action: "verify".into(),
            answer: Some(answer.into()),
            target: Some(task.to_string()),
        };
        append_in(
            tx,
            EventDraft::new(
                repo_id,
                OWNER_SESSION,
                "owner_action",
                serde_json::to_value(action)?,
            )
            .task(task),
        )?;
        Ok(line)
    })
}

/// The owner sets (`Some`) or unsets (`None`) a per-repo test setting: one
/// `config_changed` with `session_id = owner`. Returns whether anything changed.
pub fn change_setting(
    store: &mut Store,
    repo_id: &str,
    key: &str,
    new: Option<serde_json::Value>,
) -> Result<bool> {
    if !super::SETTING_KEYS.contains(&key) {
        return Err(refuse(format!("unknown setting {key}")));
    }
    let now = settings(store.conn(), repo_id)?;
    let old = match key {
        "test_command" => now.test_command.map(serde_json::Value::from),
        _ => now.test_timeout_s.map(serde_json::Value::from),
    }
    .unwrap_or(serde_json::Value::Null);
    let new = new.unwrap_or(serde_json::Value::Null);
    if old == new {
        return Ok(false);
    }
    let data = super::SettingChanged {
        scope: "repo".into(),
        key: key.into(),
        old,
        new,
    };
    store.append(EventDraft::new(
        repo_id,
        OWNER_SESSION,
        "config_changed",
        serde_json::to_value(data)?,
    ))?;
    Ok(true)
}
