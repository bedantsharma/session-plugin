//! Importing superpowers plan files into the plan (spec §12.1). Files are only read, never
//! written. A file's tasks become plan nodes through `plan_edit` (one batch per file,
//! grouped under a re-plan when more than 30% of its tasks changed); a ticked step is
//! recorded as claimed done, never verified.
//!
//! SessionStart runs [`scan`] with a small budget (file reads only, no git); the CLI
//! `bridge-screen import` runs it without the file limit, or [`import_path`] for one file.

pub mod diff;
pub mod find;
pub mod parse;
pub mod project;
pub mod scope;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::checkpoint::{ClaimEvidence, ClaimSource, claimed_done_draft};
use crate::error::{Error, Result};
use crate::plan::project::{EditMeta, NodeEdited};
use crate::plan::{
    EditContext, NodeKind, NodeStatus, PlanOp, get_node, load_tree, next_child,
    plan_edit_in_sourced,
};
use crate::store::{Actor, EventDraft, Store, append_in};

pub use diff::{ImportDiff, ImportTarget, ItemLink, SourceItem};
pub use find::{PLANS_DIR, plan_files, spec_files};
pub use parse::{ParsedPlan, ParsedStep, ParsedTask, parse};
pub use project::{IMPORT_KINDS, ImportedItem, PlanFileSeen, PlanImported, project};

/// Largest plan file read.
pub const MAX_FILE_BYTES: u64 = 1 << 20;

/// Who imports, and whether to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCtx {
    /// The SessionStart session, or `owner` from the CLI.
    pub session_id: String,
    pub actor: Actor,
    /// Work out the changes and roll them back.
    pub dry_run: bool,
}

/// How much one scan may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanBudget {
    pub files: usize,
    pub bytes: u64,
}

impl ScanBudget {
    /// SessionStart: at most 20 files and 1 MiB read.
    pub const SESSION_START: Self = Self {
        files: 20,
        bytes: 1 << 20,
    };
    /// The CLI: no file limit (each file still at most 1 MiB).
    pub const UNLIMITED: Self = Self {
        files: usize::MAX,
        bytes: u64::MAX,
    };
}

/// What happened to one plan file.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ImportOutcome {
    Imported {
        path: String,
        node_id: String,
        added: usize,
        edited: usize,
        removed: usize,
        kept: usize,
        claimed: usize,
        replan: bool,
        /// Plain-words lines for each change (shown by `--dry-run`).
        changes: Vec<String>,
    },
    Unchanged {
        path: String,
    },
    Baseline {
        path: String,
    },
    Skipped {
        path: String,
        why: String,
    },
    /// A file first seen as already planned by hand has changed, and no goal or milestone
    /// has its title: nothing was created (it would duplicate the hand-made plan). It
    /// waits for `bridge-screen import --into <milestone> <file>`.
    NeedsLink {
        path: String,
    },
    /// A finished run was closed because this file's plan arrived; its progress starts
    /// from 0% (spec §10.1).
    RunClosed {
        number: u32,
        tasks: usize,
    },
    /// Plan files were found but nothing says what is finished: nothing was imported; the
    /// owner is asked once ([`scope`]).
    NeedsScope {
        paths: Vec<String>,
    },
    /// Something went wrong importing this file; nothing of it was written.
    Failed {
        path: String,
        error: String,
    },
}

impl ImportOutcome {
    pub fn path(&self) -> &str {
        match self {
            Self::Imported { path, .. }
            | Self::Unchanged { path }
            | Self::Baseline { path }
            | Self::Skipped { path, .. }
            | Self::NeedsLink { path }
            | Self::Failed { path, .. } => path,
            Self::NeedsScope { paths } => paths.first().map_or("", String::as_str),
            Self::RunClosed { .. } => "",
        }
    }

    /// One line in plain words, or `None` when there is nothing worth saying.
    pub fn summary(&self) -> Option<String> {
        match self {
            Self::Imported {
                path,
                added,
                edited,
                removed,
                kept,
                claimed,
                ..
            } => {
                let mut parts = Vec::new();
                if *added > 0 {
                    parts.push(format!("added {added}"));
                }
                if *edited > 0 {
                    parts.push(format!("changed {edited}"));
                }
                if *removed > 0 {
                    parts.push(format!("removed {removed}"));
                }
                if *kept > 0 {
                    parts.push(format!(
                        "kept {kept} no longer in the file because work was done on them"
                    ));
                }
                let mut line = if parts.is_empty() {
                    format!("Imported {}: nothing changed", short_name(path))
                } else {
                    format!(
                        "Imported {}: {} tasks and steps",
                        short_name(path),
                        parts.join(", ")
                    )
                };
                if *claimed > 0 {
                    let _ = write!(
                        line,
                        "; {claimed} ticked step{} recorded as claimed (not checked)",
                        if *claimed == 1 { "" } else { "s" }
                    );
                }
                Some(line)
            }
            Self::Baseline { path } => Some(format!(
                "Noted {} as already planned by hand (not imported)",
                short_name(path)
            )),
            Self::Skipped { path, why } => Some(format!("Skipped {}: {why}", short_name(path))),
            Self::NeedsLink { path } => Some(needs_link_line(path)),
            Self::NeedsScope { paths } => Some(scope::question_text(paths)),
            Self::RunClosed { number, tasks } => Some(format!(
                "Run {number} is finished ({tasks} task{}, all passed their tests) and was closed; \
                 the new plan starts a fresh run from 0%.",
                if *tasks == 1 { "" } else { "s" }
            )),
            Self::Failed { path, error } => {
                Some(format!("Could not import {}: {error}", short_name(path)))
            }
            Self::Unchanged { .. } => None,
        }
    }
}

/// What to tell the owner about a changed plan file that matches no milestone.
pub fn needs_link_line(path: &str) -> String {
    format!(
        "{} changed but isn't linked to a milestone, so nothing was imported; run \
         `bridge-screen import --into <milestone> {path}` to link it.",
        short_name(path)
    )
}

/// `docs/superpowers/plans/2026-10-21-sp8-x.md` → `2026-10-21-sp8-x`.
pub fn short_name(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name)
}

/// A `plan_sources` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanSource {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub mtime: i64,
    pub node_id: Option<String>,
    pub baseline: bool,
    pub imported_at: String,
    /// Changed since it was baselined and matches no milestone (see
    /// [`ImportOutcome::NeedsLink`]).
    pub needs_link: bool,
}

/// Every plan file this repo has imported or seen, by path.
pub fn sources(conn: &Connection, repo_id: &str) -> Result<Vec<PlanSource>> {
    let mut stmt = conn.prepare_cached(
        "SELECT path, hash, size, mtime, node_id, baseline, imported_at, needs_link
         FROM plan_sources
         WHERE repo_id = ?1 ORDER BY path",
    )?;
    let rows = stmt.query_map([repo_id], |r| {
        Ok(PlanSource {
            path: r.get(0)?,
            hash: r.get(1)?,
            size: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
            mtime: r.get(3)?,
            node_id: r.get(4)?,
            baseline: r.get(5)?,
            imported_at: r.get(6)?,
            needs_link: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn source(conn: &Connection, repo_id: &str, path: &str) -> Result<Option<PlanSource>> {
    Ok(sources(conn, repo_id)?.into_iter().find(|s| s.path == path))
}

/// A file's `plan_source_items` with their nodes' state.
pub fn source_items(conn: &Connection, repo_id: &str, path: &str) -> Result<Vec<SourceItem>> {
    let mut stmt = conn.prepare_cached(
        "SELECT i.task_no, i.step_no, i.node_id, i.title, n.status, n.expected_files,
                n.next_child
         FROM plan_source_items i JOIN plan_nodes n ON n.repo_id = i.repo_id AND n.id = i.node_id
         WHERE i.repo_id = ?1 AND i.path = ?2 ORDER BY i.task_no, i.step_no",
    )?;
    let rows = stmt.query_map(params![repo_id, path], |r| {
        let step: u32 = r.get(1)?;
        let status: String = r.get(4)?;
        let files: String = r.get(5)?;
        Ok(SourceItem {
            task_no: r.get(0)?,
            step_no: (step != 0).then_some(step),
            node_id: r.get(2)?,
            title: r.get(3)?,
            status: NodeStatus::parse(&status).unwrap_or(NodeStatus::Todo),
            files: serde_json::from_str(&files).unwrap_or_default(),
            next_child: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// What a file looks like on disk.
struct FileFacts {
    rel: String,
    size: u64,
    mtime: i64,
}

fn facts(root: &Path, path: &Path) -> Result<FileFacts> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Ok(FileFacts {
        rel: relative(root, path),
        size: meta.len(),
        mtime,
    })
}

/// `path` relative to `root`, `/`-separated (the path itself when outside `root`).
fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn seen_draft(repo_id: &str, ctx: &ImportCtx, d: &PlanFileSeen) -> Result<EventDraft> {
    Ok(EventDraft::new(
        repo_id,
        &ctx.session_id,
        "plan_file_seen",
        serde_json::to_value(d)?,
    )
    .actor(ctx.actor.clone()))
}

/// The goal, if the repo has a live one.
fn live_goal(conn: &Connection, repo_id: &str) -> Result<Option<crate::plan::PlanNode>> {
    Ok(get_node(conn, repo_id, "G")?.filter(|g| g.status != NodeStatus::Dropped))
}

/// Does the repo have any live plan node?
fn has_plan(conn: &Connection, repo_id: &str) -> Result<bool> {
    let n: i64 = conn
        .prepare_cached(
            "SELECT count(*) FROM plan_nodes WHERE repo_id = ?1 AND status != 'dropped'",
        )?
        .query_row([repo_id], |r| r.get(0))?;
    Ok(n > 0)
}

fn file_stem(rel: &str) -> String {
    parse::one_line(short_name(rel))
}

/// A live goal or milestone of the current run whose title is `title` (case-insensitive);
/// a closed run is never reopened by a new file.
fn container_titled(conn: &Connection, repo_id: &str, title: &str) -> Result<Option<String>> {
    Ok(crate::plan::runs::run_tree(conn, repo_id)?
        .into_iter()
        .find(|n| {
            matches!(n.kind, NodeKind::Goal | NodeKind::Milestone)
                && n.title.trim().eq_ignore_ascii_case(title.trim())
        })
        .map(|n| n.id))
}

/// Link a file to an existing goal or milestone by hand (`--into`): its children are
/// matched to the file's tasks by title (case-insensitive), their children to the steps.
fn items_by_title(
    conn: &Connection,
    repo_id: &str,
    into: &str,
    parsed: &ParsedPlan,
) -> Result<Vec<SourceItem>> {
    let tree = load_tree(conn, repo_id, false)?;
    let mut out = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for t in &parsed.tasks {
        let Some(node) = tree.iter().find(|n| {
            n.parent_id.as_deref() == Some(into)
                && !used.contains(&n.id)
                && n.title.trim().eq_ignore_ascii_case(&t.title)
        }) else {
            continue;
        };
        used.push(node.id.clone());
        let next = next_child(conn, repo_id, &node.id)?;
        out.push(SourceItem {
            task_no: t.number,
            step_no: None,
            node_id: node.id.clone(),
            title: t.title.clone(),
            status: node.status,
            files: node.expected_files.clone(),
            next_child: next,
        });
        for s in &t.steps {
            if let Some(sub) = tree.iter().find(|n| {
                n.parent_id.as_deref() == Some(node.id.as_str())
                    && !used.contains(&n.id)
                    && n.title.trim().eq_ignore_ascii_case(&s.text)
            }) {
                used.push(sub.id.clone());
                out.push(SourceItem {
                    task_no: t.number,
                    step_no: Some(s.number),
                    node_id: sub.id.clone(),
                    title: s.text.clone(),
                    status: sub.status,
                    files: sub.expected_files.clone(),
                    next_child: next_child(conn, repo_id, &sub.id)?,
                });
            }
        }
    }
    Ok(out)
}

/// Where the file's tasks go, and what its last import produced.
fn target_for(
    tx: &Transaction,
    repo_id: &str,
    rel: &str,
    parsed: &ParsedPlan,
    linked: Option<&str>,
    into: Option<&str>,
) -> Result<(ImportTarget, Vec<SourceItem>)> {
    let title = parsed.title.clone().unwrap_or_else(|| file_stem(rel));
    let intent = parsed
        .goal
        .clone()
        .unwrap_or_else(|| format!("Imported from {rel}"));
    let existing = |id: &str, update_intent: bool| -> Result<ImportTarget> {
        let node = get_node(tx, repo_id, id)?
            .filter(|n| n.status != NodeStatus::Dropped)
            .ok_or_else(|| Error::NotFound(format!("no plan node {id}")))?;
        if !matches!(node.kind, NodeKind::Goal | NodeKind::Milestone) {
            return Err(Error::Invalid(format!(
                "{id} is a {}; link a plan file to the goal or a milestone",
                node.kind.as_str()
            )));
        }
        Ok(ImportTarget::Existing {
            node_id: node.id.clone(),
            next_child: next_child(tx, repo_id, &node.id)?,
            intent: node.intent,
            update_intent,
        })
    };
    if let Some(into) = into {
        let items = items_by_title(tx, repo_id, into, parsed)?;
        return Ok((existing(into, false)?, items));
    }
    if let Some(id) = linked
        && get_node(tx, repo_id, id)?.is_some_and(|n| n.status != NodeStatus::Dropped)
    {
        return Ok((existing(id, true)?, source_items(tx, repo_id, rel)?));
    }
    let Some(goal) = live_goal(tx, repo_id)? else {
        return Ok((ImportTarget::NewGoal { title, intent }, Vec::new()));
    };
    // A file seen before as already planned by hand, now changed: when a goal or
    // milestone has the file's title, the file belongs to it (matched by title, like
    // `--into`) instead of becoming a duplicate milestone.
    if let Some(id) = container_titled(tx, repo_id, &title)? {
        let items = items_by_title(tx, repo_id, &id, parsed)?;
        return Ok((existing(&id, false)?, items));
    }
    let id = next_child(tx, repo_id, &goal.id)?.to_string();
    Ok((
        ImportTarget::NewMilestone {
            goal_id: goal.id,
            id,
            title,
            intent,
        },
        Vec::new(),
    ))
}

fn describe(op: &PlanOp) -> String {
    match op {
        PlanOp::AddNode { kind, title, .. } => format!("add {}: {title}", kind.as_str()),
        PlanOp::EditNode { id, title, .. } => match title {
            Some(t) => format!("rename {id} to: {t}"),
            None => format!("update {id}'s files"),
        },
        PlanOp::RemoveNode { id } => format!("remove {id} (nothing was done on it)"),
        PlanOp::SetIntent { id, .. } => format!("update {id}'s purpose from the file's goal"),
        other => format!("{other:?}"),
    }
}

/// Import one file inside `tx`. `into` links the file to that goal or milestone by hand.
fn import_in(
    tx: &Transaction,
    repo_id: &str,
    root: &Path,
    path: &Path,
    ctx: &ImportCtx,
    into: Option<&str>,
) -> Result<ImportOutcome> {
    let f = facts(root, path)?;
    let rel = f.rel.clone();
    if f.size > MAX_FILE_BYTES {
        return Ok(ImportOutcome::Skipped {
            path: rel,
            why: "larger than 1 MiB".into(),
        });
    }
    let bytes = std::fs::read(path)?;
    let hash = sha256_hex(&bytes);
    let prev = source(tx, repo_id, &rel)?;
    // A file planned by hand and never linked stays a baseline file until it is linked.
    let hand_planned = prev
        .as_ref()
        .is_some_and(|p| p.baseline && p.node_id.is_none());
    let seen = |baseline: bool, hash: &str| PlanFileSeen {
        path: rel.clone(),
        hash: hash.to_string(),
        size: f.size,
        mtime: f.mtime,
        baseline,
        needs_link: false,
    };
    if into.is_none()
        && let Some(p) = &prev
    {
        if p.hash == hash {
            // Same content (perhaps a new mtime): nothing is written.
            return Ok(ImportOutcome::Unchanged { path: rel });
        }
        if p.baseline && p.hash.is_empty() {
            // A baseline file that was only stat'ed: finish its baseline.
            append_in(tx, seen_draft(repo_id, ctx, &seen(true, &hash))?)?;
            return Ok(ImportOutcome::Baseline { path: rel });
        }
    }
    let Ok(text) = String::from_utf8(bytes) else {
        append_in(tx, seen_draft(repo_id, ctx, &seen(hand_planned, &hash))?)?;
        return Ok(ImportOutcome::Skipped {
            path: rel,
            why: "not a text file".into(),
        });
    };
    let parsed = parse(&text);
    if parsed.tasks.is_empty() {
        append_in(tx, seen_draft(repo_id, ctx, &seen(hand_planned, &hash))?)?;
        return Ok(ImportOutcome::Skipped {
            path: rel,
            why: "no \"### Task N:\" headings".into(),
        });
    }
    if hand_planned
        && into.is_none()
        && container_titled(
            tx,
            repo_id,
            &parsed.title.clone().unwrap_or_else(|| file_stem(&rel)),
        )?
        .is_none()
    {
        // Edited since the hand-made plan was baselined, and no milestone has its title:
        // importing would add a duplicate milestone. Record it as needing a link.
        let mut d = seen(true, &hash);
        d.needs_link = true;
        append_in(tx, seen_draft(repo_id, ctx, &d)?)?;
        return Ok(ImportOutcome::NeedsLink { path: rel });
    }
    let linked = prev.as_ref().and_then(|p| p.node_id.clone());
    let file = ReadFile {
        rel,
        hash,
        size: f.size,
        mtime: f.mtime,
    };
    apply_import(tx, repo_id, &file, &parsed, linked.as_deref(), ctx, into)
}

/// A plan file read for import.
struct ReadFile {
    rel: String,
    hash: String,
    size: u64,
    mtime: i64,
}

/// Turn a parsed file into plan events: the diff through `plan_edit`, the keeps, the
/// ticked steps as claims, then `plan_imported`.
fn apply_import(
    tx: &Transaction,
    repo_id: &str,
    file: &ReadFile,
    parsed: &ParsedPlan,
    linked: Option<&str>,
    ctx: &ImportCtx,
    into: Option<&str>,
) -> Result<ImportOutcome> {
    let rel = file.rel.clone();
    let (target, items) = target_for(tx, repo_id, &rel, parsed, linked, into)?;
    let d = diff::diff(&items, parsed, &target, &rel);
    let node_id = match &target {
        ImportTarget::NewGoal { .. } => "G".to_string(),
        ImportTarget::NewMilestone { id, .. } => id.clone(),
        ImportTarget::Existing { node_id, .. } => node_id.clone(),
    };
    let mut changes: Vec<String> = d.ops.iter().map(describe).collect();
    let edit = EditContext {
        repo_id: repo_id.to_string(),
        session_id: ctx.session_id.clone(),
        actor: ctx.actor.clone(),
        reason: Some(format!("imported from {rel}")),
        replan: d.replan,
    };
    if !d.ops.is_empty() {
        let out = plan_edit_in_sourced(tx, &edit, &d.ops, &d.sources)?;
        if out.added_ids != d.added {
            return Err(Error::Invalid(format!(
                "the plan changed while importing {rel}; try again"
            )));
        }
    }
    for (id, reason) in &d.keeps {
        if let Some(node) = get_node(tx, repo_id, id)? {
            let data = NodeEdited {
                id: id.clone(),
                title: None,
                expected_files: Some(node.expected_files),
                meta: EditMeta {
                    reason: Some(reason.clone()),
                    replan_id: None,
                },
            };
            append_in(
                tx,
                EventDraft::new(
                    repo_id,
                    &ctx.session_id,
                    "node_edited",
                    serde_json::to_value(data)?,
                )
                .actor(ctx.actor.clone())
                .task(id),
            )?;
            changes.push(format!("keep {id}: {reason}, and work was done on it"));
        }
    }
    record_claims(tx, repo_id, ctx, &rel, &d.claims, &mut changes)?;
    let data = PlanImported {
        path: rel.clone(),
        hash: file.hash.clone(),
        size: file.size,
        mtime: file.mtime,
        node_id: node_id.clone(),
        tasks: parsed.tasks.len(),
        changed_ratio: d.changed_ratio,
        replan: d.replan,
        items: d
            .items
            .iter()
            .map(|i| ImportedItem {
                task: i.task_no,
                step: i.step_no,
                node_id: i.node_id.clone(),
                title: i.title.clone(),
            })
            .collect(),
    };
    append_in(
        tx,
        EventDraft::new(
            repo_id,
            &ctx.session_id,
            "plan_imported",
            serde_json::to_value(data)?,
        )
        .actor(ctx.actor.clone()),
    )?;
    let count = |p: fn(&PlanOp) -> bool| d.ops.iter().filter(|o| p(o)).count();
    Ok(ImportOutcome::Imported {
        path: rel,
        node_id,
        added: count(|o| {
            matches!(
                o,
                PlanOp::AddNode {
                    kind: NodeKind::Task | NodeKind::Subtask,
                    ..
                }
            )
        }),
        edited: count(|o| matches!(o, PlanOp::EditNode { .. })),
        removed: count(|o| matches!(o, PlanOp::RemoveNode { .. })),
        kept: d.keeps.len(),
        claimed: d.claims.len(),
        replan: d.replan,
        changes,
    })
}

/// Record each ticked step as claimed done: inferred, so no decision ask, no checkpoint
/// request and no files (it stays claimed until someone checks it).
fn record_claims(
    tx: &Transaction,
    repo_id: &str,
    ctx: &ImportCtx,
    rel: &str,
    claims: &[String],
    changes: &mut Vec<String>,
) -> Result<()> {
    let by = ClaimSource {
        repo_id,
        session_id: &ctx.session_id,
        actor: &ctx.actor,
        inferred: true,
    };
    let evidence = format!("ticked in {rel}");
    for id in claims {
        let draft = claimed_done_draft(
            tx,
            &by,
            id,
            &ClaimEvidence {
                evidence: &evidence,
                files: &[],
                symbols: &[],
            },
        )?;
        append_in(tx, draft)?;
        changes.push(format!("record {id} as claimed done (ticked in the file)"));
    }
    Ok(())
}

/// Run `f` in a transaction: committed, or rolled back for a dry run.
fn run<T>(
    store: &mut Store,
    dry_run: bool,
    f: impl FnOnce(&Transaction) -> Result<T>,
) -> Result<T> {
    if dry_run {
        store.dry_run(f)
    } else {
        store.write(f)
    }
}

/// Import one plan file. `into` links it by hand to an existing goal or milestone (tasks
/// matched by title). `root` is the checkout the path is relative to.
pub fn import_path(
    store: &mut Store,
    repo_id: &str,
    root: &Path,
    path: &Path,
    ctx: &ImportCtx,
    into: Option<&str>,
) -> Result<ImportOutcome> {
    run(store, ctx.dry_run, |tx| {
        import_in(tx, repo_id, root, path, ctx, into)
    })
}

/// Check the plan folder and import new or changed files (spec §12.1). Files whose size and
/// mtime match their last import are skipped unread; the rest are read until the budget is
/// spent (what's left waits for the next scan). A repo with a hand-made plan that never
/// imported records a baseline of its existing files instead (decision 1).
pub fn scan(
    store: &mut Store,
    repo_id: &str,
    root: &Path,
    ctx: &ImportCtx,
    budget: ScanBudget,
) -> Result<Vec<ImportOutcome>> {
    let files = plan_files(root)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(asked) = scope::gate(store, repo_id, root, &files, ctx, budget)? {
        return Ok(vec![asked]);
    }
    let known = sources(store.conn(), repo_id)?;
    if known.is_empty() && has_plan(store.conn(), repo_id)? {
        return run(store, ctx.dry_run, |tx| {
            baseline(tx, repo_id, root, &files, ctx, budget)
        });
    }
    let mut out = Vec::new();
    let mut due = Vec::new();
    let mut read: u64 = 0;
    // Unchanged files only cost a stat; the file budget counts the ones to read, so a
    // folder with more files than the budget is worked through over several scans.
    for path in files {
        if due.len() >= budget.files {
            break;
        }
        let f = match facts(root, &path) {
            Ok(f) => f,
            Err(e) => {
                out.push(ImportOutcome::Failed {
                    path: relative(root, &path),
                    error: e.to_string(),
                });
                continue;
            }
        };
        if known.iter().any(|k| {
            k.path == f.rel && k.size == f.size && k.mtime == f.mtime && !k.hash.is_empty()
        }) {
            continue;
        }
        let cost = f.size.min(MAX_FILE_BYTES);
        if read.saturating_add(cost) > budget.bytes {
            break;
        }
        read = read.saturating_add(cost);
        due.push((path, f.rel));
    }
    if ctx.dry_run {
        // One rolled-back transaction, so each file sees the ones before it.
        return store.dry_run(|tx| {
            for (path, rel) in due {
                tx.execute_batch("SAVEPOINT import_file")?;
                let before = closed_count(tx, repo_id)?;
                match import_in(tx, repo_id, root, &path, ctx, None) {
                    Ok(o) => {
                        tx.execute_batch("RELEASE import_file")?;
                        out.extend(run_closed_note(tx, repo_id, before)?);
                        out.push(o);
                    }
                    Err(e) => {
                        tx.execute_batch("ROLLBACK TO import_file; RELEASE import_file")?;
                        out.push(ImportOutcome::Failed {
                            path: rel,
                            error: e.to_string(),
                        });
                    }
                }
            }
            Ok(out)
        });
    }
    for (path, rel) in due {
        let before = closed_count(store.conn(), repo_id)?;
        match import_path(store, repo_id, root, &path, ctx, None) {
            Ok(o) => {
                out.extend(run_closed_note(store.conn(), repo_id, before)?);
                out.push(o);
            }
            Err(e) => out.push(ImportOutcome::Failed {
                path: rel,
                error: e.to_string(),
            }),
        }
    }
    Ok(out)
}

fn closed_count(conn: &Connection, repo_id: &str) -> Result<usize> {
    Ok(crate::plan::runs::load(conn, repo_id)?.closed.len())
}

/// A note when the import just closed the finished run.
fn run_closed_note(
    conn: &Connection,
    repo_id: &str,
    before: usize,
) -> Result<Option<ImportOutcome>> {
    let runs = crate::plan::runs::load(conn, repo_id)?;
    if runs.closed.len() <= before {
        return Ok(None);
    }
    let Some(last) = runs.closed.last() else {
        return Ok(None);
    };
    Ok(Some(ImportOutcome::RunClosed {
        number: last.number,
        tasks: crate::plan::runs::tasks_in(conn, repo_id, last)?,
    }))
}

/// Record every existing plan file as already planned by hand. Files over the read budget
/// are only stat'ed (empty hash) and hashed on a later scan.
fn baseline(
    tx: &Transaction,
    repo_id: &str,
    root: &Path,
    files: &[PathBuf],
    ctx: &ImportCtx,
    budget: ScanBudget,
) -> Result<Vec<ImportOutcome>> {
    let mut out = Vec::new();
    let mut read: u64 = 0;
    for (i, path) in files.iter().enumerate() {
        let Ok(f) = facts(root, path) else {
            continue;
        };
        let readable = i < budget.files
            && f.size <= MAX_FILE_BYTES
            && read.saturating_add(f.size) <= budget.bytes;
        let hash = if readable {
            read = read.saturating_add(f.size);
            std::fs::read(path)
                .map(|b| sha256_hex(&b))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let seen = PlanFileSeen {
            path: f.rel.clone(),
            hash,
            size: f.size,
            mtime: f.mtime,
            baseline: true,
            needs_link: false,
        };
        append_in(tx, seen_draft(repo_id, ctx, &seen)?)?;
        out.push(ImportOutcome::Baseline { path: f.rel });
    }
    Ok(out)
}

/// The repo root recorded for `repo_id`, if registered.
pub fn registered_root(conn: &Connection, repo_id: &str) -> Result<Option<PathBuf>> {
    Ok(conn
        .query_row("SELECT root FROM repos WHERE id = ?1", [repo_id], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .map(PathBuf::from))
}

#[cfg(test)]
mod tests;
