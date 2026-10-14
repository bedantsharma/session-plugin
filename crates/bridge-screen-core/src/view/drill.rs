//! Drill-down models (spec §10.2): a project's plan tree and scope line, a session's
//! timeline, a task's decisions and verification evidence, the raw history and the
//! drift-tuning changes. Read-only, like the glance; `now` is a parameter.

use std::collections::{BTreeMap, HashMap, HashSet};

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use super::{ActionSpec, NodeRef, Tone, button, short};
use crate::config::Config;
use crate::decision::{self, Decision};
use crate::error::Result;
use crate::handoff::{self, HandoffNote, LeftoverAction, LeftoverState, ReadbackState};
use crate::owner::OwnerAction;
use crate::plan::{Hill, NodeKind, NodeStatus, PlanNode, load_tree};
use crate::session::{self, SessionState};
use crate::store::{Actor, OWNER_SESSION, repo_name};
use crate::tangent::{self, Label, LabelSource};
use crate::verify::{self, Verification};

/// Most points a scope line carries.
pub const SCOPE_MAX_POINTS: usize = 500;
/// Most history rows per page.
pub const HISTORY_MAX: usize = 200;
/// Longest event data shown in the history, in bytes of JSON.
pub const HISTORY_DATA_MAX: usize = 2048;

/// One plan node in the drill-down tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TreeNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: NodeKind,
    pub title: String,
    pub intent: Option<String>,
    pub status: NodeStatus,
    pub hill: Option<Hill>,
    /// A verification it depended on went stale.
    pub stale: bool,
    /// Its L4 came from the owner.
    pub owner_checked: bool,
    pub estimate_min: Option<u32>,
    /// Active minutes recorded against it.
    pub actual_min: i64,
    pub depends_on: Vec<String>,
    pub decisions: usize,
    pub depth: usize,
}

/// Total live estimate and the part done, at one moment (spec §5.6 scope line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopePoint {
    pub ts: String,
    pub total_min: u64,
    pub done_min: u64,
}

/// A deliberate re-plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Replan {
    pub ts: String,
    pub reason: Option<String>,
    pub op_count: i64,
    pub session_id: String,
}

/// The project drill-down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectDrill {
    pub repo_id: String,
    pub name: String,
    pub tree: Vec<TreeNode>,
    pub scope: Vec<ScopePoint>,
    pub replans: Vec<Replan>,
}

fn actual_minutes(conn: &Connection, repo: &str) -> Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT task_id, active_secs FROM task_time WHERE repo_id = ?1")?;
    let rows = stmt.query_map([repo], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? / 60)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn decision_counts(conn: &Connection, repo: &str) -> Result<HashMap<String, usize>> {
    let mut counts = HashMap::new();
    for d in decision::list(conn, repo, None, false)? {
        if let Some(t) = d.task_id {
            *counts.entry(t).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

pub fn project(conn: &Connection, repo: &str, now: Timestamp) -> Result<ProjectDrill> {
    let _ = now;
    let marks = verify::plan_marks(conn, repo)?;
    let actual = actual_minutes(conn, repo)?;
    let decisions = decision_counts(conn, repo)?;
    let tree = load_tree(conn, repo, false)?
        .into_iter()
        .map(|n: PlanNode| TreeNode {
            stale: marks.get(&n.id).is_some_and(|m| m == "stale"),
            owner_checked: marks.get(&n.id).is_some_and(|m| m == "✓owner"),
            actual_min: actual.get(&n.id).copied().unwrap_or(0),
            decisions: decisions.get(&n.id).copied().unwrap_or(0),
            id: n.id,
            parent_id: n.parent_id,
            kind: n.kind,
            title: n.title,
            intent: n.intent,
            status: n.status,
            hill: n.hill,
            estimate_min: n.estimate_min,
            depends_on: n.depends_on,
            depth: n.depth,
        })
        .collect();
    let mut stmt = conn.prepare(
        "SELECT ts, json_extract(data, '$.reason'), COALESCE(json_extract(data, '$.op_count'), 0),
                session_id
         FROM events WHERE repo_id = ?1 AND kind = 'replan' ORDER BY pos",
    )?;
    let replans = stmt
        .query_map([repo], |r| {
            Ok(Replan {
                ts: r.get(0)?,
                reason: r.get(1)?,
                op_count: r.get(2)?,
                session_id: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ProjectDrill {
        repo_id: repo.to_string(),
        name: repo_name(conn, repo)?.unwrap_or_else(|| repo.to_string()),
        tree,
        scope: scope_line(conn, repo)?,
        replans,
    })
}

/// A plan node as the scope replay sees it.
struct Replayed {
    parent: Option<String>,
    kind: String,
    estimate: u64,
    dropped: bool,
}

/// The plan and its verifications, replayed event by event.
#[derive(Default)]
struct Replay {
    nodes: BTreeMap<String, Replayed>,
    /// verif_id → (task, level, still counts)
    verifs: HashMap<String, (String, i64, bool)>,
}

impl Replay {
    fn apply(&mut self, kind: &str, d: &Value) {
        let id = d["id"].as_str().unwrap_or("").to_string();
        let text = |k: &str| d[k].as_str().map(str::to_string);
        match kind {
            "node_added" => {
                self.nodes.insert(
                    id,
                    Replayed {
                        parent: text("parent_id"),
                        kind: text("kind").unwrap_or_else(|| "task".into()),
                        estimate: d["estimate_min"].as_u64().unwrap_or(0),
                        dropped: false,
                    },
                );
            }
            "node_removed" => self.drop_subtree(id),
            "node_moved" => {
                if let Some(n) = self.nodes.get_mut(&id) {
                    n.parent = text("new_parent_id");
                }
            }
            "estimate_set" => {
                if let Some(n) = self.nodes.get_mut(&id) {
                    n.estimate = d["estimate_min"].as_u64().unwrap_or(0);
                }
            }
            "verified" => {
                let task = text("task_id").unwrap_or_default();
                let level = d["level"].as_i64().unwrap_or(0);
                self.verifs
                    .insert(text("verif_id").unwrap_or_default(), (task, level, true));
            }
            "verification_failed" => {
                let task = d["task_id"].as_str().unwrap_or("");
                for v in self.verifs.values_mut().filter(|v| v.0 == task) {
                    v.2 = false;
                }
            }
            _ => {
                if let Some(v) = self.verifs.get_mut(d["verif_id"].as_str().unwrap_or("")) {
                    v.1 = d["to_level"].as_i64().unwrap_or(0);
                }
            }
        }
    }

    /// The node and its subtree.
    fn drop_subtree(&mut self, id: String) {
        let mut gone: HashSet<String> = HashSet::from([id]);
        loop {
            let more: Vec<String> = self
                .nodes
                .iter()
                .filter(|(k, n)| {
                    !gone.contains(*k) && n.parent.as_ref().is_some_and(|p| gone.contains(p))
                })
                .map(|(k, _)| k.clone())
                .collect();
            if more.is_empty() {
                break;
            }
            gone.extend(more);
        }
        for g in gone {
            if let Some(n) = self.nodes.get_mut(&g) {
                n.dropped = true;
            }
        }
    }

    /// (total live estimate, the part at L2 or above).
    fn totals(&self) -> (u64, u64) {
        let done: HashSet<&str> = self
            .verifs
            .values()
            .filter(|(_, level, counts)| *counts && *level >= 2)
            .map(|(task, _, _)| task.as_str())
            .collect();
        let parents: HashSet<&str> = self
            .nodes
            .values()
            .filter(|n| !n.dropped)
            .filter_map(|n| n.parent.as_deref())
            .collect();
        let (mut total, mut done_min) = (0, 0);
        for (id, n) in &self.nodes {
            let leaf = matches!(n.kind.as_str(), "task" | "subtask")
                && !n.dropped
                && !parents.contains(id.as_str());
            if leaf {
                total += n.estimate;
                if done.contains(id.as_str()) {
                    done_min += n.estimate;
                }
            }
        }
        (total, done_min)
    }
}

/// Replay the plan and verification events into (total live estimate, done estimate)
/// after every event that changes either, downsampled evenly to at most
/// [`SCOPE_MAX_POINTS`] (first and last kept).
pub fn scope_line(conn: &Connection, repo: &str) -> Result<Vec<ScopePoint>> {
    let mut stmt = conn.prepare(
        "SELECT ts, kind, data FROM events WHERE repo_id = ?1 AND kind IN (
           'node_added', 'node_removed', 'node_moved', 'estimate_set', 'verified',
           'verification_failed', 'verification_stale') ORDER BY pos",
    )?;
    let rows = stmt.query_map([repo], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut replay = Replay::default();
    let mut points: Vec<ScopePoint> = Vec::new();
    for row in rows {
        let (ts, kind, data) = row?;
        replay.apply(&kind, &serde_json::from_str(&data)?);
        let (total_min, done_min) = replay.totals();
        if points
            .last()
            .is_none_or(|p| (p.total_min, p.done_min) != (total_min, done_min))
        {
            points.push(ScopePoint {
                ts,
                total_min,
                done_min,
            });
        }
    }
    Ok(downsample(points, SCOPE_MAX_POINTS))
}

/// At most `max` points, evenly spaced, keeping the first and the last.
fn downsample<T: Clone>(points: Vec<T>, max: usize) -> Vec<T> {
    if points.len() <= max || max < 2 {
        return points;
    }
    let n = points.len();
    (0..max)
        .map(|i| points[i * (n - 1) / (max - 1)].clone())
        .collect()
}

/// One unit on a session's timeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnitRow {
    pub unit_no: i64,
    pub started_at: String,
    pub stopped_at: Option<String>,
    pub active_s: i64,
    pub task_id: Option<String>,
    pub label: Option<Label>,
    pub label_source: Option<LabelSource>,
    pub drift_score: Option<f64>,
    /// `D2`, when a detour was open.
    pub detour: Option<String>,
    pub files: usize,
    pub disagreement: bool,
}

/// Where a session's time went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimeSplit {
    pub on_task_s: i64,
    pub drift_s: i64,
    pub unlabeled_s: i64,
    /// Per detour: (label, reason, seconds).
    pub detours: Vec<DetourTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetourTime {
    pub label: String,
    pub reason: String,
    pub secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuspendedRow {
    pub item_id: String,
    pub what: String,
    pub resume_cue: String,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckpointRow {
    pub sha: String,
    pub task_id: Option<String>,
    pub next_action: Option<String>,
    pub ts: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeftoverRow {
    pub leftover_id: String,
    pub path: String,
    pub task_id: Option<String>,
    pub suggested: LeftoverAction,
    pub state: LeftoverState,
    pub resolution: Option<LeftoverAction>,
    pub stash_ref: Option<String>,
    pub commit_sha: Option<String>,
}

/// An owner answer on the session's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerAnswer {
    pub ts: String,
    pub answer: String,
    pub reason: Option<String>,
}

/// The session drill-down.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionDrill {
    pub session_id: String,
    pub short: String,
    pub repo_id: String,
    pub state: SessionState,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub task: Option<NodeRef>,
    pub units: Vec<UnitRow>,
    pub split: TimeSplit,
    pub suspended: Vec<SuspendedRow>,
    pub checkpoints: Vec<CheckpointRow>,
    pub leftovers: Vec<LeftoverRow>,
    pub handoff_note: Option<HandoffNote>,
    pub readback: ReadbackState,
    pub owner_answers: Vec<OwnerAnswer>,
}

/// The session's units and where their time went.
fn timeline(conn: &Connection, id: &str) -> Result<(Vec<UnitRow>, TimeSplit)> {
    let mut labels: HashMap<String, (String, String)> = HashMap::new();
    let mut split = TimeSplit {
        on_task_s: 0,
        drift_s: 0,
        unlabeled_s: 0,
        detours: Vec::new(),
    };
    let mut per_detour: BTreeMap<i64, DetourTime> = BTreeMap::new();
    let mut units = Vec::new();
    for u in session::units(conn, id)? {
        let detour = match &u.detour_id {
            Some(d) => match labels.get(d) {
                Some(l) => Some(l.clone()),
                None => tangent::get_detour(conn, d)?.map(|det| {
                    let v = (det.label(), det.reason.clone());
                    labels.insert(d.clone(), v.clone());
                    per_detour.entry(det.no).or_insert(DetourTime {
                        label: det.label(),
                        reason: det.reason,
                        secs: 0,
                    });
                    v
                }),
            },
            None => None,
        };
        match (u.label, &detour) {
            (Some(Label::OnTask), _) => split.on_task_s += u.active_secs,
            (Some(Label::Drift), _) => split.drift_s += u.active_secs,
            (Some(Label::Detour), Some((label, _))) => {
                if let Some(d) = per_detour.values_mut().find(|d| &d.label == label) {
                    d.secs += u.active_secs;
                }
            }
            _ => split.unlabeled_s += u.active_secs,
        }
        units.push(UnitRow {
            unit_no: u.unit_no,
            started_at: u.started_at,
            stopped_at: u.stopped_at,
            active_s: u.active_secs,
            task_id: u.task_id,
            label: u.label,
            label_source: u.label_source,
            drift_score: u.drift_score,
            detour: detour.map(|(l, _)| l),
            files: u.files.len(),
            disagreement: u.disagreement,
        });
    }
    split.detours = per_detour.into_values().collect();
    Ok((units, split))
}

fn checkpoints(conn: &Connection, id: &str) -> Result<Vec<CheckpointRow>> {
    let mut stmt = conn.prepare(
        "SELECT sha, task_id, next_action, ts FROM checkpoints WHERE session_id = ?1
         ORDER BY ts, rowid",
    )?;
    Ok(stmt
        .query_map([id], |r| {
            Ok(CheckpointRow {
                sha: r.get(0)?,
                task_id: r.get(1)?,
                next_action: r.get(2)?,
                ts: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

/// The session's latest handoff note.
fn handoff_note(conn: &Connection, id: &str) -> Result<Option<HandoffNote>> {
    conn.query_row(
        "SELECT task_id, in_flight, if_then, ts FROM handoff_notes WHERE session_id = ?1
         ORDER BY ts DESC, rowid DESC LIMIT 1",
        [id],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    )
    .optional()?
    .map(|(task_id, in_flight, if_then, ts)| {
        Ok(HandoffNote {
            session_id: id.to_string(),
            task_id,
            in_flight,
            if_then: serde_json::from_str(&if_then)?,
            ts,
        })
    })
    .transpose()
}

/// The owner's drift answers about the session, with their reasons.
fn owner_answers(conn: &Connection, repo: &str, id: &str) -> Result<Vec<OwnerAnswer>> {
    let mut stmt = conn.prepare(
        "SELECT ts, json_extract(data, '$.answer'), json_extract(data, '$.reason') FROM events
         WHERE repo_id = ?1 AND session_id = ?2 AND kind = 'owner_action'
           AND json_extract(data, '$.action') = 'drift' AND json_extract(data, '$.target') = ?3
         ORDER BY pos",
    )?;
    Ok(stmt
        .query_map(params![repo, OWNER_SESSION, id], |r| {
            Ok(OwnerAnswer {
                ts: r.get(0)?,
                answer: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                reason: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn session(conn: &Connection, id: &str, now: Timestamp) -> Result<Option<SessionDrill>> {
    let _ = now;
    let Some(s) = session::get(conn, id)? else {
        return Ok(None);
    };
    let (units, split) = timeline(conn, id)?;
    let mut suspended = Vec::new();
    for item in tangent::suspended(conn, id)? {
        suspended.push(SuspendedRow {
            what: tangent::item_display(conn, &s.repo_id, &item)?,
            item_id: item.item_id,
            resume_cue: item.resume_cue,
            since: item.suspended_at,
        });
    }
    let leftovers = handoff::leftovers(conn, &s.repo_id, false)?
        .into_iter()
        .filter(|l| l.from_session == id)
        .map(|l| LeftoverRow {
            leftover_id: l.leftover_id,
            path: l.path,
            task_id: l.task_id,
            suggested: l.suggested,
            state: l.state,
            resolution: l.resolution,
            stash_ref: l.stash_ref,
            commit_sha: l.commit_sha,
        })
        .collect();
    let task = match &s.active_task {
        Some(t) => Some(super::node_ref(conn, &s.repo_id, t)?),
        None => None,
    };
    Ok(Some(SessionDrill {
        short: short(&s.session_id),
        checkpoints: checkpoints(conn, id)?,
        handoff_note: handoff_note(conn, id)?,
        owner_answers: owner_answers(conn, &s.repo_id, id)?,
        session_id: s.session_id,
        repo_id: s.repo_id,
        state: s.state,
        branch: s.branch,
        worktree: s.worktree,
        task,
        units,
        split,
        suspended,
        leftovers,
        readback: s.readback,
    }))
}

/// An agent that worked on a task.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ActorRow {
    pub agent_type: Option<String>,
    pub model: Option<String>,
}

/// The task drill-down.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskDrill {
    pub repo_id: String,
    pub id: String,
    pub title: String,
    pub status: Option<NodeStatus>,
    pub estimate_min: Option<u32>,
    pub actual_min: i64,
    /// Each live decision with its supersede chain, oldest first.
    pub decisions: Vec<Vec<Decision>>,
    pub verifications: Vec<Verification>,
    pub actors: Vec<ActorRow>,
    /// Units on the task after its latest failed verification.
    pub retries: usize,
    /// Tokens and cost arrive with sub-project 7; always `None` for now.
    pub usage: Option<Value>,
}

pub fn task(conn: &Connection, repo: &str, id: &str) -> Result<TaskDrill> {
    let node = crate::plan::get_node(conn, repo, id)?;
    let mut decisions = Vec::new();
    for d in decision::list(conn, repo, Some(id), false)? {
        decisions.push(decision::chain(conn, repo, &d.decision_id)?);
    }
    // Sessions that worked on it, and the agents and models behind them.
    let mut stmt = conn.prepare(
        "SELECT DISTINCT u.session_id, s.model FROM units u
         JOIN sessions s ON s.session_id = u.session_id
         WHERE s.repo_id = ?1 AND u.task_id = ?2",
    )?;
    let sessions: Vec<(String, Option<String>)> = stmt
        .query_map(params![repo, id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut actors: Vec<ActorRow> = Vec::new();
    let mut add = |row: ActorRow| {
        if !actors.contains(&row) && (row.agent_type.is_some() || row.model.is_some()) {
            actors.push(row);
        }
    };
    for (sid, model) in &sessions {
        add(ActorRow {
            agent_type: None,
            model: model.clone(),
        });
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT actor FROM events WHERE session_id = ?1 AND actor != '{}'",
        )?;
        for a in stmt.query_map([sid], |r| r.get::<_, String>(0))? {
            let a: Actor = serde_json::from_str(&a?)?;
            add(ActorRow {
                agent_type: a.agent_type,
                model: a.model,
            });
        }
    }
    actors.sort();
    let failed_at: Option<String> = conn
        .query_row(
            "SELECT ts FROM events WHERE repo_id = ?1 AND kind = 'verification_failed'
               AND json_extract(data, '$.task_id') = ?2 ORDER BY pos DESC LIMIT 1",
            params![repo, id],
            |r| r.get(0),
        )
        .optional()?;
    let retries = match failed_at
        .as_deref()
        .and_then(|t| t.parse::<Timestamp>().ok())
    {
        Some(after) => {
            let mut stmt = conn.prepare(
                "SELECT u.started_at FROM units u JOIN sessions s ON s.session_id = u.session_id
                 WHERE s.repo_id = ?1 AND u.task_id = ?2",
            )?;
            let starts: Vec<String> = stmt
                .query_map(params![repo, id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            starts
                .iter()
                .filter(|t| t.parse::<Timestamp>().is_ok_and(|t| t > after))
                .count()
        }
        None => 0,
    };
    Ok(TaskDrill {
        repo_id: repo.to_string(),
        id: id.to_string(),
        title: node.as_ref().map(|n| n.title.clone()).unwrap_or_default(),
        status: node.as_ref().map(|n| n.status),
        estimate_min: node.as_ref().and_then(|n| n.estimate_min),
        actual_min: session::task_time(conn, repo, id)?.map_or(0, |(secs, _)| secs / 60),
        decisions,
        verifications: verify::verifications(conn, repo, id)?,
        actors,
        retries,
        usage: None,
    })
}

/// History filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryFilter {
    pub kind: Option<String>,
    pub session: Option<String>,
    pub task: Option<String>,
    /// Only events before this position (the next page).
    pub before: Option<i64>,
    /// At most [`HISTORY_MAX`].
    pub limit: usize,
}

/// One raw event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryRow {
    pub pos: i64,
    pub ts: String,
    pub kind: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub actor: Value,
    /// The payload; a string cut at [`HISTORY_DATA_MAX`] bytes when larger.
    pub data: Value,
}

/// A page of history, newest first.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryPage {
    pub events: Vec<HistoryRow>,
    /// Pass as `before` for the next page; `None` on the last page.
    pub next_before: Option<i64>,
}

pub fn history(conn: &Connection, repo: &str, f: &HistoryFilter) -> Result<HistoryPage> {
    let limit = f.limit.clamp(1, HISTORY_MAX);
    let mut stmt = conn.prepare(
        "SELECT pos, ts, kind, session_id, task_id, actor, data FROM events
         WHERE repo_id = ?1 AND (?2 IS NULL OR kind = ?2) AND (?3 IS NULL OR session_id = ?3)
           AND (?4 IS NULL OR task_id = ?4) AND (?5 IS NULL OR pos < ?5)
         ORDER BY pos DESC LIMIT ?6",
    )?;
    let rows = stmt.query_map(
        params![
            repo,
            f.kind,
            f.session,
            f.task,
            f.before,
            i64::try_from(limit + 1).unwrap_or(i64::MAX)
        ],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        },
    )?;
    let mut events = Vec::new();
    for row in rows {
        let (pos, ts, kind, session_id, task_id, actor, data) = row?;
        let data = if data.len() > HISTORY_DATA_MAX {
            let mut cut = HISTORY_DATA_MAX;
            while !data.is_char_boundary(cut) {
                cut -= 1;
            }
            Value::String(format!("{}…", &data[..cut]))
        } else {
            serde_json::from_str(&data)?
        };
        events.push(HistoryRow {
            pos,
            ts,
            kind,
            session_id,
            task_id,
            actor: serde_json::from_str(&actor)?,
            data,
        });
    }
    let next_before = if events.len() > limit {
        events.truncate(limit);
        events.last().map(|e| e.pos)
    } else {
        None
    };
    Ok(HistoryPage {
        events,
        next_before,
    })
}

/// The three tuned values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Cutoffs {
    pub drift_lower: f64,
    pub drift_upper: f64,
    pub tolerance_pct: f64,
}

/// One `config_changed` of a tuned value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TuningChange {
    pub ts: String,
    pub key: String,
    pub old: Option<f64>,
    pub new: f64,
    pub samples: i64,
}

/// The tuning drill-down (spec §7.6).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TuningDrill {
    pub repo_id: String,
    pub current: Cutoffs,
    pub defaults: Cutoffs,
    pub tuned: bool,
    pub samples: i64,
    /// Newest first.
    pub changes: Vec<TuningChange>,
    /// Put the defaults back (when tuned).
    pub revert: Option<ActionSpec>,
}

pub fn tuning(conn: &Connection, repo: &str, cfg: &Config) -> Result<TuningDrill> {
    let th = tangent::thresholds(conn, repo, cfg)?;
    let defaults = tangent::Thresholds::from_config(cfg);
    let cut = |t: &tangent::Thresholds| Cutoffs {
        drift_lower: t.lower,
        drift_upper: t.upper,
        tolerance_pct: t.tolerance_pct,
    };
    let mut stmt = conn.prepare(
        "SELECT ts, data FROM events WHERE repo_id = ?1 AND kind = 'config_changed'
           AND json_extract(data, '$.key') IN ('drift_lower', 'drift_upper', 'tolerance_pct')
         ORDER BY pos DESC",
    )?;
    let mut changes = Vec::new();
    for row in stmt.query_map([repo], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (ts, data) = row?;
        let d: tangent::ConfigChanged = serde_json::from_str(&data)?;
        changes.push(TuningChange {
            ts,
            key: d.key,
            old: d.old,
            new: d.new,
            samples: d.samples,
        });
    }
    let samples: i64 = conn.query_row(
        "SELECT count(*) FROM drift_samples WHERE repo_id = ?1",
        [repo],
        |r| r.get(0),
    )?;
    let tuned = th != defaults;
    Ok(TuningDrill {
        repo_id: repo.to_string(),
        current: cut(&th),
        defaults: cut(&defaults),
        tuned,
        samples,
        changes,
        revert: tuned.then(|| {
            button(
                "Revert to defaults",
                OwnerAction::ResetDrift {
                    repo: repo.to_string(),
                },
                Tone::Neutral,
            )
        }),
    })
}

#[cfg(test)]
mod tests;
