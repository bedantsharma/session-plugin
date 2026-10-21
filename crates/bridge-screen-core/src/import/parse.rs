//! Parsing a superpowers plan file (spec §12.1). Pure: text in, [`ParsedPlan`] out.
//!
//! Accepts the superpowers template (`### Task N: name`, a `**Files:**` list,
//! `- [ ] **Step k: text**`) and this repo's own style (plain `- [ ] text` steps, no Files
//! list). Anything inside fenced code blocks is ignored.

use std::collections::HashSet;

use serde::Serialize;

/// Longest title or step text kept (the plan's title limit).
pub const MAX_TEXT_CHARS: usize = 200;

/// Most steps kept in one task, and most tasks plus steps kept from one file. A real plan
/// has a few dozen; a file of 100,000 checkboxes would otherwise stall SessionStart. What
/// is left out is named in the warnings.
pub const MAX_STEPS_PER_TASK: usize = 100;
pub const MAX_NODES: usize = 1000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ParsedPlan {
    pub title: Option<String>,
    pub goal: Option<String>,
    pub tasks: Vec<ParsedTask>,
    /// What was skipped and why, in plain words.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedTask {
    pub number: u32,
    pub title: String,
    pub files: Vec<String>,
    pub steps: Vec<ParsedStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedStep {
    pub number: u32,
    pub text: String,
    pub checked: bool,
}

/// One line of text, at most [`MAX_TEXT_CHARS`], control characters dropped.
pub fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let squeezed = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    squeezed.chars().take(MAX_TEXT_CHARS).collect()
}

fn strip_title(line: &str) -> String {
    let mut t = line.trim();
    for suffix in [
        " — Implementation Plan",
        " - Implementation Plan",
        " Implementation Plan",
    ] {
        if let Some(rest) = t.strip_suffix(suffix) {
            t = rest.trim_end();
            break;
        }
    }
    one_line(t)
}

/// `### Task N: name` (also `Task N.` / `Task N —` / `Task N -`) → (N, name).
fn task_heading(line: &str) -> Option<(Option<u32>, String)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(2..=4).contains(&hashes) {
        return None;
    }
    let rest = line[hashes..].trim_start();
    let rest = rest.strip_prefix("Task ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let after = rest[digits.len()..].trim_start();
    let name = ["—", ":", ".", "-", "–"]
        .iter()
        .find_map(|sep| after.strip_prefix(sep))
        .unwrap_or(if after.is_empty() { "" } else { after });
    let number = digits.parse::<u32>().ok().filter(|n| *n > 0);
    Some((number, one_line(name.trim())))
}

/// `- [ ] text` / `- [x] text` at the start of a line (at most one space of indent).
fn checkbox(line: &str) -> Option<(bool, &str)> {
    let indent = line.len() - line.trim_start().len();
    if indent > 1 {
        return None;
    }
    let t = line.trim_start();
    let t = t.strip_prefix("- ").or_else(|| t.strip_prefix("* "))?;
    if let Some(rest) = t.strip_prefix("[ ]") {
        return Some((false, rest.trim()));
    }
    if let Some(rest) = t.strip_prefix("[x]").or_else(|| t.strip_prefix("[X]")) {
        return Some((true, rest.trim()));
    }
    None
}

/// `**Step k: text**` → (k, text).
fn step_label(text: &str) -> Option<(u32, String)> {
    let inner = text.strip_prefix("**")?;
    let inner = inner.strip_prefix("Step ")?;
    let digits: String = inner.chars().take_while(char::is_ascii_digit).collect();
    let k = digits.parse::<u32>().ok().filter(|k| *k > 0)?;
    let rest = inner[digits.len()..].trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
    let body = match rest.find("**") {
        Some(end) => &rest[..end],
        None => rest,
    };
    Some((k, one_line(body)))
}

/// `- Create: \`path:12-40\`` → `path`.
fn file_item(line: &str) -> Option<String> {
    let t = line.trim_start().strip_prefix("- ")?;
    let start = t.find('`')?;
    let rest = &t[start + 1..];
    let end = rest.find('`')?;
    let mut path = rest[..end].trim();
    if let Some((p, lines)) = path.rsplit_once(':')
        && !lines.is_empty()
        && lines
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == ',')
    {
        path = p;
    }
    (!path.is_empty()).then(|| path.to_string())
}

/// Parser state while walking the lines.
#[derive(Default)]
struct Parser {
    plan: ParsedPlan,
    in_fence: bool,
    goal_lines: Option<Vec<String>>,
    /// Inside a task that was kept (not before the first task or in a skipped one).
    in_task: bool,
    in_files: bool,
    /// Task numbers seen so far (kept tasks only).
    seen: HashSet<u32>,
    /// Tasks plus steps kept so far.
    nodes: usize,
    /// Which caps already warned (`steps`, `nodes`).
    warned: HashSet<&'static str>,
}

impl Parser {
    fn finish_goal(&mut self) {
        if let Some(lines) = self.goal_lines.take() {
            self.plan.goal = Some(one_line(&lines.join(" "))).filter(|g| !g.is_empty());
        }
    }

    fn line(&mut self, raw: &str) {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            self.in_fence = !self.in_fence;
            return;
        }
        if self.in_fence {
            return;
        }
        if let Some(lines) = self.goal_lines.as_mut() {
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("**") {
                self.finish_goal();
            } else {
                lines.push(trimmed.to_string());
                return;
            }
        }
        if self.plan.title.is_none()
            && let Some(t) = line.strip_prefix("# ")
        {
            self.plan.title = Some(strip_title(t)).filter(|t| !t.is_empty());
            return;
        }
        if self.plan.goal.is_none()
            && let Some(at) = line.find("**Goal:**")
        {
            self.goal_lines = Some(vec![line[at + "**Goal:**".len()..].trim().to_string()]);
            return;
        }
        if trimmed.starts_with('#') {
            self.heading(trimmed);
        } else if self.in_task {
            self.task_line(line, trimmed);
        }
    }

    fn heading(&mut self, trimmed: &str) {
        self.in_files = false;
        match task_heading(trimmed) {
            Some((Some(number), title)) if !title.is_empty() => {
                if self.nodes >= MAX_NODES {
                    if self.warned.insert("nodes") {
                        self.plan.warnings.push(format!(
                            "too many tasks and steps; kept the first {MAX_NODES}"
                        ));
                    }
                    self.in_task = false;
                } else if self.seen.insert(number) {
                    self.nodes += 1;
                    self.plan.tasks.push(ParsedTask {
                        number,
                        title,
                        files: Vec::new(),
                        steps: Vec::new(),
                    });
                    self.in_task = true;
                } else {
                    self.plan
                        .warnings
                        .push(format!("task {number} appears twice; kept the first"));
                    self.in_task = false;
                }
            }
            Some((number, _)) => {
                self.plan.warnings.push(match number {
                    Some(n) => format!("task {n} has no name; skipped"),
                    None => "a task heading has no usable number; skipped".to_string(),
                });
                self.in_task = false;
            }
            None => {
                // Another heading ends the task only at the task headings' level or higher.
                if trimmed.chars().take_while(|c| *c == '#').count() <= 3 {
                    self.in_task = false;
                }
            }
        }
    }

    fn task_line(&mut self, line: &str, trimmed: &str) {
        let Some(task) = self.plan.tasks.last_mut() else {
            return;
        };
        if trimmed.starts_with("**Files:**") {
            self.in_files = true;
            return;
        }
        if self.in_files {
            if let Some(path) = file_item(line) {
                if !task.files.contains(&path) {
                    task.files.push(path);
                }
                return;
            }
            if trimmed.starts_with("- ") && checkbox(line).is_none() {
                return;
            }
            self.in_files = false;
        }
        let Some((checked, body)) = checkbox(line) else {
            return;
        };
        if task.steps.len() >= MAX_STEPS_PER_TASK || self.nodes >= MAX_NODES {
            if self.warned.insert("steps") {
                self.plan.warnings.push(format!(
                    "too many steps in task {}; kept the first {MAX_STEPS_PER_TASK}",
                    task.number
                ));
            }
            return;
        }
        let position = u32::try_from(task.steps.len() + 1).unwrap_or(u32::MAX);
        let (number, text) = match step_label(body) {
            Some((k, text)) => (k, text),
            None => (position, one_line(body)),
        };
        let number = if task.steps.iter().any(|s| s.number == number) {
            task.steps
                .iter()
                .map(|s| s.number)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
        } else {
            number
        };
        if !text.is_empty() {
            self.nodes += 1;
            task.steps.push(ParsedStep {
                number,
                text,
                checked,
            });
        }
    }
}

/// Parse a plan file's text. Never panics; odd input gives fewer tasks and warnings.
pub fn parse(text: &str) -> ParsedPlan {
    let mut p = Parser::default();
    for raw in text.lines() {
        p.line(raw);
    }
    p.finish_goal();
    if p.plan.tasks.is_empty() {
        p.plan
            .warnings
            .push("no \"### Task N:\" headings".to_string());
    }
    p.plan
}
