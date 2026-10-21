//! Branch groups on the glance (spec §10.1): the same non-default branch with live
//! sessions in two or more repos is one piece of work, with a combined done%.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;

use super::{ProjectRow, leaves, pct};
use crate::error::Result;
use crate::plan::{NodeStatus, PlanNode, load_tree};

/// One repo in a branch group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupRepo {
    pub repo_id: String,
    pub name: String,
    /// `None` when no task in this repo was claimed on the branch yet.
    pub done_pct: Option<u32>,
    pub claimed_pct: Option<u32>,
    /// Live sessions on the branch.
    pub sessions: usize,
}

/// A branch with live sessions in two or more repos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchGroup {
    pub branch: String,
    pub repos: Vec<GroupRepo>,
    /// Over every repo's tasks on the branch, weighted by estimate; `None` before any.
    pub done_pct: Option<u32>,
    pub claimed_pct: Option<u32>,
}

/// A repo's default branch: `main`, `master`, and what `refs/remotes/origin/HEAD` points
/// to (read as a file under the main checkout's `.git`; no git process).
pub fn default_branches(root: &Path) -> Vec<String> {
    let mut out = vec!["main".to_string(), "master".to_string()];
    if let Ok(text) = std::fs::read_to_string(root.join(".git/refs/remotes/origin/HEAD"))
        && let Some(target) = text.trim().strip_prefix("ref:")
        && let Some(name) = target.trim().strip_prefix("refs/remotes/origin/")
    {
        out.push(name.to_string());
    }
    out
}

/// Estimate minutes (total, done, claimed) of a repo's leaves on `branch`. A leaf's branch
/// is its own, else its nearest ancestor's (a task claimed with its subtasks under it).
#[derive(Debug, Clone, Copy, Default)]
struct Minutes {
    total: u64,
    done: u64,
    claimed: u64,
    leaves: usize,
}

fn branch_of<'a>(n: &'a PlanNode, by_id: &HashMap<&str, &'a PlanNode>) -> Option<&'a str> {
    let mut cur = Some(n);
    while let Some(node) = cur {
        if let Some(b) = node.branch.as_deref() {
            return Some(b);
        }
        cur = node
            .parent_id
            .as_deref()
            .and_then(|p| by_id.get(p).copied());
    }
    None
}

fn minutes_on(tree: &[PlanNode], branch: &str) -> Minutes {
    let by_id: HashMap<&str, &PlanNode> = tree.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut m = Minutes::default();
    for leaf in leaves(tree) {
        if branch_of(leaf, &by_id) != Some(branch) {
            continue;
        }
        let min = u64::from(leaf.estimate_min.unwrap_or(0));
        m.leaves += 1;
        m.total += min;
        match leaf.status {
            NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4 => m.done += min,
            NodeStatus::Claimed | NodeStatus::L1 => m.claimed += min,
            _ => {}
        }
    }
    m
}

/// The glance's branch groups, sorted by branch name.
pub fn branch_groups(conn: &Connection, repos: &[ProjectRow]) -> Result<Vec<BranchGroup>> {
    // branch → repo index → live sessions on it.
    let mut seen: BTreeMap<String, BTreeMap<usize, usize>> = BTreeMap::new();
    for (i, p) in repos.iter().enumerate() {
        let root = crate::import::registered_root(conn, &p.repo_id)?;
        let defaults = root.as_deref().map(default_branches).unwrap_or_default();
        for s in &p.sessions {
            let Some(b) = s.branch.as_deref() else {
                continue;
            };
            if defaults.iter().any(|d| d == b) || b == "HEAD" || b.starts_with("detached at ") {
                continue;
            }
            *seen.entry(b.to_string()).or_default().entry(i).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for (branch, members) in seen.into_iter().filter(|(_, m)| m.len() >= 2) {
        let mut all = Minutes::default();
        let mut group_repos = Vec::new();
        for (i, sessions) in members {
            let p = &repos[i];
            let m = minutes_on(&load_tree(conn, &p.repo_id, false)?, &branch);
            all.total += m.total;
            all.done += m.done;
            all.claimed += m.claimed;
            all.leaves += m.leaves;
            let has = m.leaves > 0;
            group_repos.push(GroupRepo {
                repo_id: p.repo_id.clone(),
                name: p.name.clone(),
                done_pct: has.then(|| pct(m.done, m.total)),
                claimed_pct: has.then(|| pct(m.claimed, m.total)),
                sessions,
            });
        }
        group_repos.sort_by(|a, b| (&a.name, &a.repo_id).cmp(&(&b.name, &b.repo_id)));
        let has = all.leaves > 0;
        out.push(BranchGroup {
            branch,
            repos: group_repos,
            done_pct: has.then(|| pct(all.done, all.total)),
            claimed_pct: has.then(|| pct(all.claimed, all.total)),
        });
    }
    Ok(out)
}
