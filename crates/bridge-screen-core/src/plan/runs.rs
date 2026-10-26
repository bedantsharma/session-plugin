//! Runs (spec §10.1): one goal, but its milestones come in rounds. When every task of the
//! current round passed its tests and a new plan arrives (or the owner says so), the round
//! is **closed** by a `run_closed` event: a boundary at the goal's next milestone number.
//! No node is renamed or moved, so every old id and event stays valid; the closed round is
//! simply left out of what the project reads as its current progress.
//!
//! Nothing here is stored in a view table: the boundaries are read from the events when
//! asked, so a rebuild changes nothing.

use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};

use super::{NodeKind, PlanNode, get_node, is_done, load_tree};
use crate::error::{Error, Result};
use crate::store::{Actor, EventDraft, append_in};

/// The boundary event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunClosed {
    /// Milestone numbers from this one on belong to the next run.
    pub at_milestone_next: u32,
    /// The top-level nodes (milestones) of the run that closed.
    pub closed_ids: Vec<String>,
    /// When it closed (RFC 3339).
    pub closed_at: String,
}

/// A finished, closed run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClosedRun {
    /// 1 for the first run closed.
    pub number: u32,
    /// Its first milestone number.
    pub first: u32,
    /// The first milestone number after it.
    pub next: u32,
    pub milestone_ids: Vec<String>,
    pub closed_at: String,
}

/// Every boundary of a repo, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runs {
    pub closed: Vec<ClosedRun>,
    /// The first milestone number of the current run (1 before any close).
    pub current_first: u32,
}

/// The top-level number of a node id (`3.1.2` → 3); `None` for the goal.
pub fn top_number(id: &str) -> Option<u32> {
    id.split('.').next().and_then(|p| p.parse().ok())
}

impl Runs {
    /// Is the node in the current run? (The goal always is.)
    pub fn in_current(&self, id: &str) -> bool {
        top_number(id).is_none_or(|n| n >= self.current_first)
    }

    /// The run `id` belongs to: `None` for the current run.
    pub fn closed_run_of(&self, id: &str) -> Option<&ClosedRun> {
        let n = top_number(id)?;
        self.closed.iter().find(|r| n >= r.first && n < r.next)
    }

    /// The id as the owner reads it: numbers restart at 1 in each run (`3.1` in a run
    /// that began at milestone 3 reads `1.1`). The one helper every owner-facing screen
    /// uses; agents and the CLI keep using the raw id.
    pub fn shown(&self, id: &str) -> String {
        let Some(n) = top_number(id) else {
            return id.to_string();
        };
        let first = self
            .closed_run_of(id)
            .map_or(self.current_first, |r| r.first);
        let shown = n.saturating_sub(first) + 1;
        match id.split_once('.') {
            Some((_, rest)) => format!("{shown}.{rest}"),
            None => shown.to_string(),
        }
    }
}

/// The repo's runs, from its `run_closed` events.
pub fn load(conn: &Connection, repo_id: &str) -> Result<Runs> {
    let mut stmt = conn.prepare_cached(
        "SELECT data FROM events WHERE repo_id = ?1 AND kind = 'run_closed' ORDER BY pos",
    )?;
    let rows = stmt.query_map([repo_id], |r| r.get::<_, String>(0))?;
    let mut closed: Vec<ClosedRun> = Vec::new();
    let mut first = 1;
    for row in rows {
        let d: RunClosed = serde_json::from_str(&row?)?;
        closed.push(ClosedRun {
            number: u32::try_from(closed.len() + 1).unwrap_or(u32::MAX),
            first,
            next: d.at_milestone_next,
            milestone_ids: d.closed_ids,
            closed_at: d.closed_at,
        });
        first = d.at_milestone_next;
    }
    Ok(Runs {
        closed,
        current_first: first,
    })
}

/// The plan tree of the current run: the goal and everything under milestones from the
/// current boundary on. (`load_tree` still returns every run.)
pub fn run_tree(conn: &Connection, repo_id: &str) -> Result<Vec<PlanNode>> {
    let runs = load(conn, repo_id)?;
    Ok(load_tree(conn, repo_id, false)?
        .into_iter()
        .filter(|n| runs.in_current(&n.id))
        .collect())
}

/// The live tasks and subtasks with no live children in `tree`.
fn leaf_nodes(tree: &[PlanNode]) -> Vec<&PlanNode> {
    tree.iter()
        .filter(|n| matches!(n.kind, NodeKind::Task | NodeKind::Subtask))
        .filter(|n| !tree.iter().any(|c| c.parent_id.as_deref() == Some(&n.id)))
        .collect()
}

/// How many tasks a closed run had.
pub fn tasks_in(conn: &Connection, repo_id: &str, run: &ClosedRun) -> Result<usize> {
    let tree: Vec<PlanNode> = load_tree(conn, repo_id, false)?
        .into_iter()
        .filter(|n| top_number(&n.id).is_none_or(|t| t >= run.first && t < run.next))
        .collect();
    Ok(leaf_nodes(&tree).len())
}

/// What the current run looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunState {
    pub tasks: usize,
    pub unverified: usize,
}

/// Tasks in the current run and how many have not passed their tests (L2+).
pub fn state(conn: &Connection, repo_id: &str) -> Result<RunState> {
    let tree = run_tree(conn, repo_id)?;
    let leaves = leaf_nodes(&tree);
    Ok(RunState {
        tasks: leaves.len(),
        unverified: leaves.iter().filter(|n| !is_done(n.status)).count(),
    })
}

/// The current run is finished: it has tasks and every one passed its tests.
pub fn finished(conn: &Connection, repo_id: &str) -> Result<bool> {
    let s = state(conn, repo_id)?;
    Ok(s.tasks > 0 && s.unverified == 0)
}

/// Close the current run if it is finished; `Ok(None)` when it is not (nothing written).
/// A new milestone calls this first, so it lands in a fresh run.
pub fn close_if_finished(
    tx: &Transaction,
    repo_id: &str,
    session_id: &str,
    actor: &Actor,
) -> Result<Option<ClosedRun>> {
    if get_node(tx, repo_id, "G")?.is_none() || !finished(tx, repo_id)? {
        return Ok(None);
    }
    close_in(tx, repo_id, session_id, actor).map(Some)
}

/// Close the current run on the owner's say-so. Refused, with the reason in words, unless
/// every task of the run passed its tests.
pub fn close_in(
    tx: &Transaction,
    repo_id: &str,
    session_id: &str,
    actor: &Actor,
) -> Result<ClosedRun> {
    let goal = get_node(tx, repo_id, "G")?
        .ok_or_else(|| Error::Invalid("there is no plan to close a run of".into()))?;
    let runs = load(tx, repo_id)?;
    let tree = run_tree(tx, repo_id)?;
    let leaves = leaf_nodes(&tree);
    if leaves.is_empty() {
        return Err(Error::Invalid(
            "the current run has no tasks, so there is nothing to close".into(),
        ));
    }
    let open: Vec<&&PlanNode> = leaves.iter().filter(|n| !is_done(n.status)).collect();
    if !open.is_empty() {
        let names: Vec<String> = open
            .iter()
            .take(3)
            .map(|n| format!("{} {}", runs.shown(&n.id), n.title))
            .collect();
        let more = open.len().saturating_sub(3);
        return Err(Error::Invalid(format!(
            "can't close this run yet: {} of {} tasks haven't passed their tests ({}{}); nothing was closed",
            open.len(),
            leaves.len(),
            names.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        )));
    }
    let next = super::next_child(tx, repo_id, &goal.id)?;
    let closed_ids: Vec<String> = tree
        .iter()
        .filter(|n| n.parent_id.as_deref() == Some("G"))
        .map(|n| n.id.clone())
        .collect();
    let closed_at = jiff::Timestamp::now().to_string();
    let data = RunClosed {
        at_milestone_next: next,
        closed_ids: closed_ids.clone(),
        closed_at: closed_at.clone(),
    };
    append_in(
        tx,
        EventDraft::new(
            repo_id,
            session_id,
            "run_closed",
            serde_json::to_value(data)?,
        )
        .actor(actor.clone()),
    )?;
    Ok(ClosedRun {
        number: u32::try_from(runs.closed.len() + 1).unwrap_or(u32::MAX),
        first: runs.current_first,
        next,
        milestone_ids: closed_ids,
        closed_at,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::plan::{EditContext, PlanOp, plan_edit};
    use crate::store::{EventDraft, Store};

    fn ctx() -> EditContext {
        EditContext {
            repo_id: "r".into(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: None,
            replan: false,
        }
    }

    fn add(parent: Option<&str>, kind: NodeKind, title: &str) -> PlanOp {
        PlanOp::AddNode {
            parent: parent.map(str::to_string),
            kind,
            title: title.into(),
            intent: matches!(kind, NodeKind::Goal | NodeKind::Milestone).then(|| "why".into()),
            estimate_min: matches!(kind, NodeKind::Task).then_some(30),
            expected_files: Vec::new(),
        }
    }

    fn pass(s: &mut Store, task: &str) {
        s.append(
            EventDraft::new(
                "r",
                "owner",
                "verified",
                json!({"verif_id": format!("v{task}"), "task_id": task, "level": 2,
                       "sha": "abc", "files": [], "evidence": "ok", "by": "agent"}),
            )
            .task(task),
        )
        .unwrap();
    }

    /// G, milestone 1 with tasks 1.1 and 1.2.
    fn first_run() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        plan_edit(
            &mut s,
            &ctx(),
            &[
                add(None, NodeKind::Goal, "Ship"),
                add(Some("G"), NodeKind::Milestone, "Core"),
                add(Some("1"), NodeKind::Task, "Schema"),
                add(Some("1"), NodeKind::Task, "Append"),
            ],
        )
        .unwrap();
        s
    }

    fn pct(s: &Store) -> u32 {
        crate::verify::progress(s.conn(), "r").unwrap().done_pct
    }

    #[test]
    fn closing_is_refused_while_a_task_is_unverified() {
        let mut s = first_run();
        pass(&mut s, "1.1");
        let err = s
            .write(|tx| close_in(tx, "r", "owner", &Actor::default()))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("1 of 2 tasks haven't passed their tests") && err.contains("1.2 Append"),
            "{err}"
        );
        assert_eq!(load(s.conn(), "r").unwrap().closed.len(), 0);
        assert_eq!(pct(&s), 50);
    }

    #[test]
    fn a_new_plan_after_a_finished_run_starts_fresh_at_zero() {
        let mut s = first_run();
        pass(&mut s, "1.1");
        pass(&mut s, "1.2");
        assert_eq!(pct(&s), 100);
        assert!(finished(s.conn(), "r").unwrap());
        // The next plan arrives: a new milestone closes the finished run first.
        plan_edit(
            &mut s,
            &ctx(),
            &[
                add(Some("G"), NodeKind::Milestone, "Billing"),
                add(Some("2"), NodeKind::Task, "Invoices"),
            ],
        )
        .unwrap();
        let runs = load(s.conn(), "r").unwrap();
        assert_eq!(runs.closed.len(), 1);
        assert_eq!((runs.closed[0].first, runs.closed[0].next), (1, 2));
        assert_eq!(runs.current_first, 2);
        assert_eq!(pct(&s), 0, "the new run starts at 0%");
        // Numbers restart at 1; raw ids stay for agents.
        assert_eq!(runs.shown("2"), "1");
        assert_eq!(runs.shown("2.1"), "1.1");
        assert_eq!(
            runs.shown("1.2"),
            "1.2",
            "the old run keeps its own numbers"
        );
        assert_eq!(runs.shown("G"), "G");
        // The old run is still readable at 100%.
        let old = &runs.closed[0];
        assert_eq!(tasks_in(s.conn(), "r", old).unwrap(), 2);
        assert!(get_node(s.conn(), "r", "1.1").unwrap().is_some());
        let ids: Vec<String> = run_tree(s.conn(), "r")
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(ids, vec!["G", "2", "2.1"]);
        // Replaying every event rebuilds the same runs.
        s.rebuild().unwrap();
        assert_eq!(load(s.conn(), "r").unwrap(), runs);
        assert_eq!(pct(&s), 0);
    }

    #[test]
    fn an_unfinished_run_is_not_closed_by_a_new_milestone() {
        let mut s = first_run();
        pass(&mut s, "1.1");
        plan_edit(
            &mut s,
            &ctx(),
            &[add(Some("G"), NodeKind::Milestone, "Billing")],
        )
        .unwrap();
        assert_eq!(load(s.conn(), "r").unwrap().closed.len(), 0);
    }

    #[test]
    fn the_owner_can_close_a_finished_run_with_nothing_new_yet() {
        let mut s = first_run();
        pass(&mut s, "1.1");
        pass(&mut s, "1.2");
        let run = s
            .write(|tx| close_in(tx, "r", "owner", &Actor::default()))
            .unwrap();
        assert_eq!((run.number, run.first, run.next), (1, 1, 2));
        // Nothing left to close.
        let again = s.write(|tx| close_in(tx, "r", "owner", &Actor::default()));
        assert!(again.is_err());
        assert_eq!(state(s.conn(), "r").unwrap().tasks, 0);
    }
}
