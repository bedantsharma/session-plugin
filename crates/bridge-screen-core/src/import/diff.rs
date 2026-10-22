//! What changed in a plan file since its last import, as plan ops (pure).
//!
//! Node ids are predicted the way `plan_edit` assigns them (the next free number under the
//! parent, in op order); `import_file` checks the prediction against the ids it got.

use std::collections::HashSet;

use crate::plan::{NodeKind, NodeSource, NodeStatus, PlanOp};

use super::parse::{ParsedPlan, ParsedStep, ParsedTask};

/// One task or step a file's last import produced (a `plan_source_items` row), with its
/// node's current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceItem {
    pub task_no: u32,
    /// `None` for the task itself.
    pub step_no: Option<u32>,
    pub node_id: String,
    /// The title the file gave it last time (not the node's, which the owner may edit).
    pub title: String,
    pub status: NodeStatus,
    /// The node's expected files now.
    pub files: Vec<String>,
    /// The number the node's next child gets.
    pub next_child: u32,
}

/// Where a file's tasks go; decided by `import_file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// The repo has no goal: the file becomes the goal, its tasks go directly under it.
    NewGoal { title: String, intent: String },
    /// A new milestone under the goal; `id` is the one it will get.
    NewMilestone {
        goal_id: String,
        id: String,
        title: String,
        intent: String,
    },
    /// A goal or milestone the file is already linked to.
    Existing {
        node_id: String,
        next_child: u32,
        /// Its intent now, and whether the file's goal text may replace it.
        intent: Option<String>,
        update_intent: bool,
    },
}

impl ImportTarget {
    fn container(&self) -> &str {
        match self {
            Self::NewGoal { .. } => "G",
            Self::NewMilestone { id, .. } => id,
            Self::Existing { node_id, .. } => node_id,
        }
    }
}

/// A task or step and the node it maps to after the import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemLink {
    pub task_no: u32,
    pub step_no: Option<u32>,
    pub node_id: String,
    pub title: String,
}

/// The result of [`diff`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportDiff {
    pub ops: Vec<PlanOp>,
    /// `ops[i]`'s source, for added nodes.
    pub sources: Vec<Option<NodeSource>>,
    /// The ids `ops`' added nodes will get, in order.
    pub added: Vec<String>,
    /// Nodes to record as claimed done: a step ticked in the file.
    pub claims: Vec<String>,
    /// Nodes no longer in the file but with work on them: kept, with this reason.
    pub keeps: Vec<(String, String)>,
    /// Every task and step of the file after the import.
    pub items: Vec<ItemLink>,
    pub tasks_added: usize,
    pub tasks_removed: usize,
    pub tasks_edited: usize,
    pub changed_ratio: f64,
    pub replan: bool,
}

/// Share of tasks changed above which an import is recorded as a re-plan.
pub const REPLAN_RATIO: f64 = 0.30;

fn same_title(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn has_work(status: NodeStatus) -> bool {
    !matches!(status, NodeStatus::Todo | NodeStatus::Dropped)
}

fn is_done(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Claimed | NodeStatus::L1 | NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4
    )
}

type Matcher<'m, W, H> = &'m dyn Fn(&W, &H) -> bool;

/// Pair each of `wanted` with one of `have` (indices): exact title and number first, then
/// the same title under a moved number, then the same number with a new title.
fn pair<'a, W, H>(
    wanted: &[W],
    have: &'a [H],
    num_w: impl Fn(&W) -> u32,
    title_w: impl Fn(&W) -> &str,
    num_h: impl Fn(&H) -> u32,
    title_h: impl Fn(&H) -> &str,
) -> Vec<Option<&'a H>> {
    let mut out: Vec<Option<usize>> = vec![None; wanted.len()];
    let mut used: HashSet<usize> = HashSet::new();
    let passes: [Matcher<W, H>; 3] = [
        &|w, h| num_w(w) == num_h(h) && same_title(title_w(w), title_h(h)),
        &|w, h| same_title(title_w(w), title_h(h)),
        &|w, h| num_w(w) == num_h(h),
    ];
    for matches in passes {
        for (i, w) in wanted.iter().enumerate() {
            if out[i].is_some() {
                continue;
            }
            if let Some(j) = (0..have.len()).find(|j| !used.contains(j) && matches(w, &have[*j])) {
                used.insert(j);
                out[i] = Some(j);
            }
        }
    }
    out.into_iter().map(|j| j.map(|j| &have[j])).collect()
}

/// A task (`step` none) or subtask the file adds.
struct NewNode<'a> {
    parent: &'a str,
    id: String,
    title: &'a str,
    files: Vec<String>,
    task: u32,
    step: Option<u32>,
}

struct Builder<'a> {
    file: &'a str,
    d: ImportDiff,
}

impl Builder<'_> {
    fn push(&mut self, op: PlanOp, source: Option<NodeSource>) {
        self.d.ops.push(op);
        self.d.sources.push(source);
    }

    fn add_node(&mut self, n: NewNode) {
        self.push(
            PlanOp::AddNode {
                parent: Some(n.parent.to_string()),
                kind: if n.step.is_some() {
                    NodeKind::Subtask
                } else {
                    NodeKind::Task
                },
                title: n.title.to_string(),
                intent: None,
                estimate_min: None,
                expected_files: n.files,
            },
            Some(NodeSource::File {
                file: self.file.to_string(),
                task: n.task,
                step: n.step,
            }),
        );
        self.link(n.task, n.step, &n.id, n.title);
        self.d.added.push(n.id);
    }

    fn link(&mut self, task_no: u32, step_no: Option<u32>, node_id: &str, title: &str) {
        self.d.items.push(ItemLink {
            task_no,
            step_no,
            node_id: node_id.to_string(),
            title: title.to_string(),
        });
    }

    /// Drop `item` if nothing was done on it (nor on `children`), else keep it.
    fn retire(&mut self, item: &SourceItem, children: &[&SourceItem]) {
        if has_work(item.status) || children.iter().any(|c| has_work(c.status)) {
            self.d
                .keeps
                .push((item.node_id.clone(), format!("no longer in {}", self.file)));
        } else {
            self.push(
                PlanOp::RemoveNode {
                    id: item.node_id.clone(),
                },
                None,
            );
        }
    }

    /// Create the goal or milestone the file's tasks go under, or update the existing
    /// one's intent; returns the number its next child gets.
    fn open(&mut self, target: &ImportTarget, parsed: &ParsedPlan) -> u32 {
        match target {
            ImportTarget::NewGoal { title, intent } => {
                self.push(
                    PlanOp::AddNode {
                        parent: None,
                        kind: NodeKind::Goal,
                        title: title.clone(),
                        intent: Some(intent.clone()),
                        estimate_min: None,
                        expected_files: Vec::new(),
                    },
                    None,
                );
                self.d.added.push("G".into());
                1
            }
            ImportTarget::NewMilestone {
                goal_id,
                id,
                title,
                intent,
            } => {
                self.push(
                    PlanOp::AddNode {
                        parent: Some(goal_id.clone()),
                        kind: NodeKind::Milestone,
                        title: title.clone(),
                        intent: Some(intent.clone()),
                        estimate_min: None,
                        expected_files: Vec::new(),
                    },
                    None,
                );
                self.d.added.push(id.clone());
                1
            }
            ImportTarget::Existing {
                node_id,
                next_child,
                intent,
                update_intent,
            } => {
                if *update_intent
                    && let Some(goal) = &parsed.goal
                    && intent.as_deref() != Some(goal.as_str())
                {
                    self.push(
                        PlanOp::SetIntent {
                            id: node_id.clone(),
                            intent: goal.clone(),
                        },
                        None,
                    );
                }
                *next_child
            }
        }
    }

    fn new_task(&mut self, parent: &str, id: &str, t: &ParsedTask) {
        self.add_node(NewNode {
            parent,
            id: id.to_string(),
            title: &t.title,
            files: t.files.clone(),
            task: t.number,
            step: None,
        });
        for (k, s) in t.steps.iter().enumerate() {
            let sid = format!("{id}.{}", k + 1);
            self.add_node(NewNode {
                parent: id,
                id: sid.clone(),
                title: &s.text,
                files: Vec::new(),
                task: t.number,
                step: Some(s.number),
            });
            if s.checked {
                self.d.claims.push(sid);
            }
        }
    }

    fn existing_task(&mut self, t: &ParsedTask, item: &SourceItem, prev_steps: &[&SourceItem]) {
        let title = (!same_title(&t.title, &item.title)).then(|| t.title.clone());
        let files = (!t.files.is_empty() && t.files != item.files).then(|| t.files.clone());
        if title.is_some() {
            self.d.tasks_edited += 1;
        }
        if title.is_some() || files.is_some() {
            self.push(
                PlanOp::EditNode {
                    id: item.node_id.clone(),
                    title,
                    expected_files: files,
                },
                None,
            );
        }
        self.link(t.number, None, &item.node_id, &t.title);
        let have: Vec<SourceItem> = prev_steps.iter().map(|s| (*s).clone()).collect();
        let pairs = pair(
            &t.steps,
            &have,
            |s: &ParsedStep| s.number,
            |s| &s.text,
            |h: &SourceItem| h.step_no.unwrap_or(0),
            |h| &h.title,
        );
        let mut next = item.next_child;
        for (s, matched) in t.steps.iter().zip(&pairs) {
            if let Some(h) = matched {
                if !same_title(&s.text, &h.title) {
                    self.push(
                        PlanOp::EditNode {
                            id: h.node_id.clone(),
                            title: Some(s.text.clone()),
                            expected_files: None,
                        },
                        None,
                    );
                }
                self.link(t.number, Some(s.number), &h.node_id, &s.text);
                if s.checked && !is_done(h.status) {
                    self.d.claims.push(h.node_id.clone());
                }
            } else {
                let sid = format!("{}.{next}", item.node_id);
                next += 1;
                self.add_node(NewNode {
                    parent: &item.node_id,
                    id: sid.clone(),
                    title: &s.text,
                    files: Vec::new(),
                    task: t.number,
                    step: Some(s.number),
                });
                if s.checked {
                    self.d.claims.push(sid);
                }
            }
        }
        for h in &have {
            if !pairs.iter().flatten().any(|m| m.node_id == h.node_id) {
                self.retire(h, &[]);
            }
        }
    }
}

/// What to change so the plan matches `parsed`, given what the file's last import produced
/// (`prev`, dropped nodes left out) and where its tasks go.
pub fn diff(
    prev: &[SourceItem],
    parsed: &ParsedPlan,
    target: &ImportTarget,
    file: &str,
) -> ImportDiff {
    let mut b = Builder {
        file,
        d: ImportDiff::default(),
    };
    let prev: Vec<&SourceItem> = prev
        .iter()
        .filter(|i| i.status != NodeStatus::Dropped)
        .collect();
    let mut next = b.open(target, parsed);
    let container = target.container().to_string();
    let prev_tasks: Vec<SourceItem> = prev
        .iter()
        .filter(|i| i.step_no.is_none())
        .map(|i| (*i).clone())
        .collect();
    let pairs = pair(
        &parsed.tasks,
        &prev_tasks,
        |t: &ParsedTask| t.number,
        |t| &t.title,
        |h: &SourceItem| h.task_no,
        |h| &h.title,
    );
    for (t, matched) in parsed.tasks.iter().zip(&pairs) {
        if let Some(item) = matched {
            let steps: Vec<&SourceItem> = prev
                .iter()
                .filter(|i| i.task_no == item.task_no && i.step_no.is_some())
                .copied()
                .collect();
            b.existing_task(t, item, &steps);
        } else {
            let id = if container == "G" {
                next.to_string()
            } else {
                format!("{container}.{next}")
            };
            next += 1;
            b.d.tasks_added += 1;
            b.new_task(&container, &id, t);
        }
    }
    for item in &prev_tasks {
        if pairs.iter().flatten().any(|m| m.node_id == item.node_id) {
            continue;
        }
        b.d.tasks_removed += 1;
        let children: Vec<&SourceItem> = prev
            .iter()
            .filter(|i| i.task_no == item.task_no && i.step_no.is_some())
            .copied()
            .collect();
        b.retire(item, &children);
    }
    let changed = b.d.tasks_added + b.d.tasks_removed + b.d.tasks_edited;
    let base = prev_tasks.len().max(parsed.tasks.len()).max(1);
    #[allow(clippy::cast_precision_loss)]
    let ratio = changed as f64 / base as f64;
    b.d.changed_ratio = ratio;
    b.d.replan = ratio > REPLAN_RATIO && !prev_tasks.is_empty();
    b.d
}
