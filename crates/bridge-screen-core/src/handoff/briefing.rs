//! The I-PASS briefing at SessionStart, the compact re-briefing after compaction, and the
//! read-back check (spec §8.4).

use std::collections::HashSet;
use std::fmt::Write as _;

use rusqlite::{Connection, OptionalExtension, params};

use super::{
    BriefingStatus, Checkpoint, LeftoverState, ReadbackState, latest_checkpoint,
    latest_checkpoint_for_task, latest_note, leftovers,
};
use crate::claim::{keywords, task_title};
use crate::error::Result;
use crate::plan::{NodeKind, NodeStatus, PlanNode, get_node, load_tree};
use crate::session::{self, Session};
use crate::tangent::{self, ItemKind};
use crate::verify;

/// The longest briefing, in lines.
pub const MAX_LINES: usize = 40;

const VERIFY_SHOWN: usize = 3;
const LEFTOVERS_SHOWN: usize = 5;
const IF_THEN_SHOWN: usize = 5;
const DEPS_SHOWN: usize = 5;

/// Asking for the read-back at the end of a briefing.
pub const READBACK_ASK: &str = "Read back before your first edit: call handoff_readback with the goal, what's in flight, and your next action.";

/// The PreToolUse nudge when an edit starts before the read-back.
pub const READBACK_NUDGE: &str =
    "bridge-screen: read back the briefing first (handoff_readback) — it takes one call.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Briefing {
    pub status: BriefingStatus,
    pub text: String,
    pub readback_required: bool,
    /// The unverified tasks the briefing shows, with their level: each counts as a
    /// verification nudge (spec §8.5).
    pub nudges: Vec<(String, i64)>,
}

/// First sentence of an intent.
fn first_sentence(text: &str) -> &str {
    let t = text.trim();
    match t.find(". ") {
        Some(i) => &t[..=i],
        None => t,
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn display(conn: &Connection, repo_id: &str, id: &str) -> Result<String> {
    let title = task_title(conn, repo_id, id)?.unwrap_or_default();
    Ok(format!("{id} {title}").trim_end().to_string())
}

/// The session that last ended in this repo (not `me`), preferring the same worktree.
fn previous(conn: &Connection, me: &Session) -> Result<Option<Session>> {
    let id: Option<String> = conn
        .prepare_cached(
            "SELECT session_id FROM sessions
             WHERE repo_id = ?1 AND session_id != ?2 AND state = 'ended'
             ORDER BY (worktree IS ?3) DESC, ended_at DESC LIMIT 1",
        )?
        .query_row(params![me.repo_id, me.session_id, me.worktree], |r| {
            r.get(0)
        })
        .optional()?;
    match id {
        Some(id) => session::get(conn, &id),
        None => Ok(None),
    }
}

/// Everything the briefing is built from.
struct Facts {
    goal: Option<PlanNode>,
    milestone: Option<PlanNode>,
    task: Option<String>,
    checkpoint: Option<Checkpoint>,
    cue: Option<String>,
    note: Option<super::HandoffNote>,
    unverified: Vec<String>,
    /// The tasks behind `unverified`, with their level.
    unverified_tasks: Vec<(String, i64)>,
    failed: Vec<String>,
    missed: Vec<Session>,
    leftovers: Vec<super::Leftover>,
    detours: Vec<tangent::Detour>,
    suspended: Vec<String>,
    deps: Vec<String>,
}

/// The milestone above `task`, if any.
fn milestone_of(conn: &Connection, repo_id: &str, task: &str) -> Result<Option<PlanNode>> {
    let mut id = Some(task.to_string());
    while let Some(cur) = id {
        let Some(node) = get_node(conn, repo_id, &cur)? else {
            return Ok(None);
        };
        if node.kind == NodeKind::Milestone {
            return Ok(Some(node));
        }
        id = node.parent_id;
    }
    Ok(None)
}

/// The latest unresumed resume cue for `task` among `sessions`.
fn resume_cue(conn: &Connection, sessions: &[&Session], task: &str) -> Result<Option<String>> {
    let mut best: Option<(String, String)> = None;
    for s in sessions {
        for item in tangent::suspended(conn, &s.session_id)? {
            if item.kind == ItemKind::Task
                && item.ref_id == task
                && best.as_ref().is_none_or(|(at, _)| item.suspended_at > *at)
            {
                best = Some((item.suspended_at, item.resume_cue));
            }
        }
    }
    Ok(best.map(|(_, cue)| cue))
}

fn is_done(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Claimed | NodeStatus::L1 | NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4
    )
}

/// `id ready`, `id waiting` or `id waiting on <session>` for each dependency of `task`.
fn deps_of(conn: &Connection, repo: &str, task: &str) -> Result<Vec<String>> {
    let mut deps = Vec::new();
    let Some(node) = get_node(conn, repo, task)? else {
        return Ok(deps);
    };
    for d in node.depends_on {
        let Some(dep) = get_node(conn, repo, &d)? else {
            continue;
        };
        if is_done(dep.status) {
            deps.push(format!("{d} ready"));
            continue;
        }
        let holder = session::list_live(conn, Some(repo))?
            .into_iter()
            .find(|s| s.active_task.as_deref() == Some(d.as_str()));
        deps.push(match holder {
            Some(h) => format!(
                "{d} waiting on {}",
                h.session_id.chars().take(6).collect::<String>()
            ),
            None => format!("{d} waiting"),
        });
    }
    Ok(deps)
}

fn gather(conn: &Connection, me: &Session) -> Result<Facts> {
    let repo = me.repo_id.as_str();
    let prev = previous(conn, me)?;
    let task = me
        .active_task
        .clone()
        .or_else(|| prev.as_ref().and_then(|p| p.last_task.clone()));
    let mut sessions: Vec<&Session> = vec![me];
    sessions.extend(prev.as_ref());
    let tree = load_tree(conn, repo, false)?;
    let goal = tree.iter().find(|n| n.kind == NodeKind::Goal).cloned();
    let mut unverified = Vec::new();
    let mut unverified_tasks = Vec::new();
    let mut failed = Vec::new();
    for n in &tree {
        if !matches!(n.kind, NodeKind::Task | NodeKind::Subtask) {
            continue;
        }
        if n.status == NodeStatus::Failed {
            failed.push(format!("{} {}", n.id, n.title));
            continue;
        }
        // Claimed or L1 from an earlier session (this one knows what it just claimed),
        // and anything whose verification went stale (spec §8.5).
        let stale = verify::stale_note(conn, repo, &n.id)?;
        let older_claim = matches!(n.status, NodeStatus::Claimed | NodeStatus::L1)
            && super::latest_claim(conn, repo, &n.id)?
                .is_some_and(|c| c.session_id != me.session_id);
        if stale.is_none() && !older_claim {
            continue;
        }
        unverified.push(match stale {
            Some(note) => format!("{} {} ({note})", n.id, n.title),
            None => format!("{} {}", n.id, n.title),
        });
        unverified_tasks.push((n.id.clone(), verify::current_level(conn, repo, &n.id)?));
    }
    let missed = session::list_live(conn, Some(repo))?
        .into_iter()
        .chain(prev.clone())
        .chain(std::iter::once(me.clone()))
        .filter(|s| s.checkpoint_missed != 0 && s.checkpoint_pending.is_some())
        .fold(Vec::<Session>::new(), |mut acc, s| {
            if !acc.iter().any(|a| a.session_id == s.session_id) {
                acc.push(s);
            }
            acc
        });
    let mut detours = Vec::new();
    let mut suspended = Vec::new();
    for s in &sessions {
        detours.extend(tangent::open_detours(conn, &s.session_id)?);
        for item in tangent::suspended(conn, &s.session_id)? {
            if Some(item.ref_id.as_str()) != task.as_deref() {
                suspended.push(tangent::item_display(conn, repo, &item)?);
            }
        }
    }
    let (milestone, checkpoint, cue, note, deps) = match &task {
        Some(t) => {
            let checkpoint = match latest_checkpoint_for_task(conn, repo, t)? {
                Some(c) => Some(c),
                None => prev
                    .as_ref()
                    .map(|p| latest_checkpoint(conn, &p.session_id))
                    .transpose()?
                    .flatten(),
            };
            let note = latest_note(conn, repo, Some(t))?.filter(|n| n.session_id != me.session_id);
            let deps = deps_of(conn, repo, t)?;
            (
                milestone_of(conn, repo, t)?,
                checkpoint,
                resume_cue(conn, &sessions, t)?,
                note,
                deps,
            )
        }
        None => (None, None, None, None, Vec::new()),
    };
    let leftovers = leftovers(conn, repo, true)?
        .into_iter()
        .filter(|l| l.state == LeftoverState::Open)
        .collect();
    Ok(Facts {
        goal,
        milestone,
        task,
        checkpoint,
        cue,
        note,
        unverified,
        unverified_tasks,
        failed,
        missed,
        leftovers,
        detours,
        suspended,
        deps,
    })
}

impl Facts {
    fn status(&self) -> BriefingStatus {
        if !self.leftovers.is_empty() || !self.failed.is_empty() || !self.missed.is_empty() {
            BriefingStatus::Unstable
        } else if !self.detours.is_empty()
            || !self.suspended.is_empty()
            || !self.unverified.is_empty()
        {
            BriefingStatus::Watcher
        } else {
            BriefingStatus::Stable
        }
    }

    /// The next action: the resume cue, else the last checkpoint's `Bridge-Next`.
    fn next(&self) -> Option<&str> {
        self.cue.as_deref().or_else(|| {
            self.checkpoint
                .as_ref()
                .and_then(|c| c.next_action.as_deref())
        })
    }

    /// Nothing for a stable session to hear beyond the goal.
    fn nothing_pending(&self) -> bool {
        self.next().is_none()
            && self.note.is_none()
            && self.deps.is_empty()
            && self.checkpoint.is_none()
    }
}

fn goal_line(f: &Facts) -> String {
    match &f.goal {
        Some(g) => format!(
            "Goal: {} — {}",
            g.title,
            first_sentence(g.intent.as_deref().unwrap_or(""))
        )
        .trim_end_matches([' ', '—'])
        .to_string(),
        None => "No plan yet — ask the owner whether to create one (bridge-screen plan edit / plan_edit tool).".to_string(),
    }
}

/// `items` (already formatted), the first `shown` of them, then `(+N more)`.
fn capped(items: &[String], shown: usize) -> String {
    let mut text = items
        .iter()
        .take(shown)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > shown {
        let _ = write!(text, " (+{} more)", items.len() - shown);
    }
    text
}

/// The action list: commit first, failed and unverified items, leftovers, open detours,
/// suspended items, and the next action.
fn action_lines(f: &Facts, lines: &mut Vec<String>) {
    for s in f.missed.iter().take(VERIFY_SHOWN) {
        let short: String = s.session_id.chars().take(6).collect();
        lines.push(format!(
            "  - Commit first: checkpoint missed in session {short} (due: {}).",
            s.checkpoint_pending.as_deref().unwrap_or("")
        ));
    }
    if f.missed.len() > VERIFY_SHOWN {
        lines.push(format!(
            "    (+{} more sessions with a missed checkpoint)",
            f.missed.len() - VERIFY_SHOWN
        ));
    }
    if !f.failed.is_empty() {
        lines.push(format!("  - Failed: {}", capped(&f.failed, VERIFY_SHOWN)));
    }
    if !f.unverified.is_empty() {
        lines.push(format!(
            "  - Verify first ({}): {}",
            f.unverified.len(),
            capped(&f.unverified, VERIFY_SHOWN)
        ));
    }
    if !f.leftovers.is_empty() {
        lines.push(format!(
            "  - Leftovers ({}, resolve with resolve_leftover):",
            f.leftovers.len()
        ));
        for l in f.leftovers.iter().take(LEFTOVERS_SHOWN) {
            let task = l.task_id.as_deref().unwrap_or("no task");
            lines.push(format!(
                "    {} {} ({task}) → {}",
                l.leftover_id,
                l.path,
                l.suggested.as_str()
            ));
        }
        if f.leftovers.len() > LEFTOVERS_SHOWN {
            lines.push(format!(
                "    (+{} more)",
                f.leftovers.len() - LEFTOVERS_SHOWN
            ));
        }
    }
    if !f.detours.is_empty() {
        let list: Vec<String> = f
            .detours
            .iter()
            .map(|d| format!("{} {}", d.label(), d.reason))
            .collect();
        lines.push(format!("  - Open detours: {}", capped(&list, VERIFY_SHOWN)));
    }
    if !f.suspended.is_empty() {
        lines.push(format!(
            "  - Suspended: {}",
            capped(&f.suspended, VERIFY_SHOWN)
        ));
    }
    if let Some(next) = f.next() {
        lines.push(format!("  - Next: {next}"));
    }
}

/// The SessionStart briefing (≤ 40 lines), in I-PASS order: severity, summary, actions,
/// if/then, dependencies, and the read-back request unless stable. A stable session with
/// nothing pending gets three lines.
pub fn briefing(conn: &Connection, me: &Session) -> Result<Briefing> {
    let f = gather(conn, me)?;
    let status = f.status();
    let mut lines = vec![
        format!("bridge-screen briefing — {}", status.as_str()),
        goal_line(&f),
    ];
    if status == BriefingStatus::Stable && f.nothing_pending() {
        if f.goal.is_none() {
            return Ok(Briefing {
                status,
                text: lines.join("\n"),
                readback_required: false,
                nudges: Vec::new(),
            });
        }
        lines.push(match &me.active_task {
            Some(t) => format!(
                "Active task: {} — carry on.",
                display(conn, &me.repo_id, t)?
            ),
            None => "Claim your task with claim_task and go.".to_string(),
        });
        return Ok(Briefing {
            status,
            text: lines.join("\n"),
            readback_required: false,
            nudges: Vec::new(),
        });
    }
    if let Some(m) = &f.milestone {
        lines.push(
            format!(
                "Milestone: {} — {}",
                m.title,
                first_sentence(m.intent.as_deref().unwrap_or(""))
            )
            .trim_end_matches([' ', '—'])
            .to_string(),
        );
    }
    let mut summary = match &f.task {
        Some(t) => display(conn, &me.repo_id, t)?,
        None => "no task yet".to_string(),
    };
    if let Some(c) = &f.checkpoint {
        let _ = write!(summary, "; last checkpoint {}", short_sha(&c.sha));
        if let Some(next) = &c.next_action {
            let _ = write!(summary, " \"{next}\"");
        }
    }
    if let Some(cue) = &f.cue {
        let _ = write!(summary, "; resume cue: {cue}");
    }
    lines.push(format!("Summary: {summary}"));
    if let Some(n) = &f.note {
        lines.push(format!("In flight: {}", n.in_flight));
    }
    lines.push("Actions:".to_string());
    action_lines(&f, &mut lines);
    if let Some(n) = f.note.as_ref().filter(|n| !n.if_then.is_empty()) {
        lines.push("If/then:".to_string());
        for line in n.if_then.iter().take(IF_THEN_SHOWN) {
            lines.push(format!("  - {line}"));
        }
    }
    if !f.deps.is_empty() {
        lines.push(format!("Dependencies: {}", capped(&f.deps, DEPS_SHOWN)));
    }
    let readback_required = status != BriefingStatus::Stable;
    // Cut to the cap, keeping room for the read-back request: it must always be there.
    lines.truncate(MAX_LINES - usize::from(readback_required));
    if readback_required {
        lines.push(READBACK_ASK.to_string());
    }
    let nudges = f
        .unverified_tasks
        .iter()
        .take(VERIFY_SHOWN)
        .cloned()
        .collect();
    Ok(Briefing {
        status,
        text: lines.join("\n"),
        readback_required,
        nudges,
    })
}

/// After compaction (≤ 8 lines, no read-back): the active task, its cue or next action,
/// and open detours.
pub fn compact_rebriefing(conn: &Connection, me: &Session) -> Result<String> {
    let mut lines = vec!["bridge-screen: re-briefing after compaction.".to_string()];
    match &me.active_task {
        Some(t) => {
            lines.push(format!("Active task: {}", display(conn, &me.repo_id, t)?));
            if let Some(cue) = resume_cue(conn, &[me], t)? {
                lines.push(format!("On return: {cue}"));
            } else if let Some(next) =
                latest_checkpoint_for_task(conn, &me.repo_id, t)?.and_then(|c| c.next_action)
            {
                lines.push(format!("Next: {next}"));
            }
        }
        None => lines.push("Active task: none — claim one with claim_task.".to_string()),
    }
    let detours = tangent::open_detours(conn, &me.session_id)?;
    if !detours.is_empty() {
        let list: Vec<String> = detours
            .iter()
            .map(|d| {
                let current = me.current_detour.as_deref() == Some(d.detour_id.as_str());
                format!(
                    "{} {}{}",
                    d.label(),
                    d.reason,
                    if current { " (current)" } else { "" }
                )
            })
            .collect();
        lines.push(format!("Open detours: {}", capped(&list, VERIFY_SHOWN)));
    }
    lines.truncate(8);
    Ok(lines.join("\n"))
}

/// The outcome of a read-back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadbackCheck {
    pub result: ReadbackState,
    pub diff: Option<String>,
    /// The tool result: a go-ahead, the one correction, or a plain acknowledgement.
    pub reply: String,
}

/// Does `text` mention task id `id` as a word?
fn mentions(text: &str, id: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '.'))
        .map(|w| w.trim_matches('.'))
        .any(|w| w.eq_ignore_ascii_case(id))
}

/// Has this session already had its correction?
fn corrected(conn: &Connection, session_id: &str) -> Result<bool> {
    let n: i64 = conn
        .prepare_cached(
            "SELECT count(*) FROM events WHERE session_id = ?1 AND kind = 'readback'
             AND json_extract(data, '$.result') = 'mismatch'",
        )?
        .query_row([session_id], |r| r.get(0))?;
    Ok(n > 0)
}

/// Compare a read-back with the log (spec §8.4): what's in flight must name the active
/// task (or a leftover's task when there is none), and the next action must share a
/// keyword with the expected one (resume cue or `Bridge-Next`) when there is one. On a
/// mismatch the reply is the one correction; a session never gets a second.
pub fn check_readback(
    conn: &Connection,
    me: &Session,
    _goal: &str,
    in_flight: &str,
    next_action: &str,
) -> Result<ReadbackCheck> {
    let f = gather(conn, me)?;
    let task = me
        .active_task
        .clone()
        .or_else(|| f.leftovers.iter().find_map(|l| l.task_id.clone()))
        .or_else(|| f.task.clone());
    let expected_next = f.next().map(str::to_string);
    let mut diffs = Vec::new();
    if let Some(t) = &task
        && !mentions(in_flight, t)
    {
        diffs.push(format!("task: expected {t}"));
    }
    if let Some(next) = &expected_next {
        let want: HashSet<String> = keywords(next);
        let got: HashSet<String> = keywords(next_action);
        if !want.is_empty() && want.is_disjoint(&got) {
            diffs.push(format!("next action: expected \"{next}\""));
        }
    }
    if diffs.is_empty() {
        return Ok(ReadbackCheck {
            result: ReadbackState::Ok,
            diff: None,
            reply: "Read-back matches the log. Go ahead.".to_string(),
        });
    }
    let reply = if corrected(conn, &me.session_id)? {
        "Read-back recorded.".to_string()
    } else {
        let mut text = String::from("Close, but the log says: ");
        match &task {
            Some(t) => {
                let title = task_title(conn, &me.repo_id, t)?.unwrap_or_default();
                let _ = write!(text, "active task {t} \"{title}\"");
            }
            None => text.push_str("no active task"),
        }
        if let Some(next) = &expected_next {
            let _ = write!(text, ", next action \"{next}\"");
        }
        text.push_str(". Carry on from there.");
        text
    };
    Ok(ReadbackCheck {
        result: ReadbackState::Mismatch,
        diff: Some(diffs.join("; ")),
        reply,
    })
}
