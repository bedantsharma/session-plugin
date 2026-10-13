//! Validated plan edits. A batch is applied in one transaction: all ops or none.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::model::{Hill, NodeKind, NodeStatus, PlanNode};
use super::project::{
    DepChange, EditMeta, EstimateSet, HillMoved, IntentSet, NodeAdded, NodeEdited, NodeMoved,
    NodeRef,
};
use super::query::{get_node, is_in_subtree, next_child, would_cycle};
use crate::error::Result;
use crate::store::{Actor, Event, EventDraft, Store, append_in};

const MAX_TITLE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanOp {
    AddNode {
        #[serde(default)]
        parent: Option<String>,
        kind: NodeKind,
        title: String,
        #[serde(default)]
        intent: Option<String>,
        #[serde(default)]
        estimate_min: Option<u32>,
        #[serde(default)]
        expected_files: Vec<String>,
    },
    EditNode {
        id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        expected_files: Option<Vec<String>>,
    },
    MoveNode {
        id: String,
        new_parent: String,
    },
    RemoveNode {
        id: String,
    },
    AddDep {
        id: String,
        depends_on: String,
    },
    RemoveDep {
        id: String,
        depends_on: String,
    },
    SetIntent {
        id: String,
        intent: String,
    },
    SetEstimate {
        id: String,
        estimate_min: u32,
    },
    SetHill {
        id: String,
        hill: Hill,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("no operations given")]
    Empty,
    #[error("op {index}: node {id} not found")]
    NotFound { index: usize, id: String },
    #[error("op {index}: node {id} was dropped")]
    Dropped { index: usize, id: String },
    #[error("op {index}: a {child} cannot go under {parent}")]
    BadParent {
        index: usize,
        parent: String,
        child: String,
    },
    #[error("op {index}: the plan already has a goal ({id})")]
    GoalExists { index: usize, id: String },
    #[error("op {index}: a {kind} needs an intent (its purpose and end state)")]
    MissingIntent { index: usize, kind: String },
    #[error("op {index}: title must be 1 to 200 characters")]
    BadTitle { index: usize },
    #[error("op {index}: estimate must be greater than 0 minutes")]
    BadEstimate { index: usize },
    #[error("op {index}: {id} depending on {depends_on} would create a cycle")]
    Cycle {
        index: usize,
        id: String,
        depends_on: String,
    },
    #[error("op {index}: {reason}")]
    Invalid { index: usize, reason: String },
}

/// Who is editing, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditContext {
    pub repo_id: String,
    pub session_id: String,
    pub actor: Actor,
    pub reason: Option<String>,
    /// Record the batch as one deliberate re-plan (requires a reason).
    pub replan: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditOutcome {
    pub events: Vec<Event>,
    /// Ids assigned to added nodes, in op order.
    pub added_ids: Vec<String>,
}

/// Validate and apply `ops` in order, atomically.
pub fn plan_edit(store: &mut Store, ctx: &EditContext, ops: &[PlanOp]) -> Result<EditOutcome> {
    if ops.is_empty() {
        return Err(PlanError::Empty.into());
    }
    store.write(|tx| {
        let mut events = Vec::new();
        let mut added_ids = Vec::new();
        let replan_id = if ctx.replan {
            if ctx.reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
                return Err(PlanError::Invalid {
                    index: 0,
                    reason: "a replan needs a reason".into(),
                }
                .into());
            }
            let data = json!({ "reason": ctx.reason, "op_count": ops.len() });
            let event = append_in(tx, draft(ctx, "replan", data, None))?;
            let id = event.id.clone();
            events.push(event);
            Some(id)
        } else {
            None
        };
        let meta = EditMeta {
            reason: ctx.reason.clone(),
            replan_id,
        };
        for (index, op) in ops.iter().enumerate() {
            let (kind, data, task) = validate(tx, &ctx.repo_id, index, op, &meta)?;
            if kind == "node_added" {
                added_ids.push(task.clone());
            }
            events.push(append_in(tx, draft(ctx, kind, data, Some(task)))?);
        }
        Ok(EditOutcome { events, added_ids })
    })
}

fn draft(ctx: &EditContext, kind: &str, data: Value, task: Option<String>) -> EventDraft {
    let mut d = EventDraft::new(&ctx.repo_id, &ctx.session_id, kind, data).actor(ctx.actor.clone());
    if let Some(task) = task {
        d = d.task(task);
    }
    d
}

/// Look up a node that must exist and not be dropped.
fn live(conn: &Connection, repo: &str, index: usize, id: &str) -> Result<PlanNode> {
    match get_node(conn, repo, id)? {
        None => Err(PlanError::NotFound {
            index,
            id: id.to_string(),
        }
        .into()),
        Some(n) if n.status == NodeStatus::Dropped => Err(PlanError::Dropped {
            index,
            id: id.to_string(),
        }
        .into()),
        Some(n) => Ok(n),
    }
}

fn check_title(index: usize, title: &str) -> Result<String> {
    let t = title.trim();
    if t.is_empty() || t.chars().count() > MAX_TITLE_CHARS {
        return Err(PlanError::BadTitle { index }.into());
    }
    Ok(t.to_string())
}

fn invalid(index: usize, reason: impl Into<String>) -> crate::error::Error {
    PlanError::Invalid {
        index,
        reason: reason.into(),
    }
    .into()
}

/// An accepted op: (event kind, event data, task id).
type Validated = (&'static str, Value, String);

/// Where an op is being validated: the views to check against and the op's position.
#[derive(Clone, Copy)]
struct Check<'a> {
    conn: &'a Connection,
    repo: &'a str,
    index: usize,
}

/// Validate one op against the current views. Returns (event kind, data, task id).
fn validate(
    conn: &Connection,
    repo: &str,
    index: usize,
    op: &PlanOp,
    meta: &EditMeta,
) -> Result<Validated> {
    let c = Check { conn, repo, index };
    let meta = meta.clone();
    match op {
        PlanOp::AddNode {
            parent,
            kind,
            title,
            intent,
            estimate_min,
            expected_files,
        } => {
            let title = check_title(index, title)?;
            let intent = intent.as_deref().map(str::trim).filter(|i| !i.is_empty());
            if kind.requires_intent() && intent.is_none() {
                return Err(PlanError::MissingIntent {
                    index,
                    kind: kind.as_str().into(),
                }
                .into());
            }
            if estimate_min == &Some(0) {
                return Err(PlanError::BadEstimate { index }.into());
            }
            let (id, parent_id) = c.assign_id(*kind, parent.as_deref())?;
            let data = NodeAdded {
                id: id.clone(),
                parent_id,
                kind: *kind,
                title,
                intent: intent.map(str::to_string),
                estimate_min: *estimate_min,
                expected_files: expected_files.clone(),
                meta,
            };
            Ok(("node_added", serde_json::to_value(data)?, id))
        }
        PlanOp::EditNode {
            id,
            title,
            expected_files,
        } => c.edit_node(id, title.as_deref(), expected_files.as_ref(), meta),
        PlanOp::MoveNode { id, new_parent } => c.move_node(id, new_parent, meta),
        PlanOp::RemoveNode { id } => c.remove_node(id, meta),
        PlanOp::AddDep { id, depends_on } => c.add_dep(id, depends_on, meta),
        PlanOp::RemoveDep { id, depends_on } => c.remove_dep(id, depends_on, meta),
        PlanOp::SetIntent { id, intent } => c.set_intent(id, intent, meta),
        PlanOp::SetEstimate { id, estimate_min } => c.set_estimate(id, *estimate_min, meta),
        PlanOp::SetHill { id, hill } => c.set_hill(id, *hill, meta),
    }
}

impl Check<'_> {
    fn live(self, id: &str) -> Result<PlanNode> {
        live(self.conn, self.repo, self.index, id)
    }

    fn bad_parent(self, parent: &str, child: NodeKind) -> crate::error::Error {
        PlanError::BadParent {
            index: self.index,
            parent: parent.into(),
            child: child.as_str().into(),
        }
        .into()
    }

    /// The id a new node of `kind` under `parent` gets, plus its parent id.
    fn assign_id(self, kind: NodeKind, parent: Option<&str>) -> Result<(String, Option<String>)> {
        match (kind, parent) {
            (NodeKind::Goal, None) => {
                if let Some(g) = get_node(self.conn, self.repo, "G")?
                    && g.status != NodeStatus::Dropped
                {
                    return Err(PlanError::GoalExists {
                        index: self.index,
                        id: g.id,
                    }
                    .into());
                }
                Ok(("G".to_string(), None))
            }
            (_, None) => Err(self.bad_parent("nothing", kind)),
            (_, Some(pid)) => {
                let p = self.live(pid)?;
                if !p.kind.allows_child(kind) {
                    return Err(self.bad_parent(p.kind.as_str(), kind));
                }
                let n = next_child(self.conn, self.repo, &p.id)?;
                let id = if p.kind == NodeKind::Goal {
                    n.to_string()
                } else {
                    format!("{}.{n}", p.id)
                };
                Ok((id, Some(p.id)))
            }
        }
    }

    fn edit_node(
        self,
        id: &str,
        title: Option<&str>,
        expected_files: Option<&Vec<String>>,
        meta: EditMeta,
    ) -> Result<Validated> {
        self.live(id)?;
        let title = title.map(|t| check_title(self.index, t)).transpose()?;
        if title.is_none() && expected_files.is_none() {
            return Err(invalid(
                self.index,
                "edit_node needs a title or expected_files",
            ));
        }
        let data = NodeEdited {
            id: id.to_string(),
            title,
            expected_files: expected_files.cloned(),
            meta,
        };
        Ok(("node_edited", serde_json::to_value(data)?, id.to_string()))
    }

    fn move_node(self, id: &str, new_parent: &str, meta: EditMeta) -> Result<Validated> {
        let node = self.live(id)?;
        if node.kind == NodeKind::Goal {
            return Err(invalid(self.index, "the goal cannot be moved"));
        }
        let parent = self.live(new_parent)?;
        if is_in_subtree(self.conn, self.repo, id, new_parent)? {
            return Err(invalid(
                self.index,
                format!("{id} cannot move under itself or its children"),
            ));
        }
        if !parent.kind.allows_child(node.kind) {
            return Err(self.bad_parent(parent.kind.as_str(), node.kind));
        }
        let data = NodeMoved {
            id: id.to_string(),
            new_parent_id: parent.id,
            meta,
        };
        Ok(("node_moved", serde_json::to_value(data)?, id.to_string()))
    }

    fn remove_node(self, id: &str, meta: EditMeta) -> Result<Validated> {
        let node = self.live(id)?;
        if node.kind == NodeKind::Goal {
            return Err(invalid(self.index, "the goal cannot be removed"));
        }
        let data = NodeRef {
            id: id.to_string(),
            meta,
        };
        Ok(("node_removed", serde_json::to_value(data)?, id.to_string()))
    }

    fn add_dep(self, id: &str, depends_on: &str, meta: EditMeta) -> Result<Validated> {
        let index = self.index;
        let node = self.live(id)?;
        let target = self.live(depends_on)?;
        if node.id == target.id {
            return Err(invalid(index, format!("{id} cannot depend on itself")));
        }
        if node.kind == NodeKind::Goal || target.kind == NodeKind::Goal {
            return Err(invalid(index, "the goal cannot take part in dependencies"));
        }
        if node.depends_on.iter().any(|d| d == depends_on) {
            return Err(invalid(
                index,
                format!("{id} already depends on {depends_on}"),
            ));
        }
        if would_cycle(self.conn, self.repo, id, depends_on)? {
            return Err(PlanError::Cycle {
                index,
                id: id.to_string(),
                depends_on: depends_on.to_string(),
            }
            .into());
        }
        let data = DepChange {
            id: id.to_string(),
            depends_on: depends_on.to_string(),
            meta,
        };
        Ok(("dep_added", serde_json::to_value(data)?, id.to_string()))
    }

    fn remove_dep(self, id: &str, depends_on: &str, meta: EditMeta) -> Result<Validated> {
        let node = self.live(id)?;
        if !node.depends_on.iter().any(|d| d == depends_on) {
            return Err(invalid(
                self.index,
                format!("{id} does not depend on {depends_on}"),
            ));
        }
        let data = DepChange {
            id: id.to_string(),
            depends_on: depends_on.to_string(),
            meta,
        };
        Ok(("dep_removed", serde_json::to_value(data)?, id.to_string()))
    }

    fn set_intent(self, id: &str, intent: &str, meta: EditMeta) -> Result<Validated> {
        let node = self.live(id)?;
        let intent = intent.trim();
        if intent.is_empty() {
            return Err(PlanError::MissingIntent {
                index: self.index,
                kind: node.kind.as_str().into(),
            }
            .into());
        }
        let data = IntentSet {
            id: id.to_string(),
            intent: intent.to_string(),
            meta,
        };
        Ok(("intent_set", serde_json::to_value(data)?, id.to_string()))
    }

    fn set_estimate(self, id: &str, estimate_min: u32, meta: EditMeta) -> Result<Validated> {
        self.live(id)?;
        if estimate_min == 0 {
            return Err(PlanError::BadEstimate { index: self.index }.into());
        }
        let data = EstimateSet {
            id: id.to_string(),
            estimate_min,
            meta,
        };
        Ok(("estimate_set", serde_json::to_value(data)?, id.to_string()))
    }

    fn set_hill(self, id: &str, hill: Hill, meta: EditMeta) -> Result<Validated> {
        let node = self.live(id)?;
        if node.kind != NodeKind::Milestone {
            return Err(invalid(
                self.index,
                "only milestones have an uphill/downhill state",
            ));
        }
        let data = HillMoved {
            id: id.to_string(),
            hill,
            meta,
        };
        Ok(("hill_moved", serde_json::to_value(data)?, id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::plan::{NodeStatus, get_node, load_tree};
    use crate::store::{Actor, Store, read_all};

    fn ctx() -> EditContext {
        EditContext {
            repo_id: "r".into(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: Some("setup".into()),
            replan: false,
        }
    }

    fn goal() -> PlanOp {
        PlanOp::AddNode {
            parent: None,
            kind: NodeKind::Goal,
            title: "Ship".into(),
            intent: Some("Owner sees progress".into()),
            estimate_min: None,
            expected_files: vec![],
        }
    }

    fn node(parent: &str, kind: NodeKind, title: &str) -> PlanOp {
        PlanOp::AddNode {
            parent: Some(parent.into()),
            kind,
            title: title.into(),
            intent: kind.requires_intent().then(|| "why".to_string()),
            estimate_min: None,
            expected_files: vec![],
        }
    }

    fn plan_error(result: Result<EditOutcome>) -> PlanError {
        match result {
            Err(Error::Plan(e)) => e,
            other => panic!("expected a plan error, got {other:?}"),
        }
    }

    fn seeded() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        plan_edit(
            &mut s,
            &ctx(),
            &[
                goal(),
                node("G", NodeKind::Milestone, "Core"),
                node("1", NodeKind::Task, "Schema"),
                node("1", NodeKind::Task, "Append"),
            ],
        )
        .unwrap();
        s
    }

    #[test]
    fn ids_follow_the_tree() {
        let mut s = Store::open_in_memory().unwrap();
        let out = plan_edit(
            &mut s,
            &ctx(),
            &[
                goal(),
                node("G", NodeKind::Milestone, "Core"),
                node("1", NodeKind::Task, "Schema"),
                node("1.1", NodeKind::Subtask, "Write SQL"),
                node("G", NodeKind::Task, "Loose task"),
            ],
        )
        .unwrap();
        assert_eq!(out.added_ids, vec!["G", "1", "1.1", "1.1.1", "2"]);
        assert_eq!(out.events.len(), 5);
        assert!(out.events.iter().all(|e| e.data["reason"] == "setup"));
        assert_eq!(out.events[2].task_id.as_deref(), Some("1.1"));
    }

    #[test]
    fn ids_are_never_reused_after_removal() {
        let mut s = seeded();
        plan_edit(&mut s, &ctx(), &[PlanOp::RemoveNode { id: "1.2".into() }]).unwrap();
        let out = plan_edit(&mut s, &ctx(), &[node("1", NodeKind::Task, "Again")]).unwrap();
        assert_eq!(out.added_ids, vec!["1.3"]);
    }

    #[test]
    fn invalid_op_rolls_back_whole_batch() {
        let mut s = seeded();
        let before = read_all(s.conn(), None).unwrap().len();
        let err = plan_error(plan_edit(
            &mut s,
            &ctx(),
            &[
                node("1", NodeKind::Task, "Ok one"),
                PlanOp::SetEstimate {
                    id: "1.1".into(),
                    estimate_min: 30,
                },
                node("1.1", NodeKind::Milestone, "Wrong kind"),
            ],
        ));
        assert_eq!(
            err,
            PlanError::BadParent {
                index: 2,
                parent: "task".into(),
                child: "milestone".into()
            }
        );
        assert_eq!(read_all(s.conn(), None).unwrap().len(), before);
        assert!(get_node(s.conn(), "r", "1.3").unwrap().is_none());
        assert_eq!(
            get_node(s.conn(), "r", "1.1")
                .unwrap()
                .unwrap()
                .estimate_min,
            None
        );
    }

    #[test]
    fn structural_rules() {
        let mut s = seeded();
        assert_eq!(plan_error(plan_edit(&mut s, &ctx(), &[])), PlanError::Empty);
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[goal()])),
            PlanError::GoalExists {
                index: 0,
                id: "G".into()
            }
        );
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[node("9", NodeKind::Task, "x")])),
            PlanError::NotFound {
                index: 0,
                id: "9".into()
            }
        );
        let no_intent = PlanOp::AddNode {
            parent: Some("G".into()),
            kind: NodeKind::Milestone,
            title: "M".into(),
            intent: Some("   ".into()),
            estimate_min: None,
            expected_files: vec![],
        };
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[no_intent])),
            PlanError::MissingIntent {
                index: 0,
                kind: "milestone".into()
            }
        );
        assert_eq!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[node("1", NodeKind::Task, "  ")]
            )),
            PlanError::BadTitle { index: 0 }
        );
        assert_eq!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[node("1", NodeKind::Task, &"x".repeat(201))]
            )),
            PlanError::BadTitle { index: 0 }
        );
        assert_eq!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::SetEstimate {
                    id: "1.1".into(),
                    estimate_min: 0
                }]
            )),
            PlanError::BadEstimate { index: 0 }
        );
        let orphan = PlanOp::AddNode {
            parent: None,
            kind: NodeKind::Task,
            title: "t".into(),
            intent: None,
            estimate_min: None,
            expected_files: vec![],
        };
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[orphan])),
            PlanError::BadParent {
                index: 0,
                parent: "nothing".into(),
                child: "task".into()
            }
        );
    }

    #[test]
    fn dependency_rules() {
        let mut s = seeded();
        let dep = |id: &str, on: &str| PlanOp::AddDep {
            id: id.into(),
            depends_on: on.into(),
        };
        plan_edit(&mut s, &ctx(), &[dep("1.2", "1.1")]).unwrap();
        assert_eq!(
            plan_error(plan_edit(&mut s, &ctx(), &[dep("1.1", "1.2")])),
            PlanError::Cycle {
                index: 0,
                id: "1.1".into(),
                depends_on: "1.2".into()
            }
        );
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[dep("1.1", "1.1")])),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[dep("1.1", "G")])),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(&mut s, &ctx(), &[dep("1.2", "1.1")])),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::RemoveDep {
                    id: "1.1".into(),
                    depends_on: "1.2".into()
                }]
            )),
            PlanError::Invalid { index: 0, .. }
        ));
        plan_edit(
            &mut s,
            &ctx(),
            &[PlanOp::RemoveDep {
                id: "1.2".into(),
                depends_on: "1.1".into(),
            }],
        )
        .unwrap();
        let deps = get_node(s.conn(), "r", "1.2").unwrap().unwrap().depends_on;
        assert_eq!(deps, Vec::<String>::new());
    }

    #[test]
    fn move_remove_and_field_rules() {
        let mut s = seeded();
        plan_edit(&mut s, &ctx(), &[node("G", NodeKind::Milestone, "Screens")]).unwrap();
        plan_edit(
            &mut s,
            &ctx(),
            &[PlanOp::MoveNode {
                id: "1.2".into(),
                new_parent: "2".into(),
            }],
        )
        .unwrap();
        assert_eq!(
            get_node(s.conn(), "r", "1.2")
                .unwrap()
                .unwrap()
                .parent_id
                .as_deref(),
            Some("2")
        );
        assert!(matches!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::MoveNode {
                    id: "1".into(),
                    new_parent: "1".into()
                }]
            )),
            PlanError::BadParent { .. } | PlanError::Invalid { .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::RemoveNode { id: "G".into() }]
            )),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::SetHill {
                    id: "1.1".into(),
                    hill: Hill::Uphill
                }]
            )),
            PlanError::Invalid { index: 0, .. }
        ));
        assert!(matches!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::SetIntent {
                    id: "1".into(),
                    intent: " ".into()
                }]
            )),
            PlanError::MissingIntent { index: 0, .. }
        ));
        plan_edit(&mut s, &ctx(), &[PlanOp::RemoveNode { id: "1".into() }]).unwrap();
        assert_eq!(
            get_node(s.conn(), "r", "1.1").unwrap().unwrap().status,
            NodeStatus::Dropped
        );
        assert_eq!(
            plan_error(plan_edit(
                &mut s,
                &ctx(),
                &[PlanOp::EditNode {
                    id: "1.1".into(),
                    title: Some("x".into()),
                    expected_files: None
                }]
            )),
            PlanError::Dropped {
                index: 0,
                id: "1.1".into()
            }
        );
        let live: Vec<String> = load_tree(s.conn(), "r", false)
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(live, vec!["G", "2", "1.2"]);
    }

    #[test]
    fn replan_groups_edits() {
        let mut s = seeded();
        let mut c = ctx();
        c.replan = true;
        c.reason = None;
        assert!(matches!(
            plan_error(plan_edit(
                &mut s,
                &c,
                &[PlanOp::RemoveNode { id: "1.2".into() }]
            )),
            PlanError::Invalid { index: 0, .. }
        ));
        c.reason = Some("scope changed".into());
        let out = plan_edit(
            &mut s,
            &c,
            &[
                PlanOp::RemoveNode { id: "1.2".into() },
                node("1", NodeKind::Task, "New"),
            ],
        )
        .unwrap();
        assert_eq!(out.events.len(), 3);
        assert_eq!(out.events[0].kind, "replan");
        assert_eq!(out.events[0].data["op_count"], 2);
        let replan_id = out.events[0].id.clone();
        assert!(
            out.events[1..]
                .iter()
                .all(|e| e.data["replan_id"] == replan_id.as_str())
        );
    }

    #[test]
    fn ops_parse_from_json() {
        let ops: Vec<PlanOp> = serde_json::from_str(
            r#"[{"op":"add_node","kind":"goal","title":"G","intent":"i"},
                {"op":"set_hill","id":"1","hill":"uphill"},
                {"op":"add_dep","id":"1.2","depends_on":"1.1"}]"#,
        )
        .unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(
            &ops[0],
            PlanOp::AddNode {
                parent: None,
                kind: NodeKind::Goal,
                ..
            }
        ));
    }
}
