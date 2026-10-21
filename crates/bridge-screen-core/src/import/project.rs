//! Import event payloads and the projector that keeps `plan_sources` and
//! `plan_source_items` up to date. Rebuild needs no file: everything is in the events.

use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::plan::NodeSource;
use crate::plan::project::NodeAdded;
use crate::store::Event;

/// Event kinds routed to this projector.
pub const IMPORT_KINDS: &[&str] = &["plan_imported", "plan_file_seen"];

/// One task (`step` absent) or step of an imported file and the node it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedItem {
    pub task: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    pub node_id: String,
    pub title: String,
}

/// A plan file was imported: its tasks and steps are now these nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanImported {
    /// Repo-relative path, `/`-separated.
    pub path: String,
    pub hash: String,
    pub size: u64,
    /// Modification time, milliseconds since the epoch.
    pub mtime: i64,
    /// The goal or milestone the file's tasks are under.
    pub node_id: String,
    pub tasks: usize,
    pub changed_ratio: f64,
    pub replan: bool,
    /// Every task and step of the file after this import (replaces the earlier set).
    #[serde(default)]
    pub items: Vec<ImportedItem>,
}

/// A plan file was looked at without importing: unchanged, a baseline (decision 1: a
/// repo with a hand-made plan records its existing files instead of importing them), or
/// a file without tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanFileSeen {
    pub path: String,
    /// Empty when the file was only stat'ed (a baseline over the read budget).
    pub hash: String,
    pub size: u64,
    pub mtime: i64,
    #[serde(default)]
    pub baseline: bool,
}

fn payload<T: serde::de::DeserializeOwned>(event: &Event) -> Result<T> {
    Ok(serde_json::from_value(event.data.clone())?)
}

fn i64_of(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// A `node_added` from a plan file links that task or step to the file.
pub fn node_added(tx: &Transaction, repo: &str, d: &NodeAdded) -> Result<()> {
    if let Some(NodeSource::File { file, task, step }) = &d.source {
        tx.execute(
            "INSERT OR REPLACE INTO plan_source_items (repo_id, path, task_no, step_no, node_id,
                                                     title)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![repo, file, task, step.unwrap_or(0), d.id, d.title],
        )?;
    }
    Ok(())
}

/// Apply one import event to the import views.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    let repo = event.repo_id.as_str();
    match event.kind.as_str() {
        "plan_imported" => {
            let d: PlanImported = payload(event)?;
            tx.execute(
                "INSERT INTO plan_sources (repo_id, path, hash, size, mtime, node_id, baseline,
                                           imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)
                 ON CONFLICT(repo_id, path) DO UPDATE SET
                     hash = excluded.hash, size = excluded.size, mtime = excluded.mtime,
                     node_id = excluded.node_id, baseline = 0, imported_at = excluded.imported_at",
                params![
                    repo,
                    d.path,
                    d.hash,
                    i64_of(d.size),
                    d.mtime,
                    d.node_id,
                    event.ts
                ],
            )?;
            tx.execute(
                "DELETE FROM plan_source_items WHERE repo_id = ?1 AND path = ?2",
                params![repo, d.path],
            )?;
            for item in &d.items {
                tx.execute(
                    "INSERT OR REPLACE INTO plan_source_items (repo_id, path, task_no, step_no,
                                                             node_id, title)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        repo,
                        d.path,
                        item.task,
                        item.step.unwrap_or(0),
                        item.node_id,
                        item.title
                    ],
                )?;
                let source = NodeSource::File {
                    file: d.path.clone(),
                    task: item.task,
                    step: item.step,
                };
                tx.execute(
                    "UPDATE plan_nodes SET source = ?3 WHERE repo_id = ?1 AND id = ?2",
                    params![repo, item.node_id, source.to_column()],
                )?;
            }
        }
        "plan_file_seen" => {
            let d: PlanFileSeen = payload(event)?;
            tx.execute(
                "INSERT INTO plan_sources (repo_id, path, hash, size, mtime, node_id, baseline,
                                           imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)
                 ON CONFLICT(repo_id, path) DO UPDATE SET
                     hash = excluded.hash, size = excluded.size, mtime = excluded.mtime,
                     baseline = excluded.baseline",
                params![
                    repo,
                    d.path,
                    d.hash,
                    i64_of(d.size),
                    d.mtime,
                    d.baseline,
                    event.ts
                ],
            )?;
        }
        _ => {}
    }
    Ok(())
}
