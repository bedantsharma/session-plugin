//! Read models for the screens (spec §10): the **glance** — projects, the needs-you queue
//! and live sessions — and the drill-downs ([`drill`]). `status`, the dashboard API,
//! `watch` and the desktop card all render these, so every screen shows the same facts;
//! ordering, escalation, state words and lights are decided here, never in a screen.
//!
//! Read-only: nothing here writes. `now` is a parameter (live ages, away time); it never
//! reaches a view table.

pub mod drill;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::claim::task_title;
use crate::config::Config;
use crate::error::Result;
use crate::forecast::{self, Forecast};
use crate::handoff::{self, BriefingStatus, LeftoverAction, LeftoverState, ReadbackState};
use crate::owner::{DriftAnswer, OwnerAction};
use crate::plan::{Hill, NodeKind, NodeStatus, PlanNode, get_node, load_tree};
use crate::session::{self, Session, SessionState};
use crate::store::{OWNER_SESSION, repo_name};
use crate::tangent::{self, Label, NavState};
use crate::telemetry;
use crate::verify;

/// More needs-you items than this escalate (spec §10.1).
pub const ESCALATE_ABOVE: usize = 4;

/// The glance screen: everything at a glance (spec §10.1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Glance {
    /// When the model was built (RFC 3339), for "as of HH:MM".
    pub as_of: String,
    /// The newest event's position: the screens' polling cursor.
    pub last_event_pos: i64,
    pub live_sessions: usize,
    pub projects: Vec<ProjectRow>,
    /// Items with a defined owner response, most urgent first.
    pub needs_you: Vec<NeedsYou>,
    /// The number every screen shows (pill, tab title, favicon, `status` first line).
    pub needs_you_count: usize,
    /// More than [`ESCALATE_ABOVE`] items.
    pub escalated: bool,
}

impl Glance {
    /// The glance narrowed to one repo (the count and escalation follow).
    #[must_use]
    pub fn for_repo(mut self, repo_id: &str) -> Self {
        self.projects.retain(|p| p.repo_id == repo_id);
        self.needs_you.retain(|i| i.repo_id == repo_id);
        self.live_sessions = self.projects.iter().map(|p| p.sessions.len()).sum();
        self.needs_you_count = self.needs_you.len();
        self.escalated = self.needs_you_count > ESCALATE_ABOVE;
        self
    }
}

/// A plan node by id and title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeRef {
    pub id: String,
    pub title: String,
}

/// One project (repo) on the glance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectRow {
    pub repo_id: String,
    pub name: String,
    pub goal: Option<NodeRef>,
    /// The goal's status from every task (derived, never stored); `None` without a goal.
    pub goal_status: Option<crate::plan::Rollup>,
    pub done_pct: u32,
    pub claimed_pct: u32,
    pub unestimated: usize,
    pub milestones: Vec<MilestoneRow>,
    pub leftovers: LeftoverCounts,
    /// `tuned drift .35–.65, tolerance 25%` when the repo's thresholds were auto-tuned.
    pub tuned: Option<String>,
    /// When the plan will likely be done (spec §9.3, §10.1); `None` without a plan.
    pub forecast: Option<ForecastRow>,
    /// Tokens and money spent on the repo so far; `None` before any usage.
    pub cost: Option<CostRow>,
    /// Its needs-you items.
    pub needs_you: usize,
    /// Live sessions, the ones needing the owner first.
    pub sessions: Vec<SessionRow>,
}

/// A project's forecast with its words (spec §9.3, §10.1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ForecastRow {
    #[serde(flatten)]
    pub forecast: Forecast,
    /// `likely by Oct 30 · could take until Nov 8`, or the reason there are no dates.
    pub text: String,
    /// `Oct 30–Nov 8` for one-line screens; `None` without dates.
    pub compact: Option<String>,
    /// `likely Oct 30–Nov 8`, or a few words on why there are no dates.
    pub short: String,
    /// Tooltip and screen-reader text: what the two dates mean.
    pub help: String,
    /// `based on 12 finished tasks; tasks typically take 1.8× their estimate`.
    pub basis: String,
}

/// Tokens and what they cost, computed at read time from the configured prices.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostRow {
    pub tokens: u64,
    /// `3.1M`.
    pub tokens_text: String,
    /// USD of the priced models (`None`: nothing priced — unknown, never $0).
    pub usd: Option<f64>,
    /// Models with tokens and no price: `usd` is then a lower bound.
    pub unpriced_models: Vec<String>,
    /// `$12.40`, `≥ $12.40 (some models unpriced)` or `cost unknown`.
    pub text: String,
    /// `$12.40 spent · 3.1M tokens`.
    pub summary: String,
}

/// The cost of `scope`, or `None` when nothing was spent there.
pub fn cost_row(
    conn: &Connection,
    scope: &telemetry::Scope,
    cfg: &Config,
) -> Result<Option<CostRow>> {
    let tokens = telemetry::totals(conn, scope)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    Ok(Some(cost_of(&tokens, cfg)))
}

/// [`CostRow`] for some tokens.
pub fn cost_of(tokens: &telemetry::Tokens, cfg: &Config) -> CostRow {
    let cost = telemetry::cost(tokens, &cfg.prices);
    let text = telemetry::cost_text(&cost);
    let tokens_text = telemetry::tokens_text(tokens.sum.total());
    let summary = match cost.usd {
        Some(usd) if cost.complete() => {
            format!("{} spent · {tokens_text} tokens", telemetry::usd_text(usd))
        }
        Some(usd) => format!(
            "≥ {} spent (some models unpriced) · {tokens_text} tokens",
            telemetry::usd_text(usd)
        ),
        None => format!("cost unknown · {tokens_text} tokens"),
    };
    CostRow {
        tokens: tokens.sum.total(),
        tokens_text,
        usd: cost.usd,
        unpriced_models: cost.unpriced_models,
        text,
        summary,
    }
}

/// The forecast row for a repo at `now` (dates in the owner's time zone).
pub fn forecast_row(conn: &Connection, repo_id: &str, now: Timestamp) -> Result<ForecastRow> {
    forecast_row_in(conn, repo_id, now, &jiff::tz::TimeZone::system())
}

/// [`forecast_row`] in a given time zone.
pub fn forecast_row_in(
    conn: &Connection,
    repo_id: &str,
    now: Timestamp,
    tz: &jiff::tz::TimeZone,
) -> Result<ForecastRow> {
    let f = forecast::run(conn, repo_id, now, tz)?;
    let tasks = if f.points == 1 { "task" } else { "tasks" };
    Ok(ForecastRow {
        text: forecast::range_text(&f, now, tz),
        compact: forecast::compact_range(&f, now, tz),
        short: forecast::short_text(&f, now, tz),
        help: format!(
            "50% and 90% chance, based on how long {} verified {tasks} actually took",
            f.points
        ),
        basis: forecast::basis_text(&f),
        forecast: f,
    })
}

/// A milestone's progress and hill state (spec §5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MilestoneRow {
    pub id: String,
    pub title: String,
    /// From its tasks (derived, never stored): `done` once every task passed its tests.
    pub status: crate::plan::Rollup,
    pub hill: Option<Hill>,
    pub done_pct: u32,
    pub claimed_pct: u32,
    /// Hill state and done% unchanged across the last 3 sessions that worked under it.
    pub stuck: bool,
    /// How many sessions it has been stuck for (0 unless `stuck`).
    pub stuck_sessions: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct LeftoverCounts {
    pub open: usize,
    pub report_only: usize,
}

/// A session's status light (spec §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Light {
    Green,
    Amber,
    Red,
}

/// Why a session that isn't working is stopped (owner decision 2026-10-14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Rest {
    /// It asked a question or waits on a permission prompt: also a queue item.
    NeedsYou { why: NeedsKind },
    /// Its task was claimed done (or verified) in this session and nothing is pending.
    Done { task: NodeRef },
    /// Stopped for any other reason.
    Idle,
}

/// Something the agent owes, or information: shown on the session, never queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagKind {
    CheckpointMissed,
    HandoffUnstable,
    SquashFailed,
    CheckpointPending,
    ReadbackPending,
    VerifyPending,
}

impl FlagKind {
    /// How the flag reads on a session line.
    pub fn text(self) -> &'static str {
        match self {
            Self::CheckpointMissed => "checkpoint missed",
            Self::HandoffUnstable => "handoff unstable",
            Self::SquashFailed => "squash failed",
            Self::CheckpointPending => "checkpoint pending",
            Self::ReadbackPending => "read-back pending",
            Self::VerifyPending => "verify pending",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionFlag {
    pub kind: FlagKind,
    /// Seconds since the flag was raised, when known.
    pub age_s: Option<i64>,
    /// verify_pending: the tasks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<String>,
}

/// The detour a session is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetourRef {
    /// `D2`.
    pub label: String,
    pub reason: String,
}

/// One live session on the glance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionRow {
    pub session_id: String,
    /// The first 6 characters.
    pub short: String,
    pub repo_id: String,
    /// The terminal's state column: `working`, `idle`, `done`, `needs you` — the last only
    /// when the session raises a needs-you item (the light is red).
    pub state_label: String,
    pub light: Light,
    /// The word next to the light: `on task`, `detour`, `drift`, `idle`, `done`,
    /// `needs you`.
    pub light_word: String,
    pub task: Option<NodeRef>,
    pub detour: Option<DetourRef>,
    pub units: i64,
    pub active_min: i64,
    /// The navigator light in words: `on task`, `detour 25m · D1 "…"`, `drift ⚠ 40m`.
    pub nav_text: String,
    pub flags: Vec<SessionFlag>,
    /// Set when the session isn't working.
    pub rest: Option<Rest>,
    /// It ended its turn with a question to the owner, or is in the ask tool.
    pub asked_question: bool,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    /// Tokens and money the session spent so far; `None` before any usage.
    pub cost: Option<CostRow>,
    /// Sort key: 0 red, 1 other needs-you (question, permission wait), 2 handoff trouble,
    /// 3 working, 4 resting.
    pub rank: u8,
}

/// Kinds of needs-you item, in rank order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedsKind {
    /// Far outside tolerance, or drift with no reason: Continue / Make it a task / Stop.
    DriftRed,
    /// The agent asked the owner a question: answer in that session.
    Question,
    /// A permission prompt (or form) waits in that session.
    PermissionWait,
    /// Claimed work nudged twice without a check: ✓ Works / ✗ Broken.
    VerifyOverdue,
    /// A dead session's uncommitted files: Adopt / Resume / Undo.
    Leftover,
}

impl NeedsKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DriftRed => "drift_red",
            Self::Question => "question",
            Self::PermissionWait => "permission_wait",
            Self::VerifyOverdue => "verify_overdue",
            Self::Leftover => "leftover",
        }
    }

    /// Queue order: drift first, then sessions blocked on the owner, then verification,
    /// then recovery.
    pub fn rank(self) -> u8 {
        match self {
            Self::DriftRed => 0,
            Self::Question | Self::PermissionWait => 1,
            Self::VerifyOverdue => 2,
            Self::Leftover => 3,
        }
    }
}

/// A field a screen asks for before sending the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionInput {
    /// The action field it fills, e.g. `title`.
    pub name: String,
    pub label: String,
    pub required: bool,
}

/// How a button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Primary,
    Neutral,
    Danger,
}

/// One button: its label and the exact action it sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionSpec {
    pub label: String,
    pub action: OwnerAction,
    /// Fields to ask for first; empty for one-click actions.
    pub inputs: Vec<ActionInput>,
    pub tone: Tone,
}

/// Where to go to answer an item that has no buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RespondIn {
    pub session_short: String,
    pub worktree: Option<String>,
}

/// One needs-you item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NeedsYou {
    /// Stable between polls: kind plus target.
    pub id: String,
    pub kind: NeedsKind,
    pub repo_id: String,
    pub repo_name: String,
    pub session_id: Option<String>,
    pub task: Option<NodeRef>,
    pub title: String,
    pub detail: String,
    /// Leftover: the files.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    pub actions: Vec<ActionSpec>,
    pub respond_in: Option<RespondIn>,
    pub rank: u8,
    /// Seconds since the item appeared.
    pub age_s: i64,
}

fn short(id: &str) -> String {
    id.chars().take(6).collect()
}

fn parse_ts(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

fn age(now: Timestamp, ts: Option<&str>) -> Option<i64> {
    ts.and_then(parse_ts)
        .map(|t| now.duration_since(t).as_secs().max(0))
}

/// The newest event position (0 for an empty log).
pub fn last_event_pos(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(pos), 0) FROM events", [], |r| r.get(0))?)
}

/// When the session last recorded an event of `kind`.
fn latest_ts(conn: &Connection, session_id: &str, kind: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached(
            "SELECT ts FROM events WHERE session_id = ?1 AND kind = ?2 ORDER BY seq DESC LIMIT 1",
        )?
        .query_row(params![session_id, kind], |r| r.get(0))
        .optional()?)
}

/// The owner already answered the session's latest drift flag (its last move out of
/// green). An answered flag is no longer a needs-you item, and a second answer is stale.
pub fn drift_answered(conn: &Connection, session: &Session) -> Result<bool> {
    let flagged: i64 = conn
        .prepare_cached(
            "SELECT COALESCE(MAX(pos), 0) FROM events WHERE session_id = ?1
               AND kind = 'nav_state_changed' AND json_extract(data, '$.to') != 'green'",
        )?
        .query_row([&session.session_id], |r| r.get(0))?;
    let answered: Option<i64> = conn
        .prepare_cached(
            "SELECT pos FROM events WHERE repo_id = ?1 AND session_id = ?2
               AND kind = 'owner_action' AND pos > ?3
               AND json_extract(data, '$.action') = 'drift'
               AND json_extract(data, '$.target') = ?4
             LIMIT 1",
        )?
        .query_row(
            params![session.repo_id, OWNER_SESSION, flagged, session.session_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(answered.is_some())
}

/// The kind of the session's open wait on the owner (`permission_prompt`, `idle_prompt`,
/// `other`) and when it began.
fn open_wait(conn: &Connection, session_id: &str) -> Result<Option<(String, String)>> {
    Ok(conn
        .prepare_cached(
            "SELECT json_extract(data, '$.kind'), ts FROM events
             WHERE session_id = ?1 AND kind = 'needs_owner'
               AND json_extract(data, '$.kind') IN ('permission_prompt', 'idle_prompt', 'other')
             ORDER BY seq DESC LIMIT 1",
        )?
        .query_row([session_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?)
}

/// The question the session asked the owner, if one is open: (source, since).
fn open_question(conn: &Connection, session_id: &str) -> Result<Option<(String, String)>> {
    Ok(conn
        .prepare_cached(
            "SELECT question_source, question_since FROM sessions
             WHERE session_id = ?1 AND question_unit IS NOT NULL",
        )?
        .query_row([session_id], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })
        .optional()?)
}

fn node_ref(conn: &Connection, repo_id: &str, id: &str) -> Result<NodeRef> {
    Ok(NodeRef {
        id: id.to_string(),
        title: task_title(conn, repo_id, id)?.unwrap_or_default(),
    })
}

/// `on task 2.3 Login form` style text for a node.
fn node_text(n: &NodeRef) -> String {
    format!("{} {}", n.id, n.title).trim_end().to_string()
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut short: String = text.chars().take(max - 1).collect();
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

/// The navigator light in words — `on task`, `asked for`, `detour 25m`, `drift ⚠ 40m` —
/// plus the current detour and a `⚠` when the agent disagreed with the heuristic. Also
/// whether the last unit is drift and whether the open detour is one the owner asked for.
fn nav_text(conn: &Connection, s: &Session) -> Result<(String, bool, bool)> {
    let last = session::units(conn, &s.session_id)?
        .into_iter()
        .rev()
        .find(|u| u.stopped_at.is_some());
    let away = s.away_secs / 60;
    let drifting = last.as_ref().is_some_and(|u| u.label == Some(Label::Drift));
    let detour = match &s.current_detour {
        Some(id) => tangent::get_detour(conn, id)?,
        None => None,
    };
    let requested = detour.as_ref().is_some_and(|d| d.requested);
    let mut text = if s.current_detour.is_none() && drifting {
        format!("drift ⚠ {away}m")
    } else if requested && s.away_secs == 0 {
        "asked for".to_string()
    } else if s.current_detour.is_some() || s.away_secs > 0 {
        format!("detour {away}m")
    } else {
        "on task".to_string()
    };
    if let Some(d) = &detour {
        let _ = write!(text, " · {} \"{}\"", d.label(), cut(&d.reason, 30));
    }
    if !drifting && last.is_some_and(|u| u.disagreement) {
        text.push_str(" ⚠");
    }
    Ok((text, drifting, requested))
}

/// A checkpoint was asked for and missed, or the briefing found the session unstable.
fn handoff_trouble(s: &Session) -> bool {
    (s.checkpoint_missed != 0 && s.checkpoint_pending.is_some())
        || s.briefing_status == Some(BriefingStatus::Unstable)
}

/// The session's flags with their ages, in display order.
fn flags(conn: &Connection, s: &Session, now: Timestamp) -> Result<Vec<SessionFlag>> {
    let mut out = Vec::new();
    let flag = |kind, ts: Option<String>, tasks| SessionFlag {
        kind,
        age_s: age(now, ts.as_deref()),
        tasks,
    };
    if s.checkpoint_pending.is_some() {
        if s.checkpoint_missed != 0 {
            let ts = latest_ts(conn, &s.session_id, "checkpoint_missed")?;
            out.push(flag(FlagKind::CheckpointMissed, ts, Vec::new()));
        } else {
            let ts = latest_ts(conn, &s.session_id, "checkpoint_due")?;
            out.push(flag(FlagKind::CheckpointPending, ts, Vec::new()));
        }
    }
    if s.readback == ReadbackState::Pending {
        let ts = latest_ts(conn, &s.session_id, "briefed")?;
        out.push(flag(FlagKind::ReadbackPending, ts, Vec::new()));
    }
    let pending = verify::pending_claims(conn, &s.session_id, Timestamp::MAX)?;
    if !pending.is_empty() {
        let tasks: Vec<String> = pending.into_iter().map(|(t, _)| t).collect();
        let ts = latest_ts(conn, &s.session_id, "claimed_done")?;
        out.push(flag(FlagKind::VerifyPending, ts, tasks));
    }
    if s.briefing_status == Some(BriefingStatus::Unstable) {
        let ts = latest_ts(conn, &s.session_id, "briefed")?;
        out.push(flag(FlagKind::HandoffUnstable, ts, Vec::new()));
    }
    let failed = latest_ts(conn, &s.session_id, "squash_failed")?;
    if let Some(ts) = failed {
        let fixed = latest_ts(conn, &s.session_id, "squashed")?;
        let later = |a: &str, b: &str| parse_ts(a) > parse_ts(b);
        if fixed.as_deref().is_none_or(|f| later(&ts, f)) {
            out.push(flag(FlagKind::SquashFailed, Some(ts), Vec::new()));
        }
    }
    Ok(out)
}

/// Its active (or last) task was claimed done or verified in this session, and nothing
/// is owed: no checkpoint, no open detour, no question.
fn done_task(conn: &Connection, s: &Session, asked: bool) -> Result<Option<NodeRef>> {
    if asked || s.checkpoint_pending.is_some() || s.current_detour.is_some() {
        return Ok(None);
    }
    let Some(task) = s.active_task.as_ref().or(s.last_task.as_ref()) else {
        return Ok(None);
    };
    let claimed: i64 = conn
        .prepare_cached("SELECT count(*) FROM claims_done WHERE session_id = ?1 AND task_id = ?2")?
        .query_row(params![s.session_id, task], |r| r.get(0))?;
    let status = get_node(conn, &s.repo_id, task)?.map(|n| n.status);
    let finished = matches!(
        status,
        Some(
            NodeStatus::Claimed | NodeStatus::L1 | NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4
        )
    );
    if claimed > 0 && finished {
        return Ok(Some(node_ref(conn, &s.repo_id, task)?));
    }
    Ok(None)
}

/// What decides a session's light, words and queue items.
#[allow(clippy::struct_excessive_bools)] // independent facts, read once
struct Facts {
    nav: String,
    drifting: bool,
    task: Option<NodeRef>,
    /// An open detour the owner didn't ask for (asked-for work raises no alarm).
    has_detour: bool,
    /// The open detour is work the owner asked for.
    requested: bool,
    /// An open question: (source, since).
    question: Option<(String, String)>,
    /// A permission prompt or a form blocks the session (Claude idle at the prompt
    /// doesn't): (kind, since).
    blocking_wait: Option<(String, String)>,
    red: bool,
    /// Red, and the owner hasn't answered it yet.
    red_open: bool,
}

fn facts(conn: &Connection, s: &Session) -> Result<Facts> {
    let (nav, drifting, requested) = nav_text(conn, s)?;
    let task = match &s.active_task {
        Some(id) => Some(node_ref(conn, &s.repo_id, id)?),
        None => None,
    };
    let wait = match s.state {
        SessionState::NeedsOwner => open_wait(conn, &s.session_id)?,
        _ => None,
    };
    let red = s.nav_state == NavState::Red;
    Ok(Facts {
        nav,
        drifting,
        task,
        has_detour: s.current_detour.is_some() && !requested,
        requested,
        question: open_question(conn, &s.session_id)?,
        blocking_wait: wait.filter(|(kind, _)| kind != "idle_prompt"),
        red,
        red_open: red && !drift_answered(conn, s)?,
    })
}

/// A drift-red item's title: "b1 · far off its task (40 min)", the time away when known.
fn far_off_title(short: &str, away_secs: i64) -> String {
    if away_secs >= 60 {
        format!("{short} · far off its task ({} min)", away_secs / 60)
    } else {
        format!("{short} · far off its task")
    }
}

/// The queue items a session raises: drift red, a question or a blocking wait.
fn session_items(
    conn: &Connection,
    s: &Session,
    f: &Facts,
    repo_name: &str,
    now: Timestamp,
) -> Result<Vec<NeedsYou>> {
    let mut items = Vec::new();
    let item = |kind: NeedsKind, title: String, detail: String, since: Option<&str>| NeedsYou {
        id: format!("{}:{}", kind.as_str(), s.session_id),
        kind,
        repo_id: s.repo_id.clone(),
        repo_name: repo_name.to_string(),
        session_id: Some(s.session_id.clone()),
        task: f.task.clone(),
        title,
        detail,
        files: Vec::new(),
        actions: Vec::new(),
        respond_in: (kind != NeedsKind::DriftRed).then(|| RespondIn {
            session_short: short(&s.session_id),
            worktree: s.worktree.clone(),
        }),
        rank: kind.rank(),
        age_s: age(now, since).unwrap_or(0),
    };
    let on_task = f
        .task
        .as_ref()
        .map(|t| format!(" · on {}", node_text(t)))
        .unwrap_or_default();
    if f.red_open {
        let since = conn
            .prepare_cached(
                "SELECT ts FROM events WHERE session_id = ?1 AND kind = 'nav_state_changed'
                   AND json_extract(data, '$.to') = 'red' ORDER BY seq DESC LIMIT 1",
            )?
            .query_row([&s.session_id], |r| r.get::<_, String>(0))
            .optional()?;
        let mut red = item(
            NeedsKind::DriftRed,
            far_off_title(&short(&s.session_id), s.away_secs),
            format!("{}{on_task}", f.nav),
            since.as_deref(),
        );
        red.actions = drift_actions(
            &s.session_id,
            s.current_detour.is_some(),
            s.active_task.as_deref(),
        );
        items.push(red);
    }
    let ago = |since: &str| age(now, Some(since)).unwrap_or(0) / 60;
    if let Some((_, since)) = &f.question {
        items.push(item(
            NeedsKind::Question,
            format!("{} asked you a question", short(&s.session_id)),
            format!("{} min ago · answer in that session{on_task}", ago(since)),
            Some(since),
        ));
    } else if let Some((kind, since)) = &f.blocking_wait {
        let what = if kind == "permission_prompt" {
            "waits on a permission prompt"
        } else {
            "waits on your input"
        };
        items.push(item(
            NeedsKind::PermissionWait,
            format!("{} {what}", short(&s.session_id)),
            format!("{} min ago · answer in that session{on_task}", ago(since)),
            Some(since),
        ));
    }
    Ok(items)
}

/// The light and its word, the terminal's state column and the sort rank.
fn looks(s: &Session, f: &Facts, rest: Option<&Rest>) -> (Light, &'static str, &'static str, u8) {
    let done = matches!(rest, Some(Rest::Done { .. }));
    let (light, word) = if f.red_open || matches!(rest, Some(Rest::NeedsYou { .. })) {
        (Light::Red, "needs you")
    } else if s.state == SessionState::Working || f.red {
        if f.drifting && !f.has_detour {
            (Light::Amber, "drift")
        } else if f.has_detour || s.away_secs > 0 || f.red {
            (Light::Amber, "detour")
        } else if f.requested {
            (Light::Green, "asked for")
        } else {
            (Light::Green, "on task")
        }
    } else if done {
        (Light::Green, "done")
    } else {
        (Light::Amber, "idle")
    };
    // The terminal's state column says "needs you" only for real needs-you items (owner
    // decision 2026-10-14); agent-owed trouble (a missed checkpoint, an unstable handoff)
    // is a flag on the row, and a session merely at the prompt is `idle` or `done`.
    let needs_you = light == Light::Red;
    let label = if needs_you {
        "needs you"
    } else {
        match s.state {
            SessionState::Working => "working",
            SessionState::Ended => "ended",
            SessionState::Idle | SessionState::NeedsOwner if done => "done",
            SessionState::Idle | SessionState::NeedsOwner => "idle",
        }
    };
    let rank = if f.red_open {
        0
    } else if needs_you {
        1
    } else if handoff_trouble(s) {
        2
    } else if s.state == SessionState::Working {
        3
    } else {
        4
    };
    (light, word, label, rank)
}

/// One session row plus the queue items it raises.
fn session_row(
    conn: &Connection,
    cfg: &Config,
    s: &Session,
    repo_name: &str,
    now: Timestamp,
) -> Result<(SessionRow, Vec<NeedsYou>)> {
    let f = facts(conn, s)?;
    let items = session_items(conn, s, &f, repo_name, now)?;
    let asked = f.question.is_some();
    let rest = match s.state {
        SessionState::Working if !asked => None,
        _ if asked => Some(Rest::NeedsYou {
            why: NeedsKind::Question,
        }),
        _ if f.blocking_wait.is_some() => Some(Rest::NeedsYou {
            why: NeedsKind::PermissionWait,
        }),
        _ => Some(match done_task(conn, s, asked)? {
            Some(task) => Rest::Done { task },
            None => Rest::Idle,
        }),
    };
    let (light, light_word, state_label, rank) = looks(s, &f, rest.as_ref());
    let detour = match &s.current_detour {
        Some(id) => tangent::get_detour(conn, id)?.map(|d| DetourRef {
            label: d.label(),
            reason: d.reason,
        }),
        None => None,
    };
    let row = SessionRow {
        session_id: s.session_id.clone(),
        short: short(&s.session_id),
        repo_id: s.repo_id.clone(),
        state_label: state_label.to_string(),
        light,
        light_word: light_word.to_string(),
        task: f.task,
        detour,
        units: s.unit_count,
        active_min: s.active_secs / 60,
        nav_text: f.nav,
        flags: flags(conn, s, now)?,
        rest,
        asked_question: asked,
        branch: s.branch.clone(),
        worktree: s.worktree.clone(),
        cost: cost_row(conn, &telemetry::Scope::Session(s.session_id.clone()), cfg)?,
        rank,
    };
    Ok((row, items))
}

fn button(label: &str, action: OwnerAction, tone: Tone) -> ActionSpec {
    ActionSpec {
        label: label.to_string(),
        action,
        inputs: Vec::new(),
        tone,
    }
}

fn input(name: &str, label: &str, required: bool) -> ActionInput {
    ActionInput {
        name: name.to_string(),
        label: label.to_string(),
        required,
    }
}

/// Continue (optional reason) / Make it a task (only with a detour to promote) / Stop.
fn drift_actions(session: &str, has_detour: bool, task: Option<&str>) -> Vec<ActionSpec> {
    let drift = |answer| OwnerAction::Drift {
        session: session.to_string(),
        answer,
        parent: None,
        title: None,
        estimate: None,
        reason: None,
    };
    let mut cont = button("Continue", drift(DriftAnswer::Continue), Tone::Primary);
    cont.inputs = vec![input("reason", "reason (optional)", false)];
    let mut out = vec![cont];
    if has_detour {
        let mut make = button(
            "Make it a task",
            drift(DriftAnswer::MakeTask),
            Tone::Neutral,
        );
        if let OwnerAction::Drift { parent, .. } = &mut make.action {
            // The task's parent is the natural home for the promoted work.
            *parent = task.and_then(|t| t.rsplit_once('.').map(|(p, _)| p.to_string()));
        }
        make.inputs = vec![
            input("parent", "Under (e.g. 10)", true),
            input("title", "Task title", true),
        ];
        out.push(make);
    }
    out.push(button("Stop", drift(DriftAnswer::Stop), Tone::Danger));
    out
}

/// Escalated verifications of a repo (spec §8.5): ✓ Works / ✗ Broken.
fn verify_items(
    conn: &Connection,
    repo_id: &str,
    repo_name: &str,
    now: Timestamp,
) -> Result<Vec<NeedsYou>> {
    let mut out = Vec::new();
    for (task, title) in verify::overdue(conn, repo_id)? {
        let since: Option<String> = conn
            .prepare_cached(
                "SELECT ts FROM events WHERE repo_id = ?1 AND kind = 'needs_owner'
                   AND json_extract(data, '$.kind') = 'verify'
                   AND json_extract(data, '$.task_id') = ?2
                 ORDER BY pos DESC LIMIT 1",
            )?
            .query_row(params![repo_id, task], |r| r.get(0))
            .optional()?;
        let level = verify::current_level(conn, repo_id, &task)?;
        let node = NodeRef {
            id: task.clone(),
            title,
        };
        let mut broken = button(
            "✗ Broken",
            OwnerAction::Broken {
                repo: repo_id.to_string(),
                task: task.clone(),
                note: String::new(),
            },
            Tone::Danger,
        );
        broken.inputs = vec![input("note", "what's wrong", true)];
        out.push(NeedsYou {
            id: format!("verify_overdue:{repo_id}:{task}"),
            kind: NeedsKind::VerifyOverdue,
            repo_id: repo_id.to_string(),
            repo_name: repo_name.to_string(),
            session_id: None,
            title: format!("Verify {}", node_text(&node)),
            detail: format!(
                "claimed done (L{level}), nudged twice without a check · does it work?"
            ),
            task: Some(node),
            files: Vec::new(),
            actions: vec![
                button(
                    "✓ Works",
                    OwnerAction::Verify {
                        repo: repo_id.to_string(),
                        task: task.clone(),
                        level: 4,
                    },
                    Tone::Primary,
                ),
                broken,
            ],
            respond_in: None,
            rank: NeedsKind::VerifyOverdue.rank(),
            age_s: age(now, since.as_deref()).unwrap_or(0),
        });
    }
    Ok(out)
}

/// One item per dead session with leftovers the owner can decide on.
fn leftover_items(
    conn: &Connection,
    repo_id: &str,
    repo_name: &str,
    now: Timestamp,
) -> Result<Vec<NeedsYou>> {
    let mut by_session: BTreeMap<String, Vec<handoff::Leftover>> = BTreeMap::new();
    for l in handoff::leftovers(conn, repo_id, true)? {
        if l.state == LeftoverState::Open {
            by_session
                .entry(l.from_session.clone())
                .or_default()
                .push(l);
        }
    }
    let mut out = Vec::new();
    for (from, list) in by_session {
        // One worktree per resolution: a session's leftovers share its worktree.
        let wt = list[0].worktree.clone();
        let list: Vec<handoff::Leftover> = list.into_iter().filter(|l| l.worktree == wt).collect();
        let ids: Vec<String> = list.iter().map(|l| l.leftover_id.clone()).collect();
        let files: Vec<String> = list.iter().map(|l| l.path.clone()).collect();
        let task = match list.iter().find_map(|l| l.task_id.clone()) {
            Some(t) => Some(node_ref(conn, repo_id, &t)?),
            None => None,
        };
        let suggested = list[0].suggested;
        let first = list.iter().filter_map(|l| parse_ts(&l.ts)).min();
        let resolve = |action| OwnerAction::Resolve {
            repo: Some(repo_id.to_string()),
            leftover_ids: ids.clone(),
            action,
        };
        let tone = |a: LeftoverAction| {
            if a == suggested {
                Tone::Primary
            } else {
                Tone::Neutral
            }
        };
        let n = files.len();
        let on = task
            .as_ref()
            .map(|t| format!(" · {}", node_text(t)))
            .unwrap_or_default();
        out.push(NeedsYou {
            id: format!("leftover:{repo_id}:{from}"),
            kind: NeedsKind::Leftover,
            repo_id: repo_id.to_string(),
            repo_name: repo_name.to_string(),
            session_id: Some(from.clone()),
            task,
            title: format!(
                "S {} left {n} uncommitted file{}",
                short(&from),
                if n == 1 { "" } else { "s" }
            ),
            detail: format!("suggested: {} · in {wt}{on}", suggested.as_str()),
            files,
            actions: vec![
                button(
                    "Adopt",
                    resolve(LeftoverAction::Adopt),
                    tone(LeftoverAction::Adopt),
                ),
                button(
                    "Resume",
                    resolve(LeftoverAction::Resume),
                    tone(LeftoverAction::Resume),
                ),
                button(
                    "Undo",
                    resolve(LeftoverAction::Undo),
                    tone(LeftoverAction::Undo),
                ),
            ],
            respond_in: None,
            rank: NeedsKind::Leftover.rank(),
            age_s: first.map_or(0, |t| now.duration_since(t).as_secs().max(0)),
        });
    }
    Ok(out)
}

/// ` · tuned drift .35–.65, tolerance 25%` without the separator.
fn tuned_text(conn: &Connection, repo_id: &str, cfg: &Config) -> Result<Option<String>> {
    let th = tangent::thresholds(conn, repo_id, cfg)?;
    if th == tangent::Thresholds::from_config(cfg) {
        return Ok(None);
    }
    let short = |v: f64| {
        let t = format!("{v:.2}");
        t.strip_prefix('0').map_or(t.clone(), str::to_string)
    };
    Ok(Some(format!(
        "tuned drift {}–{}, tolerance {}%",
        short(th.lower),
        short(th.upper),
        th.tolerance_pct
    )))
}

fn pct(part: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    u32::try_from(part * 100 / total).unwrap_or(100)
}

/// Live leaves of a loaded tree: tasks and subtasks without live children.
pub fn leaves(tree: &[PlanNode]) -> Vec<&PlanNode> {
    let parents: HashSet<&str> = tree.iter().filter_map(|n| n.parent_id.as_deref()).collect();
    tree.iter()
        .filter(|n| matches!(n.kind, NodeKind::Task | NodeKind::Subtask))
        .filter(|n| !parents.contains(n.id.as_str()))
        .collect()
}

/// Every node under `root` (not `root` itself), by id.
pub fn descendants(tree: &[PlanNode], root: &str) -> HashSet<String> {
    let mut kids: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in tree {
        if let Some(p) = &n.parent_id {
            kids.entry(p.as_str()).or_default().push(n.id.as_str());
        }
    }
    let mut out = HashSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        for k in kids.get(id).into_iter().flatten() {
            if out.insert((*k).to_string()) {
                stack.push(k);
            }
        }
    }
    out
}

/// (done%, claimed%) of the leaves under a node.
fn subtree_progress(tree: &[PlanNode], under: &HashSet<String>) -> (u32, u32) {
    let (mut done, mut claimed, mut total) = (0_u64, 0_u64, 0_u64);
    for leaf in leaves(tree).into_iter().filter(|n| under.contains(&n.id)) {
        let minutes = u64::from(leaf.estimate_min.unwrap_or(0));
        total += minutes;
        match leaf.status {
            NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4 => done += minutes,
            NodeStatus::Claimed | NodeStatus::L1 => claimed += minutes,
            _ => {}
        }
    }
    (pct(done, total), pct(claimed, total))
}

/// Sessions needed without change for the stuck flag.
pub const STUCK_SESSIONS: usize = 3;

/// Kinds that move a milestone's hill state or done%.
const PROGRESS_KINDS: &[&str] = &[
    "hill_moved",
    "verified",
    "verification_failed",
    "verification_stale",
    "node_added",
    "node_removed",
    "node_moved",
    "estimate_set",
];

/// Stuck (spec §5.6): the last [`STUCK_SESSIONS`] sessions that worked on tasks under the
/// milestone came and went without its hill state or done% changing. Returns how many
/// sessions started since the last change (0 = not stuck).
fn stuck_for(
    conn: &Connection,
    repo_id: &str,
    milestone: &str,
    under: &HashSet<String>,
) -> Result<usize> {
    if under.is_empty() {
        return Ok(0);
    }
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT u.session_id, s.started_at, u.task_id FROM units u
         JOIN sessions s ON s.session_id = u.session_id
         WHERE s.repo_id = ?1 AND u.task_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([repo_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut sessions: HashMap<String, Timestamp> = HashMap::new();
    for row in rows {
        let (sid, started, task) = row?;
        if under.contains(&task)
            && let Some(t) = parse_ts(&started)
        {
            sessions.insert(sid, t);
        }
    }
    if sessions.len() < STUCK_SESSIONS {
        return Ok(0);
    }
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT kind, ts, task_id, data FROM events WHERE repo_id = ?1 AND kind IN ({})",
        PROGRESS_KINDS
            .iter()
            .map(|k| format!("'{k}'"))
            .collect::<Vec<_>>()
            .join(", ")
    ))?;
    let rows = stmt.query_map([repo_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    // The latest event that moved the milestone; the sessions since then count.
    let mut latest: Option<Timestamp> = None;
    for row in rows {
        let (kind, ts, task, data) = row?;
        let Some(at) = parse_ts(&ts) else {
            continue;
        };
        if latest.is_some_and(|l| at <= l) {
            continue;
        }
        let data: serde_json::Value = serde_json::from_str(&data)?;
        let id = data["id"].as_str().or(task.as_deref()).unwrap_or("");
        let touches = match kind.as_str() {
            "hill_moved" => id == milestone,
            "verified" => under.contains(id) && data["level"].as_i64().unwrap_or(0) >= 2,
            "node_added" => data["parent_id"]
                .as_str()
                .is_some_and(|p| p == milestone || under.contains(p)),
            "node_moved" => {
                under.contains(id)
                    || data["new_parent_id"]
                        .as_str()
                        .is_some_and(|p| p == milestone || under.contains(p))
            }
            _ => under.contains(id),
        };
        if touches {
            latest = Some(at);
        }
    }
    let unchanged = sessions
        .values()
        .filter(|start| latest.is_none_or(|l| **start > l))
        .count();
    Ok(if unchanged >= STUCK_SESSIONS {
        unchanged
    } else {
        0
    })
}

fn milestones(conn: &Connection, repo_id: &str, tree: &[PlanNode]) -> Result<Vec<MilestoneRow>> {
    let rolled = crate::plan::rollups(tree);
    let mut out = Vec::new();
    for m in tree.iter().filter(|n| n.kind == NodeKind::Milestone) {
        let under = descendants(tree, &m.id);
        let (done_pct, claimed_pct) = subtree_progress(tree, &under);
        let stuck_sessions = stuck_for(conn, repo_id, &m.id, &under)?;
        out.push(MilestoneRow {
            id: m.id.clone(),
            title: m.title.clone(),
            status: rolled
                .get(&m.id)
                .copied()
                .unwrap_or(crate::plan::Rollup::Todo),
            hill: m.hill,
            done_pct,
            claimed_pct,
            stuck: stuck_sessions > 0,
            stuck_sessions,
        });
    }
    Ok(out)
}

/// Repos worth a row: those with live sessions, needs-you items or a plan.
fn repo_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM repos r WHERE
           EXISTS (SELECT 1 FROM sessions s WHERE s.repo_id = r.id AND s.state != 'ended')
           OR EXISTS (SELECT 1 FROM plan_nodes n WHERE n.repo_id = r.id AND n.status != 'dropped')
           OR EXISTS (SELECT 1 FROM leftovers l WHERE l.repo_id = r.id AND l.state != 'resolved')
           OR EXISTS (SELECT 1 FROM verify_nudges v WHERE v.repo_id = r.id AND v.escalated = 1)
         UNION
         SELECT DISTINCT repo_id FROM sessions WHERE state != 'ended'",
    )?;
    Ok(stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Build the glance at `now` (spec §10.1).
pub fn glance(conn: &Connection, cfg: &Config, now: Timestamp) -> Result<Glance> {
    let live = session::list_live(conn, None)?;
    let mut by_repo: HashMap<String, Vec<Session>> = HashMap::new();
    for s in &live {
        by_repo
            .entry(s.repo_id.clone())
            .or_default()
            .push(s.clone());
    }
    let mut projects = Vec::new();
    let mut queue = Vec::new();
    for repo_id in repo_ids(conn)? {
        let name = repo_name(conn, &repo_id)?.unwrap_or_else(|| repo_id.clone());
        let mut rows = Vec::new();
        let mut items = Vec::new();
        // `list_live` is newest first; a stable sort keeps that within each rank.
        for s in by_repo.remove(&repo_id).unwrap_or_default() {
            let (row, raised) = session_row(conn, cfg, &s, &name, now)?;
            rows.push(row);
            items.extend(raised);
        }
        rows.sort_by_key(|r| r.rank);
        items.extend(verify_items(conn, &repo_id, &name, now)?);
        items.extend(leftover_items(conn, &repo_id, &name, now)?);
        let tree = load_tree(conn, &repo_id, false)?;
        let goal = get_node(conn, &repo_id, "G")?
            .filter(|g| g.status != NodeStatus::Dropped)
            .map(|g| NodeRef {
                id: g.id,
                title: g.title,
            });
        let p = verify::progress(conn, &repo_id)?;
        let (open, report_only) = crate::recovery::open_counts(conn, &repo_id)?;
        let forecast = match &goal {
            Some(_) => Some(forecast_row(conn, &repo_id, now)?),
            None => None,
        };
        let goal_status = goal.as_ref().map(|g| {
            crate::plan::rollups(&tree)
                .get(&g.id)
                .copied()
                .unwrap_or(crate::plan::Rollup::Todo)
        });
        projects.push(ProjectRow {
            repo_id: repo_id.clone(),
            name,
            goal,
            goal_status,
            done_pct: p.done_pct,
            claimed_pct: p.claimed_pct,
            unestimated: p.unestimated,
            milestones: milestones(conn, &repo_id, &tree)?,
            leftovers: LeftoverCounts { open, report_only },
            tuned: tuned_text(conn, &repo_id, cfg)?,
            forecast,
            cost: cost_row(conn, &telemetry::Scope::Repo(repo_id.clone()), cfg)?,
            needs_you: items.len(),
            sessions: rows,
        });
        queue.extend(items);
    }
    projects.sort_by(|a, b| (&a.name, &a.repo_id).cmp(&(&b.name, &b.repo_id)));
    // Only items with an owner response count (principle 3).
    queue.retain(|i| !i.actions.is_empty() || i.respond_in.is_some());
    queue.sort_by(|a, b| {
        (a.rank, std::cmp::Reverse(a.age_s), &a.id).cmp(&(
            b.rank,
            std::cmp::Reverse(b.age_s),
            &b.id,
        ))
    });
    let count = queue.len();
    Ok(Glance {
        as_of: now.to_string(),
        last_event_pos: last_event_pos(conn)?,
        live_sessions: live.len(),
        projects,
        needs_you: queue,
        needs_you_count: count,
        escalated: count > ESCALATE_ABOVE,
    })
}

#[cfg(test)]
mod tests;
