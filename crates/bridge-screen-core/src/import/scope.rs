//! The "nobody says what is finished" question (spec §12.1). When plan files turn up in a
//! repo with no plan, no import history and no ticked step anywhere, SessionStart does not
//! import silently: it records one `plan_scope_asked` event, and nothing is imported until
//! the owner answers (`plan_scope_answered`): a fresh run (count progress from now) or
//! "some are done" (those files are noted as already finished, the rest imported).
//!
//! No view table: the question is open while the repo's newest scope event is an ask, so
//! it is derived when read and survives a rebuild.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use super::{ImportCtx, ImportOutcome, ScanBudget, sha256_hex, short_name};
use crate::error::{Error, Result};
use crate::store::{EventDraft, Store, append_in};

/// Event kinds of the question (no projector: they only decide what a scan does).
pub const SCOPE_KINDS: &[&str] = &["plan_scope_asked", "plan_scope_answered"];

/// The ask: the plan files that were found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanScopeAsked {
    pub paths: Vec<String>,
}

/// The owner's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanScopeAnswered {
    /// `fresh` or `some_done`.
    pub mode: String,
    /// Files noted as already finished (`some_done`).
    #[serde(default)]
    pub done: Vec<String>,
}

/// An open question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenScope {
    pub paths: Vec<String>,
    /// When it was asked (RFC 3339).
    pub since: String,
}

/// The question the repo is waiting on, if one is open.
pub fn open(conn: &Connection, repo_id: &str) -> Result<Option<OpenScope>> {
    let last: Option<(String, String, String)> = conn
        .prepare_cached(
            "SELECT kind, data, ts FROM events
             WHERE repo_id = ?1 AND kind IN ('plan_scope_asked', 'plan_scope_answered')
             ORDER BY pos DESC LIMIT 1",
        )?
        .query_row([repo_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    Ok(match last {
        Some((kind, data, ts)) if kind == "plan_scope_asked" => {
            let d: PlanScopeAsked = serde_json::from_str(&data)?;
            Some(OpenScope {
                paths: d.paths,
                since: ts,
            })
        }
        _ => None,
    })
}

/// Has the repo ever been asked?
fn ever_asked(conn: &Connection, repo_id: &str) -> Result<bool> {
    Ok(conn
        .prepare_cached(
            "SELECT 1 FROM events WHERE repo_id = ?1 AND kind IN ('plan_scope_asked',
             'plan_scope_answered') LIMIT 1",
        )?
        .exists([repo_id])?)
}

/// The question in plain words, for the briefing note, the CLI and the dashboard.
pub fn question_text(paths: &[String]) -> String {
    let names: Vec<&str> = paths.iter().map(|p| short_name(p)).collect();
    let shown = names.iter().take(5).copied().collect::<Vec<_>>().join(", ");
    let more = if names.len() > 5 {
        format!(" and {} more", names.len() - 5)
    } else {
        String::new()
    };
    format!(
        "Found {} plan file{} ({shown}{more}). None say what is done. Count progress from \
         now, or tell me which are already finished?",
        names.len(),
        if names.len() == 1 { "" } else { "s" }
    )
}

/// How to answer, for the CLI.
pub const HOW_TO_ANSWER: &str = "Answer with `bridge-screen import --fresh` (count progress from now) or \
     `bridge-screen import --done <file>…` (those files are already finished).";

/// Called by a scan before it imports anything. Returns the outcome to report when the
/// scan must not import: a question is open, or one was just raised.
pub(super) fn gate(
    store: &mut Store,
    repo_id: &str,
    root: &Path,
    files: &[PathBuf],
    ctx: &ImportCtx,
    budget: ScanBudget,
) -> Result<Option<ImportOutcome>> {
    if let Some(q) = open(store.conn(), repo_id)? {
        return Ok(Some(ImportOutcome::NeedsScope { paths: q.paths }));
    }
    if ever_asked(store.conn(), repo_id)?
        || !super::sources(store.conn(), repo_id)?.is_empty()
        || super::has_plan(store.conn(), repo_id)?
    {
        return Ok(None);
    }
    // Read the files (within the budget): a plan with a ticked step says what is done.
    let mut with_tasks = Vec::new();
    let mut read: u64 = 0;
    for path in files.iter().take(budget.files) {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if meta.len() > super::MAX_FILE_BYTES || read.saturating_add(meta.len()) > budget.bytes {
            continue;
        }
        read = read.saturating_add(meta.len());
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let parsed = super::parse(&text);
        if parsed.tasks.is_empty() {
            continue;
        }
        if parsed
            .tasks
            .iter()
            .any(|t| t.steps.iter().any(|s| s.checked))
        {
            return Ok(None);
        }
        with_tasks.push(super::relative(root, path));
    }
    if with_tasks.is_empty() {
        return Ok(None);
    }
    let draft = EventDraft::new(
        repo_id,
        &ctx.session_id,
        "plan_scope_asked",
        serde_json::to_value(PlanScopeAsked {
            paths: with_tasks.clone(),
        })?,
    )
    .actor(ctx.actor.clone());
    let run = |tx: &Transaction| -> Result<()> {
        append_in(tx, draft.clone())?;
        Ok(())
    };
    if ctx.dry_run {
        store.dry_run(run)?;
    } else {
        store.write(run)?;
    }
    Ok(Some(ImportOutcome::NeedsScope { paths: with_tasks }))
}

/// The files in `paths` that `names` point at: a whole path, a path ending in the name, or
/// the file name without `.md`.
fn resolve_names(paths: &[String], names: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for raw in names {
        let name = raw.trim().trim_start_matches("./");
        if name.is_empty() {
            continue;
        }
        let hit = paths.iter().find(|p| {
            p.as_str() == name
                || p.ends_with(&format!("/{name}"))
                || short_name(p) == name
                || short_name(p) == name.strip_suffix(".md").unwrap_or(name)
        });
        match hit {
            Some(p) => {
                if !out.contains(p) {
                    out.push(p.clone());
                }
            }
            None => {
                return Err(Error::Invalid(format!(
                    "{name} is not one of the plan files found ({}); nothing was recorded",
                    paths
                        .iter()
                        .map(|p| short_name(p))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    }
    Ok(out)
}

/// Split a free-text list of file names (commas, semicolons or new lines).
pub fn split_names(text: &str) -> Vec<String> {
    text.split([',', ';', '\n'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Answer the open question: `done` empty is a fresh run (everything imported, nothing
/// finished); otherwise the named files are noted as already finished and the rest
/// imported. Owner only; a second answer is refused as stale.
pub fn answer(
    store: &mut Store,
    repo_id: &str,
    root: &Path,
    ctx: &ImportCtx,
    done: &[String],
) -> Result<String> {
    let Some(q) = open(store.conn(), repo_id)? else {
        return Err(Error::Stale(
            "already resolved: there is no open question about which plans are finished".into(),
        ));
    };
    let done = resolve_names(&q.paths, done)?;
    store.write(|tx| {
        append_in(
            tx,
            EventDraft::new(
                repo_id,
                &ctx.session_id,
                "plan_scope_answered",
                serde_json::to_value(PlanScopeAnswered {
                    mode: if done.is_empty() {
                        "fresh"
                    } else {
                        "some_done"
                    }
                    .into(),
                    done: done.clone(),
                })?,
            )
            .actor(ctx.actor.clone()),
        )?;
        for rel in &done {
            let path = root.join(rel);
            let meta = std::fs::metadata(&path)?;
            let hash = std::fs::read(&path).map(|b| sha256_hex(&b))?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            let seen = super::PlanFileSeen {
                path: rel.clone(),
                hash,
                size: meta.len(),
                mtime,
                baseline: true,
                needs_link: false,
            };
            append_in(tx, super::seen_draft(repo_id, ctx, &seen)?)?;
        }
        Ok(())
    })?;
    let imported = super::scan(store, repo_id, root, ctx, ScanBudget::UNLIMITED)?
        .iter()
        .filter(|o| matches!(o, ImportOutcome::Imported { .. }))
        .count();
    Ok(if done.is_empty() {
        format!(
            "Fresh run: imported {imported} plan file{}; progress counts from now.",
            if imported == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Noted {} plan file{} as already finished; imported {imported} other{}.",
            done.len(),
            if done.len() == 1 { "" } else { "s" },
            if imported == 1 { "" } else { "s" }
        )
    })
}
