//! Plan event payloads and the projector that applies them to `plan_nodes` / `plan_deps`.

use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use super::model::{Hill, NodeKind};
use crate::error::Result;
use crate::store::Event;

pub const PLAN_KINDS: &[&str] = &[
    "node_added",
    "node_edited",
    "node_moved",
    "node_removed",
    "dep_added",
    "dep_removed",
    "intent_set",
    "estimate_set",
    "hill_moved",
    "replan",
];

/// Why an edit happened; shared by every plan event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replan_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAdded {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: NodeKind,
    pub title: String,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub estimate_min: Option<u32>,
    #[serde(default)]
    pub expected_files: Vec<String>,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeEdited {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub expected_files: Option<Vec<String>>,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeMoved {
    pub id: String,
    pub new_parent_id: String,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRef {
    pub id: String,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepChange {
    pub id: String,
    pub depends_on: String,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSet {
    pub id: String,
    pub intent: String,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstimateSet {
    pub id: String,
    pub estimate_min: u32,
    #[serde(flatten)]
    pub meta: EditMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HillMoved {
    pub id: String,
    pub hill: Hill,
    #[serde(flatten)]
    pub meta: EditMeta,
}

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

fn next_position(tx: &Transaction, repo: &str, parent: Option<&str>) -> Result<i64> {
    Ok(tx.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM plan_nodes WHERE repo_id = ?1 AND parent_id IS ?2",
        params![repo, parent],
        |r| r.get(0),
    )?)
}

fn project_node_added(tx: &Transaction, repo: &str, d: &NodeAdded) -> Result<()> {
    let position = next_position(tx, repo, d.parent_id.as_deref())?;
    tx.execute(
        "INSERT INTO plan_nodes (repo_id, id, parent_id, kind, title, intent, estimate_min,
                                 expected_files, position)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            repo,
            d.id,
            d.parent_id,
            d.kind.as_str(),
            d.title,
            d.intent,
            d.estimate_min,
            serde_json::to_string(&d.expected_files)?,
            position
        ],
    )?;
    if let Some(parent) = &d.parent_id {
        tx.execute(
            "UPDATE plan_nodes SET next_child = next_child + 1 WHERE repo_id = ?1 AND id = ?2",
            params![repo, parent],
        )?;
    }
    Ok(())
}

fn project_node_removed(tx: &Transaction, repo: &str, d: &NodeRef) -> Result<()> {
    let subtree = "WITH RECURSIVE sub(id) AS (
                       SELECT ?2
                       UNION ALL
                       SELECT n.id FROM plan_nodes n JOIN sub ON n.parent_id = sub.id
                       WHERE n.repo_id = ?1)";
    tx.execute(
        &format!(
            "{subtree} UPDATE plan_nodes SET status = 'dropped'
             WHERE repo_id = ?1 AND id IN (SELECT id FROM sub)"
        ),
        params![repo, d.id],
    )?;
    tx.execute(
        &format!(
            "{subtree} DELETE FROM plan_deps WHERE repo_id = ?1
             AND (node_id IN (SELECT id FROM sub) OR depends_on IN (SELECT id FROM sub))"
        ),
        params![repo, d.id],
    )?;
    Ok(())
}

/// A claimed task starts being worked on: todo or failed → active. Verified and claimed-done
/// statuses (claimed, L1–L4) are left alone.
pub fn mark_claimed(tx: &Transaction, repo: &str, id: &str) -> Result<()> {
    tx.execute(
        "UPDATE plan_nodes SET status = 'active'
         WHERE repo_id = ?1 AND id = ?2 AND status IN ('todo', 'failed')",
        params![repo, id],
    )?;
    Ok(())
}

/// A released task goes back to todo, unless another live session still has it active.
pub fn mark_released(tx: &Transaction, repo: &str, id: &str) -> Result<()> {
    tx.execute(
        "UPDATE plan_nodes SET status = 'todo'
         WHERE repo_id = ?1 AND id = ?2 AND status = 'active'
           AND NOT EXISTS (SELECT 1 FROM sessions
                           WHERE repo_id = ?1 AND active_task = ?2 AND state != 'ended')",
        params![repo, id],
    )?;
    Ok(())
}

/// Apply one plan event to the plan views.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    let repo = event.repo_id.as_str();
    match event.kind.as_str() {
        "node_added" => project_node_added(tx, repo, &payload(event)?)?,
        "node_edited" => {
            let d: NodeEdited = payload(event)?;
            let files = d
                .expected_files
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            tx.execute(
                "UPDATE plan_nodes SET title = COALESCE(?3, title),
                                       expected_files = COALESCE(?4, expected_files)
                 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.title, files],
            )?;
        }
        "node_moved" => {
            let d: NodeMoved = payload(event)?;
            let position = next_position(tx, repo, Some(&d.new_parent_id))?;
            tx.execute(
                "UPDATE plan_nodes SET parent_id = ?3, position = ?4 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.new_parent_id, position],
            )?;
        }
        "node_removed" => project_node_removed(tx, repo, &payload(event)?)?,
        "dep_added" => {
            let d: DepChange = payload(event)?;
            tx.execute(
                "INSERT OR IGNORE INTO plan_deps (repo_id, node_id, depends_on) VALUES (?1, ?2, ?3)",
                params![repo, d.id, d.depends_on],
            )?;
        }
        "dep_removed" => {
            let d: DepChange = payload(event)?;
            tx.execute(
                "DELETE FROM plan_deps WHERE repo_id = ?1 AND node_id = ?2 AND depends_on = ?3",
                params![repo, d.id, d.depends_on],
            )?;
        }
        "intent_set" => {
            let d: IntentSet = payload(event)?;
            tx.execute(
                "UPDATE plan_nodes SET intent = ?3 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.intent],
            )?;
        }
        "estimate_set" => {
            let d: EstimateSet = payload(event)?;
            tx.execute(
                "UPDATE plan_nodes SET estimate_min = ?3 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.estimate_min],
            )?;
        }
        "hill_moved" => {
            let d: HillMoved = payload(event)?;
            tx.execute(
                "UPDATE plan_nodes SET hill = ?3 WHERE repo_id = ?1 AND id = ?2",
                params![repo, d.id, d.hill.as_str()],
            )?;
        }
        // `replan` only groups the edits that follow it; it changes no view.
        _ => {}
    }
    Ok(())
}
