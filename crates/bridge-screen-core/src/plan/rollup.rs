//! The status a parent shows (spec §10): derived from its live tasks when read, never
//! written to `plan_nodes`, so adding a task to a finished milestone makes it read
//! "in progress" again and a rebuild has nothing to reproduce.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use serde::Serialize;

use super::model::{NodeKind, NodeStatus, PlanNode};

/// A parent's status, from the tasks under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rollup {
    /// Nothing under it has started.
    Todo,
    /// Some work under it started or finished, not all of it.
    InProgress,
    /// Every task under it passed its tests (L2 or above, the done% test).
    Done,
    /// A task under it failed verification.
    NeedsAttention,
}

impl Rollup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::NeedsAttention => "needs_attention",
        }
    }

    /// In words, for the screens.
    pub fn words(self) -> &'static str {
        match self {
            Self::Todo => "to do",
            Self::InProgress => "in progress",
            Self::Done => "done",
            Self::NeedsAttention => "needs attention",
        }
    }
}

/// The same "done" test as done%: tests passed, or better.
pub fn is_done(status: NodeStatus) -> bool {
    matches!(status, NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4)
}

/// The roll-up of a set of task statuses; `None` when there are none (dropped ones are
/// ignored), so the node keeps its own status.
pub fn rollup_of(statuses: impl IntoIterator<Item = NodeStatus>) -> Option<Rollup> {
    let live: Vec<NodeStatus> = statuses
        .into_iter()
        .filter(|s| *s != NodeStatus::Dropped)
        .collect();
    if live.is_empty() {
        return None;
    }
    Some(if live.contains(&NodeStatus::Failed) {
        Rollup::NeedsAttention
    } else if live.iter().all(|s| is_done(*s)) {
        Rollup::Done
    } else if live.iter().any(|s| *s != NodeStatus::Todo) {
        Rollup::InProgress
    } else {
        Rollup::Todo
    })
}

/// Every parent's roll-up in a loaded tree (`load_tree`): from the live leaves under it —
/// tasks and subtasks with no live children. Leaves are not in the map.
pub fn rollups(tree: &[PlanNode]) -> HashMap<String, Rollup> {
    let live: Vec<&PlanNode> = tree
        .iter()
        .filter(|n| n.status != NodeStatus::Dropped)
        .collect();
    let parent_of: HashMap<&str, Option<&str>> = live
        .iter()
        .map(|n| (n.id.as_str(), n.parent_id.as_deref()))
        .collect();
    let parents: HashSet<&str> = live.iter().filter_map(|n| n.parent_id.as_deref()).collect();
    let mut under: HashMap<&str, Vec<NodeStatus>> = HashMap::new();
    for leaf in live
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Task | NodeKind::Subtask))
        .filter(|n| !parents.contains(n.id.as_str()))
    {
        let mut up = leaf.parent_id.as_deref();
        let mut seen = HashSet::new();
        while let Some(p) = up {
            if !seen.insert(p) {
                break;
            }
            under.entry(p).or_default().push(leaf.status);
            up = parent_of.get(p).copied().flatten();
        }
    }
    under
        .into_iter()
        .filter_map(|(id, statuses)| rollup_of(statuses).map(|r| (id.to_string(), r)))
        .collect()
}

/// How a node's status reads: its roll-up for a parent, else its own status code.
pub fn shown_status<S: BuildHasher>(
    node: &PlanNode,
    rollups: &HashMap<String, Rollup, S>,
) -> &'static str {
    rollups
        .get(&node.id)
        .map_or_else(|| node.status.as_str(), |r| r.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Hill;

    fn n(id: &str, parent: Option<&str>, kind: NodeKind, status: NodeStatus) -> PlanNode {
        PlanNode {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            kind,
            title: id.into(),
            intent: None,
            estimate_min: None,
            status,
            hill: Some(Hill::Downhill),
            expected_files: vec![],
            depends_on: vec![],
            depth: 0,
        }
    }

    fn tree(tasks: &[(&str, NodeStatus)]) -> Vec<PlanNode> {
        let mut t = vec![
            n("G", None, NodeKind::Goal, NodeStatus::Todo),
            n("1", Some("G"), NodeKind::Milestone, NodeStatus::Todo),
            n("2", Some("G"), NodeKind::Milestone, NodeStatus::Todo),
        ];
        for (id, status) in tasks {
            let parent = id.split('.').next().unwrap();
            t.push(n(id, Some(parent), NodeKind::Task, *status));
        }
        t
    }

    #[test]
    fn parent_status_rolls_up() {
        use NodeStatus::{Active, Dropped, Failed, L2, L4, Todo};
        // All tasks at L4: the milestone and the goal are done.
        let t = tree(&[("1.1", L4), ("1.2", L4)]);
        let r = rollups(&t);
        assert_eq!(r.get("1"), Some(&Rollup::Done));
        assert_eq!(r.get("G"), Some(&Rollup::Done));
        // A milestone with no tasks keeps its own status, and is not in the map.
        assert_eq!(r.get("2"), None);
        assert_eq!(shown_status(&t[2], &r), "todo");
        assert_eq!(r.get("1.1"), None, "leaves keep their own status");
        // A new task: in progress again.
        let t = tree(&[("1.1", L4), ("1.2", L4), ("1.3", Todo)]);
        assert_eq!(rollups(&t).get("1"), Some(&Rollup::InProgress));
        // A failed task: needs attention, up to the goal.
        let t = tree(&[("1.1", L4), ("1.2", Failed), ("2.1", L2)]);
        let r = rollups(&t);
        assert_eq!(r.get("1"), Some(&Rollup::NeedsAttention));
        assert_eq!(r.get("2"), Some(&Rollup::Done));
        assert_eq!(r.get("G"), Some(&Rollup::NeedsAttention));
        // A dropped task is ignored.
        let t = tree(&[("1.1", L2), ("1.2", Dropped)]);
        assert_eq!(rollups(&t).get("1"), Some(&Rollup::Done));
        // Nothing started: to do; anything started: in progress.
        assert_eq!(rollup_of([Todo, Todo]), Some(Rollup::Todo));
        assert_eq!(rollup_of([Todo, Active]), Some(Rollup::InProgress));
        assert_eq!(rollup_of([Dropped]), None);
        assert_eq!(Rollup::NeedsAttention.words(), "needs attention");
    }
}
