//! Starting on a half-built repo (spec §12.2): `bridge-screen init` gathers read-only
//! evidence for the agent, the agent drafts a plan and shows it to the owner once, and
//! `init_plan` creates the tree in one batch. Tasks the agent judged done are recorded as
//! claimed with their evidence, so they reach L1 by the existing check, never higher.

use std::fmt::Write as _;
use std::path::Path;

use rusqlite::{Connection, Transaction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::checkpoint::{ClaimEvidence, ClaimSource, claimed_done_draft};
use crate::error::{Error, Result};
use crate::git::{self, LogSummary};
use crate::import;
use crate::plan::{
    EditContext, NodeKind, NodeSource, NodeStatus, PlanOp, get_node, plan_edit_in_sourced,
};
use crate::store::append_in;

const README_CHARS: usize = 400;
const DOCS_SHOWN: usize = 30;
const SUBJECTS_SHOWN: usize = 15;
const TODO_FILES_SHOWN: usize = 10;
/// Longest confirmation from the owner that `init_plan` keeps.
pub const MAX_OWNER_SAID: usize = 200;

/// A plan file and whether it is imported already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanFile {
    pub path: String,
    pub imported: bool,
}

/// The repo's test command: configured by the owner, or a guess from its build files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestCommand {
    pub command: String,
    pub guessed: bool,
}

/// What `bridge-screen init` found (read-only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Evidence {
    /// The README's first heading and first paragraph.
    pub readme: Option<String>,
    pub docs: Vec<String>,
    pub docs_more: usize,
    pub plans: Vec<PlanFile>,
    pub specs: Vec<String>,
    pub git: LogSummary,
    pub todos: usize,
    pub todo_files: Vec<(String, usize)>,
    pub test_command: Option<TestCommand>,
    /// The repo already has a goal.
    pub has_plan: bool,
}

fn readme(dir: &Path) -> Option<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.to_ascii_lowercase().starts_with("readme"))
        })
        .collect();
    names.sort();
    let text = std::fs::read_to_string(names.first()?).ok()?;
    let mut heading = None;
    let mut para: Vec<&str> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if heading.is_none() && t.starts_with('#') {
            heading = Some(t.trim_start_matches('#').trim());
            continue;
        }
        if t.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if !t.starts_with('#') && !t.starts_with('!') && !t.starts_with('[') {
            para.push(t);
        }
    }
    let joined = [heading.unwrap_or(""), &para.join(" ")]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" — ");
    let out: String = joined.chars().take(README_CHARS).collect();
    (!out.is_empty()).then_some(out)
}

/// Paths under `docs/`, names only, sorted; at most `limit`, plus how many more.
fn docs(dir: &Path, limit: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut stack = vec![dir.join("docs")];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.filter_map(std::result::Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(dir) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
            if out.len() > 2000 {
                break;
            }
        }
    }
    out.sort();
    let more = out.len().saturating_sub(limit);
    out.truncate(limit);
    (out, more)
}

fn guess_test_command(dir: &Path) -> Option<String> {
    if dir.join("Cargo.toml").is_file() {
        return Some("cargo test --workspace".into());
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("package.json"))
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
        && v["scripts"]["test"].is_string()
    {
        return Some("npm test".into());
    }
    if dir.join("pyproject.toml").is_file() {
        return Some("pytest".into());
    }
    None
}

/// Gather the evidence for `dir`'s worktree. Reads files and runs git (CLI only); writes
/// nothing.
pub fn evidence(conn: &Connection, repo_id: &str, dir: &Path) -> Result<Evidence> {
    let (docs, docs_more) = docs(dir, DOCS_SHOWN);
    let imported: Vec<String> = import::sources(conn, repo_id)?
        .into_iter()
        .filter(|s| s.node_id.is_some() || s.baseline)
        .map(|s| s.path)
        .collect();
    let plans = import::plan_files(dir)?
        .into_iter()
        .map(|p| {
            let path = p
                .strip_prefix(dir)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            PlanFile {
                imported: imported.contains(&path),
                path,
            }
        })
        .collect();
    let specs: Vec<String> = import::spec_files(dir)?
        .into_iter()
        .map(|p| {
            p.strip_prefix(dir)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    let todo_files = git::todo_counts(dir)?;
    let test_command = match crate::verify::settings(conn, repo_id)?.test_command {
        Some(command) => Some(TestCommand {
            command,
            guessed: false,
        }),
        None => guess_test_command(dir).map(|command| TestCommand {
            command,
            guessed: true,
        }),
    };
    Ok(Evidence {
        readme: readme(dir),
        docs,
        docs_more,
        plans,
        specs,
        git: git::log_summary(dir, SUBJECTS_SHOWN)?,
        todos: todo_files.iter().map(|(_, n)| n).sum(),
        todo_files: todo_files.into_iter().take(TODO_FILES_SHOWN).collect(),
        test_command,
        has_plan: get_node(conn, repo_id, "G")?.is_some_and(|g| g.status != NodeStatus::Dropped),
    })
}

/// What `bridge-screen init` prints: the evidence, then the steps for the agent.
pub fn render(e: &Evidence, repo_name: &str, goal: Option<&str>) -> String {
    let mut out = String::new();
    if e.has_plan {
        let _ = writeln!(
            out,
            "{repo_name} already has a plan; nothing to set up. Change it with plan_edit (or `bridge-screen plan edit`)."
        );
        return out;
    }
    let _ = writeln!(
        out,
        "Setting up bridge-screen for {repo_name}: what the repo shows\n"
    );
    if let Some(r) = &e.readme {
        let _ = writeln!(out, "README: {r}");
    }
    let g = &e.git;
    if g.commits == 0 {
        let _ = writeln!(out, "History: no commits yet.");
    } else {
        let _ = writeln!(
            out,
            "History: {} commits, {} to {}. Newest:",
            g.commits,
            g.first.as_deref().unwrap_or("?"),
            g.last.as_deref().unwrap_or("?")
        );
        for s in &g.subjects {
            let _ = writeln!(out, "  - {s}");
        }
    }
    if !e.docs.is_empty() {
        let _ = writeln!(out, "Docs ({}{}):", e.docs.len(), plus(e.docs_more));
        for d in &e.docs {
            let _ = writeln!(out, "  - {d}");
        }
    }
    if !e.specs.is_empty() {
        let _ = writeln!(out, "Specs: {}", e.specs.join(", "));
    }
    let waiting: Vec<&PlanFile> = e.plans.iter().filter(|p| !p.imported).collect();
    if !e.plans.is_empty() {
        let _ = writeln!(
            out,
            "Plan files: {} ({} not imported yet)",
            e.plans.len(),
            waiting.len()
        );
    }
    let _ = writeln!(out, "TODO/FIXME: {}", e.todos);
    for (f, n) in &e.todo_files {
        let _ = writeln!(out, "  - {f}: {n}");
    }
    match &e.test_command {
        Some(t) if t.guessed => {
            let _ = writeln!(
                out,
                "Tests: probably `{}` (a guess; the owner sets it with `bridge-screen config set test-command`)",
                t.command
            );
        }
        Some(t) => {
            let _ = writeln!(out, "Tests: `{}`", t.command);
        }
        None => {
            let _ = writeln!(out, "Tests: no test command found");
        }
    }
    out.push('\n');
    if !waiting.is_empty() {
        let _ = writeln!(
            out,
            "First: run `bridge-screen import` — it turns the plan files above into the plan. Then run `bridge-screen init` again if anything is missing."
        );
        return out;
    }
    let _ = writeln!(out, "Next steps (for the agent):");
    let _ = writeln!(
        out,
        "1. Draft the plan from this evidence: the goal{} with its intent (purpose and end state), milestones with intents, and tasks with estimates in agent-minutes.",
        goal.map(|g| format!(" (\"{g}\")")).unwrap_or_default()
    );
    let _ = writeln!(
        out,
        "2. Mark the tasks that look done already, with the files (and key functions) that show it."
    );
    let _ = writeln!(
        out,
        "3. Show the draft to the owner once and ask them to confirm or correct it."
    );
    let _ = writeln!(
        out,
        "4. Call init_plan with the draft and the owner's words (owner_said). Done tasks are recorded as claimed and checked at HEAD; nothing is marked verified."
    );
    out
}

fn plus(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        format!(" shown, +{n} more")
    }
}

/// A task the agent judged done, and what shows it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DoneEvidence {
    /// What shows it is done (commits, tests that pass).
    pub evidence: String,
    /// Files, relative to the repo root, that show the work exists (checked at HEAD).
    #[serde(default)]
    pub files: Vec<String>,
    /// Key functions or types to find in `files`.
    #[serde(default)]
    pub symbols: Vec<String>,
}

/// One task of the draft.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftTask {
    /// One line, at most 200 characters.
    pub title: String,
    /// Estimate in agent-minutes.
    #[serde(default)]
    pub estimate_min: Option<u32>,
    #[serde(default)]
    pub expected_files: Vec<String>,
    /// Set when the task looks done already.
    #[serde(default)]
    pub done: Option<DoneEvidence>,
}

/// The goal or a milestone of the draft.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftNode {
    pub title: String,
    /// Purpose and end state, 2–3 sentences.
    pub intent: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftMilestone {
    pub title: String,
    pub intent: String,
    #[serde(default)]
    pub tasks: Vec<DraftTask>,
}

/// The plan the agent drafted and the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitDraft {
    pub goal: DraftNode,
    #[serde(default)]
    pub milestones: Vec<DraftMilestone>,
    /// Tasks directly under the goal.
    #[serde(default)]
    pub tasks: Vec<DraftTask>,
    /// The owner's words confirming the draft (at most 200 characters).
    pub owner_said: String,
}

/// What `apply` created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitApplied {
    pub milestones: usize,
    pub tasks: usize,
    /// The tasks recorded as claimed done.
    pub done: Vec<String>,
}

/// Who applies the draft.
#[derive(Debug, Clone, Copy)]
pub struct InitBy<'a> {
    pub repo_id: &'a str,
    pub session_id: &'a str,
    pub actor: &'a crate::store::Actor,
}

/// Create the drafted plan in one batch (sources `init`), then record the done tasks as
/// claimed with their evidence. Refused when the repo already has a goal or the owner's
/// words are missing.
pub fn apply(tx: &Transaction, by: &InitBy, draft: &InitDraft) -> Result<InitApplied> {
    if get_node(tx, by.repo_id, "G")?.is_some_and(|g| g.status != NodeStatus::Dropped) {
        return Err(Error::Invalid(
            "this repo already has a plan; use plan_edit".into(),
        ));
    }
    let said = draft.owner_said.trim();
    if said.is_empty() {
        return Err(Error::Invalid(
            "owner_said is required: show the draft to the owner and pass their words confirming it".into(),
        ));
    }
    if said.chars().count() > MAX_OWNER_SAID {
        return Err(Error::Invalid(format!(
            "owner_said must be at most {MAX_OWNER_SAID} characters"
        )));
    }
    let mut ops = vec![PlanOp::AddNode {
        parent: None,
        kind: NodeKind::Goal,
        title: draft.goal.title.clone(),
        intent: Some(draft.goal.intent.clone()),
        estimate_min: None,
        expected_files: Vec::new(),
    }];
    // For each op, the draft task it adds (to find the done ones by their new id).
    let mut task_of: Vec<Option<&DraftTask>> = vec![None];
    let add_task = |parent: String, t: &DraftTask| PlanOp::AddNode {
        parent: Some(parent),
        kind: NodeKind::Task,
        title: t.title.clone(),
        intent: None,
        estimate_min: t.estimate_min,
        expected_files: t.expected_files.clone(),
    };
    for (i, m) in draft.milestones.iter().enumerate() {
        let id = (i + 1).to_string();
        ops.push(PlanOp::AddNode {
            parent: Some("G".into()),
            kind: NodeKind::Milestone,
            title: m.title.clone(),
            intent: Some(m.intent.clone()),
            estimate_min: None,
            expected_files: Vec::new(),
        });
        task_of.push(None);
        for t in &m.tasks {
            ops.push(add_task(id.clone(), t));
            task_of.push(Some(t));
        }
    }
    for t in &draft.tasks {
        ops.push(add_task("G".into(), t));
        task_of.push(Some(t));
    }
    let ctx = EditContext {
        repo_id: by.repo_id.to_string(),
        session_id: by.session_id.to_string(),
        actor: by.actor.clone(),
        reason: Some(format!(
            "set up from the repo's history; the owner said: {said}"
        )),
        replan: false,
    };
    let sources = vec![Some(NodeSource::Init); ops.len()];
    let out = plan_edit_in_sourced(tx, &ctx, &ops, &sources)?;
    let claim_by = ClaimSource {
        repo_id: by.repo_id,
        session_id: by.session_id,
        actor: by.actor,
        inferred: true,
    };
    let mut done = Vec::new();
    for (id, task) in out.added_ids.iter().zip(&task_of) {
        let Some(d) = task.and_then(|t| t.done.as_ref()) else {
            continue;
        };
        let draft = claimed_done_draft(
            tx,
            &claim_by,
            id,
            &ClaimEvidence {
                evidence: &d.evidence,
                files: &d.files,
                symbols: &d.symbols,
            },
        )?;
        append_in(tx, draft)?;
        done.push(id.clone());
    }
    Ok(InitApplied {
        milestones: draft.milestones.len(),
        tasks: draft.tasks.len()
            + draft
                .milestones
                .iter()
                .map(|m| m.tasks.len())
                .sum::<usize>(),
        done,
    })
}
