//! The navigator (spec §7.5): picks at most one short note per unit of work and sends it
//! through the cheapest channel that works.
//!
//! Callers build [`Candidate`]s for the situation they see, call [`select`] for the channel
//! they are, then record the chosen note with [`note_sent_draft`] and deliver its text.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::claim::task_title;
use crate::error::Result;
use crate::session::{self, NeedsOwner, NeedsOwnerKind, Session, SessionState, Unit};
use crate::store::EventDraft;
use crate::tangent::{
    DriftFlagged, Label, LabelSource, NavEval, NavState, NavStateChanged, NoteSent, Thresholds,
    UnitLabeled, drift_pause, evaluate, get_detour,
};

/// Kinds of note, highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// Reserved for sub-project 4.
    Checkpoint,
    ReturnToTask,
    /// Reserved for sub-project 4.
    ReadBack,
    LabelRequest,
    /// The owner's answer to a drift question: the response to the red signal, so it
    /// outranks the tolerance and red notes it answers.
    OwnerReply,
    ToleranceWarning,
    DriftRed,
    /// A task this one depends on reached L2 (spec §9.1).
    DependencyReady,
    /// A task claimed done in an earlier unit is still unverified (spec §8.5).
    VerifyNudge,
}

impl NoteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoint",
            Self::ReturnToTask => "return_to_task",
            Self::ReadBack => "read_back",
            Self::LabelRequest => "label_request",
            Self::ToleranceWarning => "tolerance_warning",
            Self::DriftRed => "drift_red",
            Self::OwnerReply => "owner_reply",
            Self::DependencyReady => "dependency_ready",
            Self::VerifyNudge => "verify_nudge",
        }
    }
}

/// How a note reaches the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// The result of one of our MCP tools (immediate).
    ToolResult,
    /// `PostToolUse` additional context (mid-turn, free).
    PostToolUse,
    /// `Stop` additional context: forces another turn, so used sparingly.
    Stop,
    /// `UserPromptSubmit` additional context.
    PromptSubmit,
    /// `PreToolUse` additional context (the read-back nudge).
    PreToolUse,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolResult => "tool_result",
            Self::PostToolUse => "post_tool_use",
            Self::Stop => "stop",
            Self::PromptSubmit => "prompt_submit",
            Self::PreToolUse => "pre_tool_use",
        }
    }
}

/// A note that could be sent now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub kind: NoteKind,
    pub channel: Channel,
    /// 1–3 lines, ending with the one action expected.
    pub text: String,
    /// Identifies the situation (e.g. `tol:amber:D7`); a different key means the state
    /// changed and the note may repeat at once.
    pub state_key: String,
}

impl Candidate {
    pub fn new(
        kind: NoteKind,
        channel: Channel,
        text: impl Into<String>,
        state_key: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            channel,
            text: text.into(),
            state_key: state_key.into(),
        }
    }
}

/// The same kind of note about the same state isn't repeated for this many units.
pub const REPEAT_WINDOW_UNITS: i64 = 3;

/// Has any note gone out in this unit?
pub fn note_sent_in_unit(conn: &Connection, session_id: &str, unit_no: i64) -> Result<bool> {
    let n: i64 = conn
        .prepare_cached("SELECT count(*) FROM notes_sent WHERE session_id = ?1 AND unit_no = ?2")?
        .query_row(params![session_id, unit_no], |r| r.get(0))?;
    Ok(n > 0)
}

/// The note to send through `channel` in unit `unit_no`, if any:
/// - only candidates for this channel;
/// - nothing if any note already went out in this unit (one note per unit);
/// - not a kind + state already sent within the last [`REPEAT_WINDOW_UNITS`] units;
/// - the highest-priority one left.
pub fn select(
    conn: &Connection,
    session_id: &str,
    unit_no: i64,
    channel: Channel,
    candidates: Vec<Candidate>,
) -> Result<Option<Candidate>> {
    let mut left: Vec<Candidate> = candidates
        .into_iter()
        .filter(|c| c.channel == channel)
        .collect();
    if left.is_empty() || note_sent_in_unit(conn, session_id, unit_no)? {
        return Ok(None);
    }
    let mut recent = conn.prepare_cached(
        "SELECT count(*) FROM notes_sent
         WHERE session_id = ?1 AND kind = ?2 AND state_key = ?3 AND unit_no > ?4",
    )?;
    let mut kept = Vec::with_capacity(left.len());
    for c in left.drain(..) {
        let n: i64 = recent.query_row(
            params![
                session_id,
                c.kind.as_str(),
                c.state_key,
                unit_no - REPEAT_WINDOW_UNITS - 1
            ],
            |r| r.get(0),
        )?;
        if n == 0 {
            kept.push(c);
        }
    }
    Ok(kept.into_iter().min_by_key(|c| c.kind))
}

/// The `note_sent` event recording that `note` went out in unit `unit_no`.
pub fn note_sent_draft(
    repo_id: &str,
    session_id: &str,
    unit_no: i64,
    note: &Candidate,
) -> Result<EventDraft> {
    let data = NoteSent {
        unit_no,
        kind: note.kind.as_str().to_string(),
        channel: note.channel.as_str().to_string(),
        state_key: note.state_key.clone(),
    };
    Ok(EventDraft::new(
        repo_id,
        session_id,
        "note_sent",
        serde_json::to_value(data)?,
    ))
}

/// Asking the agent to label a unit it finished.
pub const LABEL_REQUEST: &str = "bridge-screen: label your last unit — on-task, detour, drift, or requested (the owner asked for this in this session) — with label_unit (one call).";

/// The same, when the heuristic flagged the unit as possible drift.
pub const DRIFT_REQUEST: &str = "bridge-screen: possible drift in your last unit. Label it — on-task, detour, drift, or requested (the owner asked for this in this session) — with label_unit (one call).";

/// Asking about a unit a new prompt cut short, at the next Stop.
pub fn earlier_request(unit_no: i64) -> String {
    format!(
        "bridge-screen: unit {unit_no} was cut short by a new message and still needs a label — on-task, detour, drift, or requested (the owner asked for this in this session). Label it with label_unit (one call)."
    )
}

/// What a hook should record and say after its own events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    pub events: Vec<EventDraft>,
    pub note: Option<String>,
}

impl Step {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.note.is_none()
    }
}

fn draft(session: &Session, kind: &str, data: &impl Serialize) -> Result<EventDraft> {
    Ok(EventDraft::new(
        &session.repo_id,
        &session.session_id,
        kind,
        serde_json::to_value(data)?,
    ))
}

fn labeled(
    session: &Session,
    unit: &Unit,
    label: Label,
    source: LabelSource,
    score: Option<f64>,
    heuristic: Option<Label>,
) -> Result<EventDraft> {
    let data = UnitLabeled {
        unit_no: unit.unit_no,
        label,
        source,
        score,
        heuristic,
        why: None,
    };
    let d = draft(session, "unit_labeled", &data)?;
    Ok(match &unit.task_id {
        Some(t) => d.task(t),
        None => d,
    })
}

/// The declared label of a unit inside a detour: `requested` when the owner asked for the
/// detour, else `detour`.
fn declared_label(conn: &Connection, detour_id: &str) -> Result<Label> {
    Ok(
        if get_detour(conn, detour_id)?.is_some_and(|d| d.requested) {
            Label::Requested
        } else {
            Label::Detour
        },
    )
}

/// After `unit_stopped` (spec §7.2): label the unit, or ask the agent to.
///
/// - a detour the owner asked for was open during the unit → `requested`, declared;
/// - another detour was open during the unit → `detour`, declared;
/// - score below the lower cut-off (or no plan to score against) → `on_task`, heuristic;
/// - otherwise ask with a [`NoteKind::LabelRequest`] through `Stop` (above the upper
///   cut-off it is first `drift_flagged`). The request is the only Stop continuation:
///   never when `stop_hook_active`, and at most one note per unit. When it can't be sent,
///   the heuristic labels the unit now (`drift` when flagged, else `on_task`).
///
/// When this unit needs no question, the unit before it may: one cut short by a new
/// prompt whose score fell between the cut-offs (see [`label_left_unlabeled`]). It is
/// asked about the same way, under the same limits.
pub fn label_at_stop(
    conn: &Connection,
    session: &Session,
    unit: &Unit,
    stop_hook_active: bool,
    th: &Thresholds,
) -> Result<Step> {
    let mut step = label_stopped(conn, session, unit, stop_hook_active, th)?;
    if step.note.is_none() {
        ask_about_interrupted(conn, session, unit.unit_no, stop_hook_active, th, &mut step)?;
    }
    Ok(step)
}

/// The unit before `unit_no`, when it was cut short by a new prompt and is still waiting
/// for its question: unlabelled, outside a detour, scored between the cut-offs.
fn waiting_interrupted(
    conn: &Connection,
    session: &Session,
    unit_no: i64,
    th: &Thresholds,
) -> Result<Option<Unit>> {
    Ok(
        crate::session::unit(conn, &session.session_id, unit_no - 1)?.filter(|u| {
            u.label.is_none()
                && u.stopped_at.is_some()
                && u.detour_id.is_none()
                && u.drift_score.is_some_and(|s| th.heuristic(s).is_none())
        }),
    )
}

/// At the Stop of unit `unit_no`: ask about the interrupted unit before it, or, when the
/// question can't go out, label that unit by the heuristic now.
fn ask_about_interrupted(
    conn: &Connection,
    session: &Session,
    unit_no: i64,
    stop_hook_active: bool,
    th: &Thresholds,
    step: &mut Step,
) -> Result<()> {
    let Some(earlier) = waiting_interrupted(conn, session, unit_no, th)? else {
        return Ok(());
    };
    let ask = Candidate::new(
        NoteKind::LabelRequest,
        Channel::Stop,
        earlier_request(earlier.unit_no),
        format!("label:{}", earlier.unit_no),
    );
    let chosen = if stop_hook_active {
        None
    } else {
        select(conn, &session.session_id, unit_no, Channel::Stop, vec![ask])?
    };
    if let Some(note) = chosen {
        step.events.push(note_sent_draft(
            &session.repo_id,
            &session.session_id,
            unit_no,
            &note,
        )?);
        step.note = Some(note.text);
    } else {
        step.events.push(labeled(
            session,
            &earlier,
            Label::OnTask,
            LabelSource::Heuristic,
            earlier.drift_score,
            None,
        )?);
    }
    Ok(())
}

/// [`label_at_stop`] for the stopped unit itself.
fn label_stopped(
    conn: &Connection,
    session: &Session,
    unit: &Unit,
    stop_hook_active: bool,
    th: &Thresholds,
) -> Result<Step> {
    let mut step = Step::default();
    if unit.label.is_some() {
        return Ok(step);
    }
    let score = unit.drift_score;
    if let Some(detour_id) = &unit.detour_id {
        let heuristic = score.and_then(|s| th.heuristic(s));
        step.events.push(labeled(
            session,
            unit,
            declared_label(conn, detour_id)?,
            LabelSource::Declared,
            score,
            heuristic,
        )?);
        return Ok(step);
    }
    let Some(s) = score.filter(|s| *s >= th.lower) else {
        step.events.push(labeled(
            session,
            unit,
            Label::OnTask,
            LabelSource::Heuristic,
            score,
            score.map(|_| Label::OnTask),
        )?);
        return Ok(step);
    };
    let flagged = s > th.upper;
    if flagged {
        let data = DriftFlagged {
            unit_no: unit.unit_no,
            score: s,
        };
        step.events.push(draft(session, "drift_flagged", &data)?);
    }
    // Each unit is its own question: the repeat window must never stop a unit from being
    // asked, or a flagged unit would fall back to drift unasked. One note per unit and
    // `stop_hook_active` still bound the continuations.
    let text = if flagged {
        DRIFT_REQUEST
    } else {
        LABEL_REQUEST
    };
    let ask = Candidate::new(
        NoteKind::LabelRequest,
        Channel::Stop,
        text,
        format!("label:{}", unit.unit_no),
    );
    let chosen = if stop_hook_active {
        None
    } else {
        select(
            conn,
            &session.session_id,
            unit.unit_no,
            Channel::Stop,
            vec![ask],
        )?
    };
    if let Some(note) = chosen {
        step.events.push(note_sent_draft(
            &session.repo_id,
            &session.session_id,
            unit.unit_no,
            &note,
        )?);
        step.note = Some(note.text);
    } else {
        let label = if flagged { Label::Drift } else { Label::OnTask };
        step.events.push(labeled(
            session,
            unit,
            label,
            LabelSource::Heuristic,
            score,
            th.heuristic(s),
        )?);
    }
    Ok(step)
}

/// When a new prompt arrives (spec §7.2):
/// - the unit it just closed because the unit missed its Stop (`interrupted`) is labelled
///   like a Stop unit, from its files alone: `requested` or `detour` if a detour was open,
///   `on_task` below the lower cut-off (or unscored), `drift` above the upper one (first
///   `drift_flagged`); between the cut-offs it waits for the next Stop's question;
/// - any other finished unit among the last two still waiting for its label is labelled by
///   the heuristic (`requested` or `detour` if a detour was open, `drift` if flagged, else
///   `on_task`).
pub fn label_left_unlabeled(
    conn: &Connection,
    session: &Session,
    interrupted: Option<i64>,
    th: &Thresholds,
) -> Result<Vec<EventDraft>> {
    let waiting: Vec<i64> = conn
        .prepare_cached(
            "SELECT unit_no FROM units
             WHERE session_id = ?1 AND stopped_at IS NOT NULL AND label IS NULL
               AND unit_no >= (SELECT max(unit_no) FROM units
                               WHERE session_id = ?1 AND stopped_at IS NOT NULL) - 1
             ORDER BY unit_no",
        )?
        .query_map([&session.session_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for unit_no in waiting {
        let Some(unit) = crate::session::unit(conn, &session.session_id, unit_no)? else {
            continue;
        };
        let score = unit.drift_score;
        let heuristic = score.and_then(|s| th.heuristic(s));
        let (label, source) = if let Some(detour_id) = &unit.detour_id {
            (declared_label(conn, detour_id)?, LabelSource::Declared)
        } else if interrupted == Some(unit_no) {
            match score {
                Some(s) if s > th.upper => {
                    let data = DriftFlagged { unit_no, score: s };
                    out.push(draft(session, "drift_flagged", &data)?);
                    (Label::Drift, LabelSource::Heuristic)
                }
                Some(s) if s >= th.lower => continue,
                _ => (Label::OnTask, LabelSource::Heuristic),
            }
        } else if unit.flagged {
            (Label::Drift, LabelSource::Heuristic)
        } else {
            (Label::OnTask, LabelSource::Heuristic)
        };
        out.push(labeled(session, &unit, label, source, score, heuristic)?);
    }
    Ok(out)
}

/// `1.2 Login form`, or `your task` without one.
fn task_display(conn: &Connection, session: &Session) -> Result<String> {
    Ok(match &session.active_task {
        Some(id) => {
            let title = task_title(conn, &session.repo_id, id)?.unwrap_or_default();
            format!("{id} {title}").trim_end().to_string()
        }
        None => "your task".to_string(),
    })
}

/// The amber note (spec §7.3).
fn tolerance_text(reason: &str, e: &NavEval, task: &str) -> String {
    let minutes = e.away_secs / 60;
    let ratio = match e.estimate_min.filter(|m| *m > 0) {
        #[allow(clippy::cast_precision_loss)] // seconds of away time
        Some(est) => format!(
            " (~{:.1}× the task estimate)",
            e.away_secs as f64 / (f64::from(est) * 60.0)
        ),
        None => String::new(),
    };
    format!(
        "bridge-screen: you've been on \"{reason}\" for {minutes} min{ratio}. Continue if it still blocks {task}; otherwise close the detour and return."
    )
}

/// The red note; with `drift_pause` the agent stops after its current step.
fn red_text(on_detour: bool, pause: bool, task: &str) -> String {
    let what = if on_detour {
        "this detour"
    } else {
        "this tangent"
    };
    let head = format!("bridge-screen: {what} is well past tolerance; the owner has been flagged.");
    if pause {
        format!(
            "{head} Finish your current step, commit, then stop and ask the owner before continuing."
        )
    } else {
        format!("{head} Keep going if it's essential, or close it and return to {task}.")
    }
}

/// The owner's answer to a drift question, as a note.
fn reply_text(answer: &str, reason: Option<&str>, task: &str) -> String {
    let says = match (answer, reason) {
        ("continue", Some(why)) => format!("continue: {}", why.trim_end_matches('.')),
        ("continue", None) => "continue".to_string(),
        ("make_task", _) => "make this a task".to_string(),
        (_, _) => format!("stop the detour and return to {task}"),
    };
    format!("bridge-screen: the owner says {says}.")
}

/// The reason the owner gave with the `owner_action` event `id`, if any.
fn owner_reason(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached(
            "SELECT json_extract(data, '$.reason') FROM events
             WHERE id = ?1 AND kind = 'owner_action'",
        )?
        .query_row([id], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

/// Track the tolerance band and pick a note for `channel` (spec §7.3, §7.5):
/// - when the computed state differs from the stored one, `nav_state_changed` (and, on
///   red, `needs_owner {kind: drift}` so the owner sees it);
/// - through `PostToolUse`: a tolerance warning on amber, the red note on red;
/// - through `PostToolUse` or `UserPromptSubmit`: a pending owner answer.
pub fn nav_step(
    conn: &Connection,
    session_id: &str,
    channel: Channel,
    th: &Thresholds,
    now: jiff::Timestamp,
) -> Result<Step> {
    let mut step = Step::default();
    let Some(session) = session::get(conn, session_id)? else {
        return Ok(step);
    };
    if session.state == SessionState::Ended {
        return Ok(step);
    }
    let e = evaluate(conn, &session, th, now)?;
    if e.state != session.nav_state {
        let data = NavStateChanged {
            from: session.nav_state,
            to: e.state,
            away_secs: e.away_secs,
        };
        step.events
            .push(draft(&session, "nav_state_changed", &data)?);
        if e.state == NavState::Red {
            let data = NeedsOwner {
                kind: NeedsOwnerKind::Drift,
                task_id: None,
            };
            step.events.push(draft(&session, "needs_owner", &data)?);
        }
    }
    let notes_here = matches!(channel, Channel::PostToolUse | Channel::PromptSubmit);
    if !notes_here {
        return Ok(step);
    }
    let detour = match &session.current_detour {
        Some(id) => get_detour(conn, id)?,
        None => None,
    };
    let key_end = detour.as_ref().map_or("none", |d| d.detour_id.as_str());
    let task = task_display(conn, &session)?;
    let mut candidates = Vec::new();
    if channel == Channel::PostToolUse {
        match e.state {
            NavState::Amber => {
                let reason = detour
                    .as_ref()
                    .map_or("work outside your task", |d| d.reason.as_str());
                candidates.push(Candidate::new(
                    NoteKind::ToleranceWarning,
                    channel,
                    tolerance_text(reason, &e, &task),
                    format!("tol:amber:{key_end}"),
                ));
            }
            NavState::Red => {
                let pause = drift_pause(conn, &session.repo_id)?;
                candidates.push(Candidate::new(
                    NoteKind::DriftRed,
                    channel,
                    red_text(detour.is_some(), pause, &task),
                    format!("tol:red:{key_end}"),
                ));
            }
            NavState::Green => {}
        }
    }
    if let Some(pending) = &session.pending_reply {
        let (answer, id) = pending.split_once(':').unwrap_or((pending.as_str(), ""));
        let reason = owner_reason(conn, id)?;
        candidates.push(Candidate::new(
            NoteKind::OwnerReply,
            channel,
            reply_text(answer, reason.as_deref(), &task),
            format!("reply:{id}"),
        ));
    }
    let unit = session::last_unit(conn, session_id)?;
    if channel == Channel::PromptSubmit {
        candidates.extend(dependency_ready(conn, &session)?);
        if let Some(u) = &unit {
            candidates.extend(verify_nudges(conn, &session, u)?);
        }
    }
    if candidates.is_empty() {
        return Ok(step);
    }
    let unit_no = unit.map_or(0, |u| u.unit_no);
    deliver(conn, &session, unit_no, channel, candidates, &mut step)?;
    Ok(step)
}

/// Pick the note among `candidates` and record it (a verification nudge also counts as
/// one, spec §8.5).
fn deliver(
    conn: &Connection,
    session: &Session,
    unit_no: i64,
    channel: Channel,
    candidates: Vec<Candidate>,
    step: &mut Step,
) -> Result<()> {
    let Some(note) = select(conn, &session.session_id, unit_no, channel, candidates)? else {
        return Ok(());
    };
    step.events.push(note_sent_draft(
        &session.repo_id,
        &session.session_id,
        unit_no,
        &note,
    )?);
    if note.kind == NoteKind::VerifyNudge
        && let Some((task, level)) = note
            .state_key
            .strip_prefix("verify:")
            .and_then(|k| k.rsplit_once(':'))
    {
        step.events.extend(crate::verify::nudge_drafts(
            conn,
            &session.repo_id,
            &session.session_id,
            task,
            level.parse().unwrap_or(0),
        )?);
    }
    step.note = Some(note.text);
    Ok(())
}

/// Tasks this session claimed done in an earlier unit that are still claimed or at L1
/// (spec §8.5): a nudge to verify each (one goes out; a task inside its repeat window
/// gives way to the next).
fn verify_nudges(conn: &Connection, session: &Session, unit: &Unit) -> Result<Vec<Candidate>> {
    let Ok(started) = unit.started_at.parse::<jiff::Timestamp>() else {
        return Ok(Vec::new());
    };
    let pending = crate::verify::pending_claims(conn, &session.session_id, started)?;
    Ok(pending
        .into_iter()
        .map(|(task, level)| {
            Candidate::new(
                NoteKind::VerifyNudge,
                Channel::PromptSubmit,
                format!(
                    "bridge-screen: {task} is claimed done but not verified (L{level}). Run the tests and call verify({task}, level 2)."
                ),
                format!("verify:{task}:{level}"),
            )
        })
        .collect())
}

/// Dependencies of the active task that reached L2 or above, verified by another session
/// (spec §9.1): once per dependency and commit per session. Links into other repos say
/// which repo; waits that cleared because their target was done get the same note.
fn dependency_ready(conn: &Connection, session: &Session) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    let Some(task) = &session.active_task else {
        return wait_notes(conn, session, out);
    };
    let Some(node) = crate::plan::get_node(conn, &session.repo_id, task)? else {
        return wait_notes(conn, session, out);
    };
    let local = node
        .depends_on
        .into_iter()
        .map(|d| (session.repo_id.clone(), None, d));
    let external = node
        .external_deps
        .into_iter()
        .map(|d| (d.repo_id, Some(d.repo_name), d.id));
    for (repo, repo_name, dep) in local.chain(external) {
        let Some(v) = crate::verify::top_verification(conn, &repo, &dep)?.filter(|v| v.level >= 2)
        else {
            continue;
        };
        if v.session_id == session.session_id {
            continue;
        }
        let key = link_key(&session.repo_id, &repo, &dep, &v.sha);
        if note_was_sent(conn, &session.session_id, &key)?
            || crate::waits::noted_by_wait(conn, session, &repo, &dep, &v.sha)?
        {
            continue;
        }
        let title = task_title(conn, &repo, &dep)?.unwrap_or_default();
        let shown = match &repo_name {
            None => dep.clone(),
            Some(name) => format!("{name}:{dep}"),
        };
        out.push(Candidate::new(
            NoteKind::DependencyReady,
            Channel::PromptSubmit,
            done_text(conn, &shown, &title, &v.session_id, &v.sha)?,
            key,
        ));
    }
    wait_notes(conn, session, out)
}

/// The state key of a plan link's "done" note: `dep:<id>:<sha>` in this repo,
/// `dep:<repo id>:<id>:<sha>` for another.
pub(crate) fn link_key(here: &str, repo: &str, id: &str, sha: &str) -> String {
    if repo == here {
        format!("dep:{id}:{sha}")
    } else {
        format!("dep:{repo}:{id}:{sha}")
    }
}

/// `links` plus the notes of waits that cleared because their target was done, leaving out
/// one the plan link's note already says (same target, same commit).
fn wait_notes(
    conn: &Connection,
    session: &Session,
    mut links: Vec<Candidate>,
) -> Result<Vec<Candidate>> {
    for c in crate::waits::cleared_notes(conn, session)? {
        if !links.iter().any(|l| l.text == c.text) {
            links.push(c);
        }
    }
    Ok(links)
}

/// Was a note with this state key sent to the session already?
pub(crate) fn note_was_sent(conn: &Connection, session_id: &str, key: &str) -> Result<bool> {
    let sent: i64 = conn
        .prepare_cached("SELECT count(*) FROM notes_sent WHERE session_id = ?1 AND state_key = ?2")?
        .query_row(params![session_id, key], |r| r.get(0))?;
    Ok(sent > 0)
}

/// "bridge-screen: api:2.4 Login endpoint is done on `feat/auth` @abc1234. Merge or rebase
/// to pick it up." — `by` is the session that verified it, for its branch.
pub(crate) fn done_text(
    conn: &Connection,
    shown: &str,
    title: &str,
    by: &str,
    sha: &str,
) -> Result<String> {
    let branch = session::get(conn, by)?.and_then(|s| s.branch);
    let short: String = sha.chars().take(7).collect();
    let on = branch.map_or_else(String::new, |b| format!(" on `{b}`"));
    Ok(format!(
        "bridge-screen: {shown} {title} is done{on} @{short}. Merge or rebase to pick it up."
    )
    .replace("  ", " "))
}

#[cfg(test)]
mod tests {
    use super::{Candidate, Channel, NoteKind, note_sent_draft, select};
    use crate::store::{EventDraft, Store};
    use serde_json::json;

    fn store() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.append(EventDraft::new(
            "r",
            "s",
            "session_started",
            json!({"cwd": "/r", "source": "startup"}),
        ))
        .unwrap();
        s
    }

    fn c(kind: NoteKind, channel: Channel, key: &str) -> Candidate {
        Candidate::new(kind, channel, format!("{kind:?}"), key)
    }

    fn send(s: &mut Store, unit_no: i64, note: &Candidate) {
        s.append(note_sent_draft("r", "s", unit_no, note).unwrap())
            .unwrap();
    }

    fn pick(s: &Store, unit_no: i64, channel: Channel, cands: &[Candidate]) -> Option<NoteKind> {
        select(s.conn(), "s", unit_no, channel, cands.to_vec())
            .unwrap()
            .map(|c| c.kind)
    }

    #[test]
    fn priority_order() {
        let s = store();
        let p = Channel::PostToolUse;
        let all = [
            c(NoteKind::OwnerReply, p, "a"),
            c(NoteKind::DriftRed, p, "b"),
            c(NoteKind::ToleranceWarning, p, "c"),
            c(NoteKind::LabelRequest, p, "d"),
            c(NoteKind::ReturnToTask, p, "e"),
        ];
        assert_eq!(pick(&s, 1, p, &all), Some(NoteKind::ReturnToTask));
        assert_eq!(pick(&s, 1, p, &all[..3]), Some(NoteKind::OwnerReply));
        assert_eq!(pick(&s, 1, p, &all[1..3]), Some(NoteKind::ToleranceWarning));
        assert_eq!(pick(&s, 1, p, &all[1..2]), Some(NoteKind::DriftRed));
        assert!(NoteKind::LabelRequest < NoteKind::OwnerReply);
        assert!(NoteKind::OwnerReply < NoteKind::ToleranceWarning);
        assert!(NoteKind::Checkpoint < NoteKind::ReturnToTask);
        assert!(NoteKind::ReturnToTask < NoteKind::ReadBack);
        assert!(NoteKind::ReadBack < NoteKind::LabelRequest);
        assert!(NoteKind::DriftRed < NoteKind::DependencyReady);
        assert!(NoteKind::DependencyReady < NoteKind::VerifyNudge);
    }

    #[test]
    fn one_note_per_unit() {
        let mut s = store();
        let p = Channel::PostToolUse;
        let warn = c(NoteKind::ToleranceWarning, p, "tol:amber:none");
        send(&mut s, 2, &c(NoteKind::OwnerReply, p, "reply:1"));
        assert_eq!(pick(&s, 2, p, std::slice::from_ref(&warn)), None);
        assert_eq!(pick(&s, 3, p, &[warn]), Some(NoteKind::ToleranceWarning));
    }

    #[test]
    fn same_kind_and_state_waits_three_units() {
        let mut s = store();
        let p = Channel::PostToolUse;
        let warn = c(NoteKind::ToleranceWarning, p, "tol:amber:D1");
        send(&mut s, 1, &warn);
        for unit in 2..=4 {
            assert_eq!(
                pick(&s, unit, p, std::slice::from_ref(&warn)),
                None,
                "{unit}"
            );
        }
        assert_eq!(
            pick(&s, 5, p, std::slice::from_ref(&warn)),
            Some(NoteKind::ToleranceWarning)
        );
        // A suppressed note doesn't hide a lower-priority one.
        let red = c(NoteKind::DriftRed, p, "tol:red:D1");
        assert_eq!(pick(&s, 2, p, &[warn, red]), Some(NoteKind::DriftRed));
    }

    #[test]
    fn changed_state_is_allowed_at_once() {
        let mut s = store();
        let p = Channel::PostToolUse;
        send(&mut s, 1, &c(NoteKind::ToleranceWarning, p, "tol:amber:D1"));
        assert_eq!(
            pick(
                &s,
                2,
                p,
                &[c(NoteKind::ToleranceWarning, p, "tol:amber:D2")]
            ),
            Some(NoteKind::ToleranceWarning)
        );
        assert_eq!(
            pick(&s, 2, p, &[c(NoteKind::DriftRed, p, "tol:red:D1")]),
            Some(NoteKind::DriftRed)
        );
    }

    #[test]
    fn only_the_calling_channel() {
        let s = store();
        let cands = [
            c(NoteKind::LabelRequest, Channel::Stop, "label"),
            c(NoteKind::OwnerReply, Channel::PromptSubmit, "reply:1"),
        ];
        assert_eq!(pick(&s, 1, Channel::PostToolUse, &cands), None);
        assert_eq!(
            pick(&s, 1, Channel::PromptSubmit, &cands),
            Some(NoteKind::OwnerReply)
        );
        assert_eq!(
            pick(&s, 1, Channel::Stop, &cands),
            Some(NoteKind::LabelRequest)
        );
    }
}
