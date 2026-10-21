//! The drift heuristic (spec §7.2): a score from 0 (clearly on the task) to 1 (clearly
//! elsewhere) for one unit of work, from the files it touched and the agent's last message
//! (the files alone when there is no message).
//!
//! [`score`] is a pure function. [`context`] reads what it compares against from the views.
//! The agent's last message is used in memory only; it is never stored here.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};

use crate::error::Result;
use crate::plan::{NodeKind, load_tree};

/// What one unit is compared against.
#[derive(Debug, Clone, Copy)]
pub struct ScoreInput<'a> {
    /// Files the unit changed, relative to the repo root.
    pub unit_files: &'a [String],
    /// The agent's last message for the unit.
    pub summary: &'a str,
    /// The task's expected files, plus files of earlier units on the task and on its
    /// sibling tasks.
    pub task_files: &'a HashSet<String>,
    /// Titles and intents of the task, its milestone and the goal.
    pub intent_text: &'a str,
    /// Top-level directories the plan or any earlier unit ever touched.
    pub mapped_dirs: &'a HashSet<String>,
}

/// The score and the parts it was made from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreBreakdown {
    /// 0..=1; higher means further from the task.
    pub score: f64,
    /// Share of the unit's files that are task files or sit next to one; `None` when the
    /// unit touched no files or the task has none to compare with.
    pub file_affinity: Option<f64>,
    /// Keyword overlap between the message and the intent text (0..=1).
    pub text_affinity: f64,
    /// Share of the unit's files in parts of the repo the plan never mentions (0..=1).
    pub area_penalty: f64,
}

const W_FILE: f64 = 0.5;
const W_TEXT: f64 = 0.3;
const W_AREA: f64 = 0.2;
/// Without file affinity, text and area share the weight 0.6 / 0.4.
const W_TEXT_ONLY: f64 = 0.6;
const W_AREA_ONLY: f64 = 0.4;
/// Text affinity is measured against at most this many of the message's keywords.
const TEXT_KEYWORDS: usize = 12;

/// Words too common to tell tasks apart.
const STOP_WORDS: &[&str] = &[
    "about", "added", "after", "all", "also", "and", "any", "are", "been", "but", "can", "could",
    "did", "does", "done", "for", "from", "get", "has", "have", "here", "how", "into", "its",
    "just", "let", "lets", "like", "make", "made", "more", "need", "new", "not", "now", "our",
    "out", "please", "should", "some", "that", "the", "them", "then", "there", "these", "this",
    "use", "used", "want", "was", "were", "what", "when", "which", "will", "with", "would", "you",
    "your", "now", "also", "one", "two", "very",
];

/// Lowercase alphanumeric tokens of at least 3 characters, stop-words removed, with a
/// simple plural trim (`tests` → `test`, but not `class`).
pub fn keywords(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_lowercase)
        .filter(|w| !STOP_WORDS.contains(&w.as_str()))
        .map(|w| {
            if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") {
                w[..w.len() - 1].to_string()
            } else {
                w
            }
        })
        .collect()
}

/// The first path component: `src` for `src/a/b.rs`, `.` for a file at the root.
pub fn top_dir(file: &str) -> String {
    let f = file.trim_start_matches("./");
    match f.trim_start_matches('/').split_once('/') {
        Some((top, _)) if f.starts_with('/') => format!("/{top}"),
        Some((top, _)) => top.to_string(),
        None => ".".to_string(),
    }
}

/// The directory a file is in (`.` at the root).
fn parent_dir(file: &str) -> &str {
    file.rsplit_once('/').map_or(".", |(dir, _)| dir)
}

#[allow(clippy::cast_precision_loss)] // file and word counts are tiny
fn share(part: usize, whole: usize) -> f64 {
    part as f64 / whole as f64
}

/// Score one unit (see the module docs and spec §7.2).
pub fn score(input: &ScoreInput) -> ScoreBreakdown {
    let files = input.unit_files;
    if files.is_empty() && input.summary.trim().is_empty() {
        return ScoreBreakdown {
            score: 0.0,
            file_affinity: None,
            text_affinity: 0.0,
            area_penalty: 0.0,
        };
    }
    let file_affinity = (!files.is_empty() && !input.task_files.is_empty()).then(|| {
        let task_dirs: HashSet<&str> = input.task_files.iter().map(|f| parent_dir(f)).collect();
        let near = files
            .iter()
            .filter(|f| input.task_files.contains(*f) || task_dirs.contains(parent_dir(f)))
            .count();
        share(near, files.len())
    });
    let said = keywords(input.summary);
    let meant = keywords(input.intent_text);
    let hits = said.intersection(&meant).count();
    let text_affinity = share(hits, said.len().clamp(1, TEXT_KEYWORDS)).min(1.0);
    let area_penalty = if files.is_empty() {
        0.0
    } else {
        let unmapped = files
            .iter()
            .filter(|f| !input.mapped_dirs.contains(&top_dir(f)))
            .count();
        share(unmapped, files.len())
    };
    // No message at all (a unit cut short by a new prompt has none): the files alone
    // decide; missing text is not evidence of drift. Without task files to compare with,
    // the area alone can't reach the drift cut-off: at most a question.
    let no_text = input.summary.trim().is_empty();
    let score = match file_affinity {
        Some(fa) if no_text => (W_FILE * (1.0 - fa) + W_AREA * area_penalty) / (W_FILE + W_AREA),
        Some(fa) => W_FILE * (1.0 - fa) + W_TEXT * (1.0 - text_affinity) + W_AREA * area_penalty,
        None if no_text => W_AREA_ONLY * area_penalty,
        None => W_TEXT_ONLY * (1.0 - text_affinity) + W_AREA_ONLY * area_penalty,
    };
    ScoreBreakdown {
        score: score.clamp(0.0, 1.0),
        file_affinity,
        text_affinity,
        area_penalty,
    }
}

/// What a repo's units are scored against, for one task.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScoreContext {
    pub task_files: HashSet<String>,
    pub intent_text: String,
    pub mapped_dirs: HashSet<String>,
}

/// The comparison set for a unit on `task` (or on no task), leaving out the unit being
/// scored. `None` when the repo has no plan: there is nothing to drift from.
pub fn context(
    conn: &Connection,
    repo_id: &str,
    task: Option<&str>,
    exclude: (&str, i64),
) -> Result<Option<ScoreContext>> {
    let tree = load_tree(conn, repo_id, false)?;
    let Some(goal) = tree.iter().find(|n| n.kind == NodeKind::Goal) else {
        return Ok(None);
    };
    let by_id: HashMap<&str, &crate::plan::PlanNode> =
        tree.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut ctx = ScoreContext::default();
    for node in &tree {
        ctx.mapped_dirs
            .extend(node.expected_files.iter().map(|f| top_dir(f)));
    }
    // The task, then its ancestors up to the goal (commander's intent, two levels up).
    let mut related: HashSet<&str> = HashSet::new();
    let mut intent = Vec::new();
    if let Some(node) = task.and_then(|t| by_id.get(t)) {
        ctx.task_files.extend(node.expected_files.iter().cloned());
        related.insert(node.id.as_str());
        for sib in tree.iter().filter(|n| {
            n.parent_id == node.parent_id && matches!(n.kind, NodeKind::Task | NodeKind::Subtask)
        }) {
            related.insert(sib.id.as_str());
        }
        let mut at = Some(*node);
        while let Some(n) = at {
            intent.push(n.title.as_str());
            intent.extend(n.intent.as_deref());
            at = n.parent_id.as_deref().and_then(|p| by_id.get(p).copied());
        }
    } else {
        intent.push(goal.title.as_str());
        intent.extend(goal.intent.as_deref());
    }
    ctx.intent_text = intent.join(" ");
    let mut stmt = conn.prepare_cached(
        "SELECT u.task_id, u.files FROM units u JOIN sessions s ON s.session_id = u.session_id
         WHERE s.repo_id = ?1 AND u.files != '[]'
           AND NOT (u.session_id = ?2 AND u.unit_no = ?3)",
    )?;
    let mut rows = stmt.query(params![repo_id, exclude.0, exclude.1])?;
    while let Some(row) = rows.next()? {
        let task_id: Option<String> = row.get(0)?;
        let files: Vec<String> = serde_json::from_str(&row.get::<_, String>(1)?)?;
        ctx.mapped_dirs.extend(files.iter().map(|f| top_dir(f)));
        if task_id.as_deref().is_some_and(|t| related.contains(t)) {
            ctx.task_files.extend(files);
        }
    }
    Ok(Some(ctx))
}

#[cfg(test)]
mod tests {
    use super::{ScoreInput, keywords, score, top_dir};
    use std::collections::HashSet;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(ToString::to_string).collect()
    }

    fn files(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    const INTENT: &str =
        "Login form validation. Accounts: people can sign in. Ship: owner sees progress.";

    #[test]
    fn keywords_are_trimmed_and_filtered() {
        assert_eq!(
            keywords("Added the Tests for login-forms, and a CI fix; class"),
            set(&["test", "login", "form", "fix", "class"])
        );
        assert_eq!(top_dir("src/a/b.rs"), "src");
        assert_eq!(top_dir("Cargo.toml"), ".");
        assert_eq!(top_dir("/etc/hosts"), "/etc");
    }

    #[test]
    fn docker_unit_on_a_login_task_is_drift() {
        let unit = files(&["docker/Dockerfile", ".github/workflows/ci.yml"]);
        let b = score(&ScoreInput {
            unit_files: &unit,
            summary: "Rewrote the Dockerfile and the CI workflow caching.",
            task_files: &set(&["src/login.rs", "tests/login.rs"]),
            intent_text: INTENT,
            mapped_dirs: &set(&["src", "tests"]),
        });
        assert_eq!(b.file_affinity, Some(0.0));
        assert!(b.text_affinity.abs() < f64::EPSILON);
        assert!((b.area_penalty - 1.0).abs() < f64::EPSILON);
        assert!(b.score > 0.7, "{b:?}");
    }

    #[test]
    fn unit_on_the_tasks_own_files_is_on_task() {
        let unit = files(&["src/login.rs", "src/login_view.rs"]);
        let b = score(&ScoreInput {
            unit_files: &unit,
            summary: "Added validation to the login form and a unit test.",
            task_files: &set(&["src/login.rs"]),
            intent_text: INTENT,
            mapped_dirs: &set(&["src"]),
        });
        // The second file sits next to a task file.
        assert_eq!(b.file_affinity, Some(1.0));
        // validation, login, form out of {validation, login, form, unit, test}.
        assert!((b.text_affinity - 0.6).abs() < 1e-9, "{b:?}");
        assert!(b.area_penalty.abs() < f64::EPSILON);
        assert!(b.score < 0.3, "{b:?}");
    }

    #[test]
    fn no_files_uses_text_and_area_only() {
        let none: Vec<String> = Vec::new();
        let b = score(&ScoreInput {
            unit_files: &none,
            summary: "Explained how the release pipeline builds artifacts.",
            task_files: &set(&["src/login.rs"]),
            intent_text: INTENT,
            mapped_dirs: &set(&["src"]),
        });
        assert_eq!(b.file_affinity, None);
        assert!((b.score - 0.6).abs() < 1e-9, "{b:?}");
        // No task files either: still text and area only.
        let unit = files(&["infra/main.tf"]);
        let b = score(&ScoreInput {
            unit_files: &unit,
            summary: "Login form validation done.",
            task_files: &HashSet::new(),
            intent_text: INTENT,
            mapped_dirs: &set(&["src"]),
        });
        assert_eq!(b.file_affinity, None);
        assert!((b.score - 0.4).abs() < 1e-9, "{b:?}");
        // No message (a unit cut short by a new prompt): the files alone.
        let unit = files(&["src/login.rs", "tools/gen.py"]);
        let b = score(&ScoreInput {
            unit_files: &unit,
            summary: "",
            task_files: &set(&["src/login.rs"]),
            intent_text: INTENT,
            mapped_dirs: &set(&["src"]),
        });
        assert!((b.score - 0.5).abs() < 1e-9, "{b:?}");
        let unit = files(&["infra/main.tf"]);
        let b = score(&ScoreInput {
            unit_files: &unit,
            summary: "",
            task_files: &HashSet::new(),
            intent_text: INTENT,
            mapped_dirs: &set(&["src"]),
        });
        assert!((b.score - 0.4).abs() < 1e-9, "{b:?}");
        // Nothing at all: 0.
        let b = score(&ScoreInput {
            unit_files: &none,
            summary: "  ",
            task_files: &HashSet::new(),
            intent_text: INTENT,
            mapped_dirs: &HashSet::new(),
        });
        assert!(b.score.abs() < f64::EPSILON);
    }
}
