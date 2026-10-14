//! Opening, closing, resuming and promoting detours (spec §7.4, §11.2). Each operation is
//! one transaction, and each records the return-to-task note its result carries.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    Detour, DetourClosed, DetourOpened, DetourPromoted, ItemKind, ItemResumed, ItemSuspended,
    Label, LabelSource, SuspendedItem, Thresholds, UnitLabeled, get_detour, get_item, open_detours,
    suspended,
};
use crate::claim::{self, ClaimMode, ClaimRequest, ClaimTarget, task_title};
use crate::error::{Error, Result};
use crate::navigator::{Candidate, Channel, NoteKind, note_sent_draft};
use crate::plan::{NodeStatus, get_node};
use crate::session::{self, Session, SessionState};
use crate::store::{Actor, EventDraft, Store, append_in};

/// Longest resume cue or detour reason.
pub const MAX_CUE_CHARS: usize = 200;

const BAD_CUE: &str = "resume_cue is required: one line (at most 200 characters) naming the next action and the file or test";
const BAD_REASON: &str = "reason is required: one line, at most 200 characters";

fn err(msg: impl Into<String>) -> Error {
    Error::Tangent(msg.into())
}

/// One non-empty line of at most [`MAX_CUE_CHARS`] characters, trimmed.
fn one_line(text: &str, error: &str) -> Result<String> {
    let t = text.trim();
    if t.is_empty() || t.contains('\n') || t.chars().count() > MAX_CUE_CHARS {
        return Err(err(error));
    }
    Ok(t.to_string())
}

fn live_session(conn: &Connection, session_id: &str) -> Result<Session> {
    let s = session::get(conn, session_id)?
        .ok_or_else(|| err(format!("unknown session {session_id}")))?;
    if s.state == SessionState::Ended {
        return Err(err(format!("session {session_id} has ended")));
    }
    Ok(s)
}

/// The unit a note goes out in: the open one, else the last one.
fn note_unit(conn: &Connection, session_id: &str) -> Result<i64> {
    Ok(session::last_unit(conn, session_id)?.map_or(0, |u| u.unit_no))
}

struct Writer<'a> {
    tx: &'a Transaction<'a>,
    session: &'a Session,
    actor: &'a Actor,
}

impl Writer<'_> {
    fn append(&self, kind: &str, data: &impl Serialize, task: Option<&str>) -> Result<()> {
        let mut draft = EventDraft::new(
            &self.session.repo_id,
            &self.session.session_id,
            kind,
            serde_json::to_value(data)?,
        )
        .actor(self.actor.clone());
        if let Some(t) = task {
            draft = draft.task(t);
        }
        append_in(self.tx, draft)?;
        Ok(())
    }

    /// Record the return-to-task note carried by a tool result.
    fn note(&self, text: &str, state_key: &str) -> Result<()> {
        let unit_no = note_unit(self.tx, &self.session.session_id)?;
        let note = Candidate::new(NoteKind::ReturnToTask, Channel::ToolResult, text, state_key);
        let draft = note_sent_draft(
            &self.session.repo_id,
            &self.session.session_id,
            unit_no,
            &note,
        )?
        .actor(self.actor.clone());
        append_in(self.tx, draft)?;
        Ok(())
    }
}

/// How a suspended item reads in messages: `1.2 Login form` or `D1 build is broken`.
pub fn item_display(conn: &Connection, repo_id: &str, item: &SuspendedItem) -> Result<String> {
    Ok(match item.kind {
        ItemKind::Task => {
            let title = task_title(conn, repo_id, &item.ref_id)?.unwrap_or_default();
            format!("{} {title}", item.ref_id).trim_end().to_string()
        }
        ItemKind::Detour => match get_detour(conn, &item.ref_id)? {
            Some(d) => format!("{} {}", d.label(), d.reason),
            None => item.ref_id.clone(),
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    pub session_id: String,
    pub reason: String,
    pub resume_cue: String,
    /// Defaults to the session's active task.
    pub blocks: Option<String>,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub detour: Detour,
    /// What opening the detour suspended (the task or the outer detour), if anything.
    pub suspended: Option<SuspendedItem>,
}

/// Open a detour and suspend the current item with its resume cue.
pub fn start_detour(store: &mut Store, req: &StartRequest) -> Result<Started> {
    store.write(|tx| {
        let session = live_session(tx, &req.session_id)?;
        let reason = one_line(&req.reason, BAD_REASON)?;
        let cue = one_line(&req.resume_cue, BAD_CUE)?;
        let blocks = req.blocks.clone().or_else(|| session.active_task.clone());
        if let Some(task) = &blocks
            && get_node(tx, &session.repo_id, task)?.is_none_or(|n| n.status == NodeStatus::Dropped)
        {
            return Err(err(format!("no task {task} in this plan")));
        }
        let w = Writer {
            tx,
            session: &session,
            actor: &req.actor,
        };
        let detour_id = ulid::Ulid::generate().to_string();
        let focus = match (&session.current_detour, &session.active_task) {
            (Some(d), _) => Some((ItemKind::Detour, d.clone())),
            (None, Some(t)) => Some((ItemKind::Task, t.clone())),
            (None, None) => None,
        };
        w.append(
            "detour_opened",
            &DetourOpened {
                detour_id: detour_id.clone(),
                reason,
                blocks: blocks.clone(),
                resume_cue: cue.clone(),
            },
            blocks.as_deref(),
        )?;
        let mut item_id = None;
        if let Some((kind, ref_id)) = focus {
            let id = ulid::Ulid::generate().to_string();
            w.append(
                "item_suspended",
                &ItemSuspended {
                    item_id: id.clone(),
                    kind,
                    ref_id,
                    resume_cue: cue.clone(),
                    by_detour: Some(detour_id.clone()),
                },
                None,
            )?;
            item_id = Some(id);
        }
        w.note(&cue, &format!("detour:{detour_id}:open"))?;
        let detour =
            get_detour(tx, &detour_id)?.ok_or_else(|| err("detour vanished after opening"))?;
        let suspended = match item_id {
            Some(id) => get_item(tx, &id)?,
            None => None,
        };
        Ok(Started { detour, suspended })
    })
}

/// How a detour ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DetourOutcome {
    /// The blocker is dealt with.
    Done,
    /// Given up; the task goes on without it.
    Abandoned,
}

impl DetourOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Abandoned => "abandoned",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseRequest {
    pub session_id: String,
    /// `D<n>` or a detour id; defaults to the session's current detour.
    pub detour: Option<String>,
    pub outcome: DetourOutcome,
    pub note: Option<String>,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    pub detour: Detour,
    /// The item the detour had suspended, now resumed (only when it was the focus).
    pub resumed: Option<SuspendedItem>,
    /// The closed detour was what the session was working on.
    pub was_focus: bool,
}

/// One of the session's open detours, by `D<n>` or id.
fn find_open(conn: &Connection, session_id: &str, which: &str) -> Result<Option<Detour>> {
    Ok(open_detours(conn, session_id)?
        .into_iter()
        .find(|d| d.detour_id == which || d.label().eq_ignore_ascii_case(which)))
}

/// What closing the focused detour `detour_id` returns to: the item its opening suspended.
/// If that item is a detour closed in the meantime (out of order), the walk continues to
/// what *that* detour suspended, so the session lands on the nearest open item.
fn return_item(
    conn: &Connection,
    session_id: &str,
    detour_id: &str,
) -> Result<Option<SuspendedItem>> {
    let mut stmt = conn.prepare_cached(
        "SELECT item_id FROM suspended_items
         WHERE session_id = ?1 AND by_detour = ?2 AND resumed_at IS NULL",
    )?;
    let mut by = detour_id.to_string();
    loop {
        let Some(id) = stmt
            .query_row(params![session_id, by], |r| r.get::<_, String>(0))
            .optional()?
        else {
            return Ok(None);
        };
        let Some(item) = get_item(conn, &id)? else {
            return Ok(None);
        };
        let closed = item.kind == ItemKind::Detour
            && get_detour(conn, &item.ref_id)?.is_some_and(|d| d.closed_at.is_some());
        if !closed {
            return Ok(Some(item));
        }
        by = item.ref_id;
    }
}

/// Close a detour. Closing the current one resumes the item it suspended.
pub fn close_detour(store: &mut Store, req: &CloseRequest) -> Result<Closed> {
    store.write(|tx| {
        let session = live_session(tx, &req.session_id)?;
        let detour = match (&req.detour, &session.current_detour) {
            (Some(which), _) => find_open(tx, &session.session_id, which)?
                .ok_or_else(|| err(format!("no open detour {which} in this session")))?,
            (None, Some(id)) => {
                get_detour(tx, id)?.ok_or_else(|| err("no open detour to close"))?
            }
            (None, None) => return Err(err("no open detour to close")),
        };
        let was_focus = session.current_detour.as_deref() == Some(detour.detour_id.as_str());
        let w = Writer {
            tx,
            session: &session,
            actor: &req.actor,
        };
        let back = if was_focus {
            return_item(tx, &session.session_id, &detour.detour_id)?
        } else {
            None
        };
        w.append(
            "detour_closed",
            &DetourClosed {
                detour_id: detour.detour_id.clone(),
                outcome: req.outcome.as_str().into(),
                note: req.note.clone(),
            },
            detour.blocks_task.as_deref(),
        )?;
        // A detour closing is a boundary: its work needs a checkpoint (spec §8.1).
        w.append(
            "checkpoint_due",
            &crate::handoff::CheckpointDue {
                reason: crate::handoff::CheckpointReason::DetourClosed,
            },
            None,
        )?;
        if let Some(item) = &back {
            w.append(
                "item_resumed",
                &ItemResumed {
                    item_id: item.item_id.clone(),
                },
                None,
            )?;
        }
        let cue = back.as_ref().map_or("", |i| i.resume_cue.as_str());
        w.note(cue, &format!("detour:{}:close", detour.detour_id))?;
        let detour = get_detour(tx, &detour.detour_id)?.unwrap_or(detour);
        Ok(Closed {
            detour,
            resumed: back,
            was_focus,
        })
    })
}

/// Resume a suspended item: by its id, the task id it points at, or `D<n>` for a detour.
pub fn resume(
    store: &mut Store,
    session_id: &str,
    which: &str,
    actor: &Actor,
) -> Result<SuspendedItem> {
    store.write(|tx| {
        let session = live_session(tx, session_id)?;
        let mut found = None;
        for item in suspended(tx, session_id)? {
            let matches = item.item_id == which
                || match item.kind {
                    ItemKind::Task => item.ref_id == which,
                    ItemKind::Detour => get_detour(tx, &item.ref_id)?
                        .is_some_and(|d| d.label().eq_ignore_ascii_case(which)),
                };
            if matches {
                found = Some(item);
                break;
            }
        }
        let item =
            found.ok_or_else(|| err(format!("no suspended item {which} in this session")))?;
        let w = Writer {
            tx,
            session: &session,
            actor,
        };
        w.append(
            "item_resumed",
            &ItemResumed {
                item_id: item.item_id.clone(),
            },
            None,
        )?;
        w.note(&item.resume_cue, &format!("resume:{}", item.item_id))?;
        Ok(item)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promoted {
    pub detour: Detour,
    pub task_id: String,
}

/// Turn the session's current detour into a real task under `parent` and claim it
/// (spec §7.3: `detour_promoted` + `node_added`, recorded as scope added mid-stream).
pub fn promote_detour(
    store: &mut Store,
    session_id: &str,
    parent: &str,
    title: &str,
    estimate_min: Option<u32>,
    actor: &Actor,
) -> Result<Promoted> {
    store.write(|tx| promote_detour_in(tx, session_id, parent, title, estimate_min, actor))
}

/// [`promote_detour`] inside the caller's transaction.
pub fn promote_detour_in(
    tx: &Transaction,
    session_id: &str,
    parent: &str,
    title: &str,
    estimate_min: Option<u32>,
    actor: &Actor,
) -> Result<Promoted> {
    {
        let session = live_session(tx, session_id)?;
        let detour = match &session.current_detour {
            Some(id) => get_detour(tx, id)?,
            None => None,
        }
        .ok_or_else(|| err("no open detour to promote"))?;
        let req = ClaimRequest {
            session_id: session.session_id.clone(),
            repo_id: session.repo_id.clone(),
            task: ClaimTarget::New {
                parent: parent.to_string(),
                title: title.to_string(),
                estimate_min,
            },
            mode: ClaimMode::Auto,
            confidence: 1.0,
            actor: actor.clone(),
        };
        let out = claim::claim_in_with_reason(tx, &req, "promoted from detour")?;
        let w = Writer {
            tx,
            session: &session,
            actor,
        };
        w.append(
            "detour_promoted",
            &DetourPromoted {
                detour_id: detour.detour_id.clone(),
                task_id: out.task_id.clone(),
            },
            Some(&out.task_id),
        )?;
        w.note("", &format!("detour:{}:promote", detour.detour_id))?;
        Ok(Promoted {
            detour,
            task_id: out.task_id,
        })
    }
}

/// Longest `why` kept with a label.
const MAX_WHY_CHARS: usize = 200;

/// A label for a unit, from the agent or the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRequest {
    pub session_id: String,
    /// Which unit; `None` picks as described on [`label_unit`].
    pub unit_no: Option<i64>,
    pub label: Label,
    pub source: LabelSource,
    pub why: Option<String>,
}

/// Label a unit (spec §7.2 `label_unit`). With no `unit_no`: the most recent finished unit
/// still waiting for a label, else the last finished unit (a correction). The label is
/// used as given; a difference from the heuristic is recorded, never argued. Returns the
/// unit number.
pub fn label_unit(
    store: &mut Store,
    req: &LabelRequest,
    th: &Thresholds,
    actor: &Actor,
) -> Result<i64> {
    let LabelRequest {
        session_id,
        unit_no,
        label,
        source,
        why,
    } = req;
    let (session_id, unit_no, label, source) = (session_id.as_str(), *unit_no, *label, *source);
    let why = why.as_deref();
    store.write(|tx| label_unit_in(tx, session_id, unit_no, label, source, why, th, actor))
}

/// [`label_unit`] inside the caller's transaction.
#[allow(clippy::too_many_arguments)]
pub fn label_unit_in(
    tx: &Transaction,
    session_id: &str,
    unit_no: Option<i64>,
    label: Label,
    source: LabelSource,
    why: Option<&str>,
    th: &Thresholds,
    actor: &Actor,
) -> Result<i64> {
    {
        let session = session::get(tx, session_id)?
            .ok_or_else(|| err(format!("unknown session {session_id}")))?;
        let finished: Vec<session::Unit> = session::units(tx, session_id)?
            .into_iter()
            .filter(|u| u.stopped_at.is_some())
            .collect();
        let unit = match unit_no {
            Some(n) => finished
                .iter()
                .find(|u| u.unit_no == n)
                .ok_or_else(|| err(format!("no finished unit {n} in this session")))?,
            None => finished
                .iter()
                .rev()
                .find(|u| u.label.is_none())
                .or_else(|| finished.last())
                .ok_or_else(|| err("no finished unit to label yet"))?,
        };
        let data = UnitLabeled {
            unit_no: unit.unit_no,
            label,
            source,
            score: unit.drift_score,
            heuristic: unit.drift_score.and_then(|s| th.heuristic(s)),
            why: why.map(|w| w.trim().chars().take(MAX_WHY_CHARS).collect()),
        };
        Writer {
            tx,
            session: &session,
            actor,
        }
        .append("unit_labeled", &data, unit.task_id.as_deref())?;
        Ok(unit.unit_no)
    }
}
