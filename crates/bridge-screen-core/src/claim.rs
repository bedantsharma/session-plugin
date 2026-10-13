//! Claiming the active task (spec §2, §11.2 `claim_task`).
//!
//! An agent claims the task it is working on, automatically when confident and after asking
//! the owner otherwise. A claim is one transaction: optionally a new plan node, then
//! `task_released` for the task it switches away from, then `task_claimed`.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::plan::{EditContext, NodeKind, NodeStatus, PlanOp, get_node, load_tree, plan_edit_in};
use crate::session::{self, TaskClaimed, TaskReleased};
use crate::store::{Actor, EventDraft, Store, append_in};

/// A plan task that might be what a prompt is about.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub task_id: String,
    pub title: String,
    /// Share of the prompt's keywords found in the task (0..=1).
    pub score: f32,
    pub status: NodeStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClaimMode {
    /// The agent was confident the prompt matches the task.
    Auto,
    /// The owner confirmed the task.
    Confirmed,
}

impl ClaimMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Confirmed => "confirmed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimTarget {
    Existing(String),
    /// A new task (under a goal or milestone) or subtask (under a task).
    New {
        parent: String,
        title: String,
        estimate_min: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimRequest {
    pub session_id: String,
    pub repo_id: String,
    pub task: ClaimTarget,
    pub mode: ClaimMode,
    /// 0..=1.
    pub confidence: f64,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimOutcome {
    pub task_id: String,
    /// The task was created by this claim.
    pub created: bool,
    /// The task the session switched away from.
    pub released: Option<String>,
}

/// Words too common to say anything about which task a prompt means.
const STOP_WORDS: &[&str] = &[
    "a", "about", "after", "all", "also", "an", "and", "any", "are", "as", "at", "be", "been",
    "but", "by", "can", "could", "do", "does", "for", "from", "get", "has", "have", "how", "i",
    "if", "in", "into", "is", "it", "its", "just", "let", "lets", "like", "make", "me", "more",
    "my", "need", "new", "no", "not", "now", "of", "on", "or", "our", "out", "please", "should",
    "so", "some", "that", "the", "them", "then", "there", "these", "this", "to", "up", "us", "use",
    "want", "was", "we", "what", "when", "which", "will", "with", "would", "you", "your",
];

/// Path parts that appear in nearly every expected file and say nothing about the task.
const PATH_NOISE: &[&str] = &["src", "lib", "mod", "rs", "ts", "js", "py", "md", "index"];

/// Lowercase word tokens, stop-words removed.
fn tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| !STOP_WORDS.contains(&w.as_str()) && !PATH_NOISE.contains(&w.as_str()))
        .collect()
}

/// Live tasks and subtasks (todo, active or failed) ranked by how many of `text`'s keywords
/// appear in their title, intent, expected file names and ancestors' titles (below the
/// goal). Ties keep plan order; tasks sharing no keyword are left out.
pub fn candidates(
    conn: &Connection,
    repo_id: &str,
    text: &str,
    limit: usize,
) -> Result<Vec<Candidate>> {
    let query = tokens(text);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let tree = load_tree(conn, repo_id, false)?;
    // Titles of the nodes on the path from the goal's children down to the current node.
    let mut path: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for node in &tree {
        path.truncate(node.depth.saturating_sub(1));
        let claimable = matches!(node.kind, NodeKind::Task | NodeKind::Subtask)
            && matches!(
                node.status,
                NodeStatus::Todo | NodeStatus::Active | NodeStatus::Failed
            );
        if claimable {
            let mut doc = String::new();
            for part in path
                .iter()
                .copied()
                .chain([node.title.as_str(), node.intent.as_deref().unwrap_or("")])
                .chain(node.expected_files.iter().map(String::as_str))
            {
                doc.push_str(part);
                doc.push(' ');
            }
            let words = tokens(&doc);
            let hits = query.iter().filter(|w| words.contains(*w)).count();
            if hits > 0 {
                #[allow(clippy::cast_precision_loss)] // word counts are tiny
                let score = hits as f32 / query.len() as f32;
                out.push(Candidate {
                    task_id: node.id.clone(),
                    title: node.title.clone(),
                    score,
                    status: node.status,
                });
            }
        }
        if node.kind != NodeKind::Goal {
            path.push(&node.title);
        }
    }
    // Stable sort: equal scores stay in plan order.
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(limit);
    Ok(out)
}

fn claim_err(msg: impl Into<String>) -> Error {
    Error::Claim(msg.into())
}

/// Claim a task for a session, creating it first for [`ClaimTarget::New`].
pub fn claim(store: &mut Store, req: &ClaimRequest) -> Result<ClaimOutcome> {
    store.write(|tx| claim_in(tx, req))
}

fn claim_in(tx: &Transaction, req: &ClaimRequest) -> Result<ClaimOutcome> {
    claim_in_with_reason(tx, req, "claimed as new work")
}

/// [`claim`] inside the caller's transaction; `reason` is recorded on a new node.
pub fn claim_in_with_reason(
    tx: &Transaction,
    req: &ClaimRequest,
    reason: &str,
) -> Result<ClaimOutcome> {
    let session = session::get(tx, &req.session_id)?
        .filter(|s| s.repo_id == req.repo_id)
        .ok_or_else(|| claim_err(format!("unknown session {}", req.session_id)))?;
    if session.state == session::SessionState::Ended {
        return Err(claim_err(format!("session {} has ended", req.session_id)));
    }
    if !(0.0..=1.0).contains(&req.confidence) {
        return Err(claim_err("confidence must be between 0 and 1"));
    }
    let (task_id, created) = match &req.task {
        ClaimTarget::Existing(id) => {
            let node = get_node(tx, &req.repo_id, id)?
                .ok_or_else(|| claim_err(format!("no task {id} in this plan")))?;
            if node.status == NodeStatus::Dropped {
                return Err(claim_err(format!("{id} was dropped")));
            }
            if matches!(node.kind, NodeKind::Goal | NodeKind::Milestone) {
                return Err(claim_err(format!(
                    "{id} is a {}; claim a task or subtask",
                    node.kind.as_str()
                )));
            }
            (node.id, false)
        }
        ClaimTarget::New {
            parent,
            title,
            estimate_min,
        } => {
            let parent_kind = get_node(tx, &req.repo_id, parent)?.map(|n| n.kind);
            let kind = match parent_kind {
                Some(NodeKind::Task | NodeKind::Subtask) => NodeKind::Subtask,
                _ => NodeKind::Task,
            };
            let ctx = EditContext {
                repo_id: req.repo_id.clone(),
                session_id: req.session_id.clone(),
                actor: req.actor.clone(),
                reason: Some(reason.into()),
                replan: false,
            };
            let op = PlanOp::AddNode {
                parent: Some(parent.clone()),
                kind,
                title: title.clone(),
                intent: None,
                estimate_min: *estimate_min,
                expected_files: Vec::new(),
            };
            let outcome = plan_edit_in(tx, &ctx, &[op])?;
            (outcome.added_ids[0].clone(), true)
        }
    };
    let previous = session.active_task.clone();
    let draft = |kind: &str, data: serde_json::Value, task: &str| {
        EventDraft::new(&req.repo_id, &req.session_id, kind, data)
            .actor(req.actor.clone())
            .task(task)
    };
    let mut released = None;
    if let Some(prev) = previous.as_deref().filter(|p| *p != task_id) {
        let data = TaskReleased {
            task_id: prev.to_string(),
            reason: "switched".into(),
        };
        append_in(
            tx,
            draft("task_released", serde_json::to_value(data)?, prev),
        )?;
        released = Some(prev.to_string());
    }
    let data = TaskClaimed {
        task_id: task_id.clone(),
        mode: req.mode.as_str().into(),
        confidence: req.confidence,
        previous,
    };
    append_in(
        tx,
        draft("task_claimed", serde_json::to_value(data)?, &task_id),
    )?;
    Ok(ClaimOutcome {
        task_id,
        created,
        released,
    })
}

/// The title of a live plan node, for messages.
pub fn task_title(conn: &Connection, repo_id: &str, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT title FROM plan_nodes WHERE repo_id = ?1 AND id = ?2",
            params![repo_id, id],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::plan::{EditContext, NodeKind, NodeStatus, PlanOp, get_node, plan_edit};
    use crate::session;
    use crate::store::{Actor, EventDraft, Store, read_all};
    use serde_json::json;

    fn add(parent: &str, kind: NodeKind, title: &str, files: &[&str]) -> PlanOp {
        PlanOp::AddNode {
            parent: Some(parent.into()),
            kind,
            title: title.into(),
            intent: kind.requires_intent().then(|| format!("{title} intent")),
            estimate_min: None,
            expected_files: files.iter().map(ToString::to_string).collect(),
        }
    }

    /// G Ship the app; 1 Authentication {1.1 Login form, 1.2 Session store, 1.3 Password reset};
    /// 2 Storage {2.1 Database schema, 2.2 Backups}
    fn plan() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        let ctx = EditContext {
            repo_id: "r".into(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: None,
            replan: false,
        };
        plan_edit(
            &mut s,
            &ctx,
            &[
                PlanOp::AddNode {
                    parent: None,
                    kind: NodeKind::Goal,
                    title: "Ship the app".into(),
                    intent: Some("Users can sign in and keep their data.".into()),
                    estimate_min: None,
                    expected_files: vec![],
                },
                add("G", NodeKind::Milestone, "Authentication", &[]),
                add("1", NodeKind::Task, "Login form", &["src/ui/login_form.rs"]),
                add(
                    "1",
                    NodeKind::Task,
                    "Session store",
                    &["src/auth/session.rs"],
                ),
                add("1", NodeKind::Task, "Password reset", &[]),
                add("G", NodeKind::Milestone, "Storage", &[]),
                add(
                    "2",
                    NodeKind::Task,
                    "Database schema",
                    &["migrations/001.sql"],
                ),
                add("2", NodeKind::Task, "Backups", &[]),
            ],
        )
        .unwrap();
        start(&mut s, "s1");
        start(&mut s, "s2");
        s
    }

    fn start(s: &mut Store, session: &str) {
        s.append(EventDraft::new(
            "r",
            session,
            "session_started",
            json!({"cwd": "/r", "source": "startup"}),
        ))
        .unwrap();
    }

    fn req(session: &str, task: ClaimTarget) -> ClaimRequest {
        ClaimRequest {
            session_id: session.into(),
            repo_id: "r".into(),
            task,
            mode: ClaimMode::Auto,
            confidence: 0.8,
            actor: Actor::default(),
        }
    }

    fn existing(id: &str) -> ClaimTarget {
        ClaimTarget::Existing(id.into())
    }

    fn status(s: &Store, id: &str) -> NodeStatus {
        get_node(s.conn(), "r", id).unwrap().unwrap().status
    }

    fn active_task(s: &Store, session: &str) -> Option<String> {
        session::get(s.conn(), session)
            .unwrap()
            .unwrap()
            .active_task
    }

    fn claim_error(result: crate::error::Result<ClaimOutcome>) -> String {
        match result {
            Err(Error::Claim(msg)) => msg,
            other => panic!("expected a claim error, got {other:?}"),
        }
    }

    #[test]
    fn candidates_rank_by_keyword_overlap() {
        let s = plan();
        let ids = |text: &str| -> Vec<String> {
            candidates(s.conn(), "r", text, 3)
                .unwrap()
                .into_iter()
                .map(|c| c.task_id)
                .collect()
        };
        // "login" and "form" hit 1.1; "session" hits 1.2; "authentication" (milestone
        // title) hits all three auth tasks, so ties fall back to plan order.
        assert_eq!(
            ids("Please fix the login form and the session timeout"),
            vec!["1.1", "1.2"]
        );
        assert_eq!(ids("authentication is broken"), vec!["1.1", "1.2", "1.3"]);
        // Expected file names count too.
        assert_eq!(ids("write migrations for the schema"), vec!["2.1"]);
        assert_eq!(ids("the of and"), Vec::<String>::new());

        let top = &candidates(s.conn(), "r", "login form", 1).unwrap()[0];
        assert_eq!(top.title, "Login form");
        assert_eq!(top.status, NodeStatus::Todo);
        assert!((top.score - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn candidates_skip_done_and_dropped_tasks() {
        let s = plan();
        s.conn()
            .execute(
                "UPDATE plan_nodes SET status = 'l2' WHERE repo_id = 'r' AND id = '1.1'",
                [],
            )
            .unwrap();
        s.conn()
            .execute(
                "UPDATE plan_nodes SET status = 'dropped' WHERE repo_id = 'r' AND id = '1.2'",
                [],
            )
            .unwrap();
        s.conn()
            .execute(
                "UPDATE plan_nodes SET status = 'failed' WHERE repo_id = 'r' AND id = '1.3'",
                [],
            )
            .unwrap();
        let got: Vec<String> = candidates(s.conn(), "r", "authentication", 5)
            .unwrap()
            .into_iter()
            .map(|c| c.task_id)
            .collect();
        assert_eq!(got, vec!["1.3"]);
    }

    #[test]
    fn claim_existing_then_switch() {
        let mut s = plan();
        let out = claim(&mut s, &req("s1", existing("1.1"))).unwrap();
        assert_eq!(
            out,
            ClaimOutcome {
                task_id: "1.1".into(),
                created: false,
                released: None
            }
        );
        assert_eq!(active_task(&s, "s1").as_deref(), Some("1.1"));
        assert_eq!(status(&s, "1.1"), NodeStatus::Active);

        let out = claim(&mut s, &req("s1", existing("1.2"))).unwrap();
        assert_eq!(out.released.as_deref(), Some("1.1"));
        assert_eq!(active_task(&s, "s1").as_deref(), Some("1.2"));
        assert_eq!(status(&s, "1.1"), NodeStatus::Todo);
        assert_eq!(status(&s, "1.2"), NodeStatus::Active);

        let kinds: Vec<(String, serde_json::Value)> = read_all(s.conn(), None)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind.starts_with("task_"))
            .map(|e| (e.kind, e.data))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (
                    "task_claimed".into(),
                    json!({"task_id": "1.1", "mode": "auto", "confidence": 0.8, "previous": null})
                ),
                (
                    "task_released".into(),
                    json!({"task_id": "1.1", "reason": "switched"})
                ),
                (
                    "task_claimed".into(),
                    json!({"task_id": "1.2", "mode": "auto", "confidence": 0.8, "previous": "1.1"})
                ),
            ]
        );
    }

    #[test]
    fn claim_new_task_and_subtask() {
        let mut s = plan();
        let out = claim(
            &mut s,
            &req(
                "s1",
                ClaimTarget::New {
                    parent: "2".into(),
                    title: "Restore drill".into(),
                    estimate_min: Some(45),
                },
            ),
        )
        .unwrap();
        assert_eq!((out.task_id.as_str(), out.created), ("2.3", true));
        let node = get_node(s.conn(), "r", "2.3").unwrap().unwrap();
        assert_eq!(node.kind, NodeKind::Task);
        assert_eq!(node.estimate_min, Some(45));
        assert_eq!(node.status, NodeStatus::Active);

        let out = claim(
            &mut s,
            &req(
                "s1",
                ClaimTarget::New {
                    parent: "2.3".into(),
                    title: "Write the runbook".into(),
                    estimate_min: None,
                },
            ),
        )
        .unwrap();
        assert_eq!(out.task_id, "2.3.1");
        assert_eq!(out.released.as_deref(), Some("2.3"));
        assert_eq!(
            get_node(s.conn(), "r", "2.3.1").unwrap().unwrap().kind,
            NodeKind::Subtask
        );
        // Parent missing → the plan error, nothing written.
        let before = read_all(s.conn(), None).unwrap().len();
        let err = claim(
            &mut s,
            &req(
                "s1",
                ClaimTarget::New {
                    parent: "9".into(),
                    title: "x".into(),
                    estimate_min: None,
                },
            ),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "op 0: node 9 not found");
        assert_eq!(read_all(s.conn(), None).unwrap().len(), before);
    }

    #[test]
    fn claim_rejects_non_tasks_and_bad_input() {
        let mut s = plan();
        assert_eq!(
            claim_error(claim(&mut s, &req("s1", existing("G")))),
            "G is a goal; claim a task or subtask"
        );
        assert_eq!(
            claim_error(claim(&mut s, &req("s1", existing("1")))),
            "1 is a milestone; claim a task or subtask"
        );
        assert_eq!(
            claim_error(claim(&mut s, &req("s1", existing("7.7")))),
            "no task 7.7 in this plan"
        );
        plan_edit(
            &mut s,
            &EditContext {
                repo_id: "r".into(),
                session_id: "owner".into(),
                actor: Actor::default(),
                reason: None,
                replan: false,
            },
            &[PlanOp::RemoveNode { id: "2.2".into() }],
        )
        .unwrap();
        assert_eq!(
            claim_error(claim(&mut s, &req("s1", existing("2.2")))),
            "2.2 was dropped"
        );
        let mut bad = req("s1", existing("1.1"));
        bad.confidence = 1.5;
        assert_eq!(
            claim_error(claim(&mut s, &bad)),
            "confidence must be between 0 and 1"
        );
        assert_eq!(
            claim_error(claim(&mut s, &req("nobody", existing("1.1")))),
            "unknown session nobody"
        );
        assert_eq!(active_task(&s, "s1"), None);
        s.append(EventDraft::new(
            "r",
            "s2",
            "session_ended",
            json!({"reason": "other"}),
        ))
        .unwrap();
        assert_eq!(
            claim_error(claim(&mut s, &req("s2", existing("1.1")))),
            "session s2 has ended"
        );
        assert_eq!(status(&s, "1.1"), NodeStatus::Todo);
    }

    #[test]
    fn two_sessions_on_one_task() {
        let mut s = plan();
        claim(&mut s, &req("s1", existing("1.1"))).unwrap();
        claim(&mut s, &req("s2", existing("1.1"))).unwrap();
        // s1 moves on; s2 still holds 1.1, so it stays active.
        claim(&mut s, &req("s1", existing("2.1"))).unwrap();
        assert_eq!(status(&s, "1.1"), NodeStatus::Active);
        // s2 ends: the task is released and goes back to todo.
        s.append(EventDraft::new(
            "r",
            "s2",
            "session_ended",
            json!({"reason": "other"}),
        ))
        .unwrap();
        assert_eq!(status(&s, "1.1"), NodeStatus::Todo);
        assert_eq!(active_task(&s, "s2"), None);
        assert_eq!(status(&s, "2.1"), NodeStatus::Active);
    }

    #[test]
    fn claim_tags_the_open_unit_and_keeps_verified_status() {
        let mut s = plan();
        s.append(EventDraft::new("r", "s1", "prompt", json!({"chars": 3})))
            .unwrap();
        s.conn()
            .execute(
                "UPDATE plan_nodes SET status = 'l2' WHERE repo_id = 'r' AND id = '1.1'",
                [],
            )
            .unwrap();
        claim(&mut s, &req("s1", existing("1.1"))).unwrap();
        let unit = session::current_unit(s.conn(), "s1").unwrap().unwrap();
        assert_eq!(unit.task_id.as_deref(), Some("1.1"));
        assert_eq!(status(&s, "1.1"), NodeStatus::L2);
    }
}
