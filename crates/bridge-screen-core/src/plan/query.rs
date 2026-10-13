//! Read-side queries over the plan views.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::model::{Hill, NodeKind, NodeStatus, PlanNode};
use crate::error::Result;

const NODE_COLUMNS: &str =
    "id, parent_id, kind, title, intent, estimate_min, status, hill, expected_files, position";

struct RawNode {
    node: PlanNode,
    position: i64,
}

fn read_row(row: &Row) -> rusqlite::Result<(RawNode, String)> {
    let kind: String = row.get(2)?;
    let status: String = row.get(6)?;
    let hill: Option<String> = row.get(7)?;
    let files: String = row.get(8)?;
    let node = PlanNode {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        kind: NodeKind::parse(&kind).unwrap_or(NodeKind::Task),
        title: row.get(3)?,
        intent: row.get(4)?,
        estimate_min: row.get(5)?,
        status: NodeStatus::parse(&status).unwrap_or(NodeStatus::Todo),
        hill: hill.as_deref().and_then(Hill::parse),
        expected_files: Vec::new(),
        depends_on: Vec::new(),
        depth: 0,
    };
    Ok((
        RawNode {
            node,
            position: row.get(9)?,
        },
        files,
    ))
}

fn deps_map(conn: &Connection, repo_id: &str) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT node_id, depends_on FROM plan_deps WHERE repo_id = ?1 ORDER BY node_id, depends_on",
    )?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for row in stmt.query_map([repo_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (node, dep) = row?;
        map.entry(node).or_default().push(dep);
    }
    Ok(map)
}

/// One node with its dependencies, or `None` if it doesn't exist.
pub fn get_node(conn: &Connection, repo_id: &str, id: &str) -> Result<Option<PlanNode>> {
    let found = conn
        .query_row(
            &format!("SELECT {NODE_COLUMNS} FROM plan_nodes WHERE repo_id = ?1 AND id = ?2"),
            params![repo_id, id],
            read_row,
        )
        .optional()?;
    let Some((raw, files)) = found else {
        return Ok(None);
    };
    let mut node = raw.node;
    node.expected_files = serde_json::from_str(&files)?;
    node.depends_on = deps_map(conn, repo_id)?.remove(id).unwrap_or_default();
    Ok(Some(node))
}

/// The whole tree, depth-first, siblings in position order.
pub fn load_tree(conn: &Connection, repo_id: &str, include_dropped: bool) -> Result<Vec<PlanNode>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {NODE_COLUMNS} FROM plan_nodes WHERE repo_id = ?1"
    ))?;
    let mut deps = deps_map(conn, repo_id)?;
    let mut children: BTreeMap<Option<String>, Vec<RawNode>> = BTreeMap::new();
    for row in stmt.query_map([repo_id], read_row)? {
        let (mut raw, files) = row?;
        if !include_dropped && raw.node.status == NodeStatus::Dropped {
            continue;
        }
        raw.node.expected_files = serde_json::from_str(&files)?;
        raw.node.depends_on = deps.remove(&raw.node.id).unwrap_or_default();
        children
            .entry(raw.node.parent_id.clone())
            .or_default()
            .push(raw);
    }
    for list in children.values_mut() {
        list.sort_by_key(|r| r.position);
    }
    let mut out = Vec::new();
    let mut stack: Vec<(PlanNode, usize)> = children
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .map(|r| (r.node, 0))
        .collect();
    while let Some((mut node, depth)) = stack.pop() {
        node.depth = depth;
        if let Some(kids) = children.remove(&Some(node.id.clone())) {
            stack.extend(kids.into_iter().rev().map(|r| (r.node, depth + 1)));
        }
        out.push(node);
    }
    Ok(out)
}

/// The number the next child of `id` will get.
pub fn next_child(conn: &Connection, repo_id: &str, id: &str) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT next_child FROM plan_nodes WHERE repo_id = ?1 AND id = ?2",
        params![repo_id, id],
        |r| r.get(0),
    )?)
}

/// Would adding "`id` depends on `depends_on`" create a cycle (including a self-loop)?
pub fn would_cycle(conn: &Connection, repo_id: &str, id: &str, depends_on: &str) -> Result<bool> {
    let reaches: i64 = conn.query_row(
        "WITH RECURSIVE reach(n) AS (
             SELECT ?3
             UNION
             SELECT d.depends_on FROM plan_deps d JOIN reach ON d.node_id = reach.n
             WHERE d.repo_id = ?1)
         SELECT count(*) FROM reach WHERE n = ?2",
        params![repo_id, id, depends_on],
        |r| r.get(0),
    )?;
    Ok(reaches > 0)
}

/// Is `candidate` equal to `root` or one of its descendants?
pub fn is_in_subtree(
    conn: &Connection,
    repo_id: &str,
    root: &str,
    candidate: &str,
) -> Result<bool> {
    let found: i64 = conn.query_row(
        "WITH RECURSIVE sub(id) AS (
             SELECT ?2
             UNION ALL
             SELECT n.id FROM plan_nodes n JOIN sub ON n.parent_id = sub.id WHERE n.repo_id = ?1)
         SELECT count(*) FROM sub WHERE id = ?3",
        params![repo_id, root, candidate],
        |r| r.get(0),
    )?;
    Ok(found > 0)
}
