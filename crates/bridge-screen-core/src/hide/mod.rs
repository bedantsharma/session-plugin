//! Hiding from the front screen (spec 2026-11-14 §3): the owner hides a needs-you card, a
//! track's bar or a whole project; hidden things sit in a "Hidden" drawer where they can be
//! brought back, and come back by themselves only when something new happens to them.
//!
//! "New" is a **stamp**, not a timer: the position of the newest event that counts as
//! something new for the thing hidden. Hiding stores the stamp it saw; the thing stays
//! hidden while its stamp is unchanged and returns when it moves. No clock, so it replays
//! the same on rebuild. Hiding never changes a number: progress, forecasts, task counts
//! and costs are untouched.

use std::collections::HashMap;

use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::plan::track::track_of;
use crate::store::Event;

/// Event kinds routed to this projector.
pub const HIDE_KINDS: &[&str] = &["hidden", "unhidden"];

/// The owner hid something (owner session). The repo is the event's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hidden {
    /// `card:<needs-you id>`, `track:<repo>:<n>` or `project:<repo>`.
    pub subject: String,
    /// What it was called on screen, for the drawer.
    pub label: String,
    /// The stamp the owner saw.
    pub stamp: i64,
}

/// The owner brought something back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unhidden {
    pub subject: String,
}

/// What kind of thing a subject is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HiddenKind {
    Card,
    Track,
    Project,
}

impl HiddenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Track => "track",
            Self::Project => "project",
        }
    }
}

/// A subject, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A needs-you card by its id.
    Card(String),
    Track {
        repo: String,
        no: u32,
    },
    Project(String),
}

impl Subject {
    pub fn parse(text: &str) -> Option<Self> {
        if let Some(id) = text.strip_prefix("card:") {
            return (!id.is_empty()).then(|| Self::Card(id.to_string()));
        }
        if let Some(rest) = text.strip_prefix("track:") {
            let (repo, no) = rest.rsplit_once(':')?;
            return Some(Self::Track {
                repo: repo.to_string(),
                no: no.parse().ok()?,
            });
        }
        let repo = text.strip_prefix("project:")?;
        (!repo.is_empty()).then(|| Self::Project(repo.to_string()))
    }

    pub fn text(&self) -> String {
        match self {
            Self::Card(id) => format!("card:{id}"),
            Self::Track { repo, no } => track_subject(repo, *no),
            Self::Project(repo) => project_subject(repo),
        }
    }

    pub fn kind(&self) -> HiddenKind {
        match self {
            Self::Card(_) => HiddenKind::Card,
            Self::Track { .. } => HiddenKind::Track,
            Self::Project(_) => HiddenKind::Project,
        }
    }
}

pub fn card_subject(id: &str) -> String {
    format!("card:{id}")
}

pub fn track_subject(repo: &str, no: u32) -> String {
    format!("track:{repo}:{no}")
}

pub fn project_subject(repo: &str) -> String {
    format!("project:{repo}")
}

/// Apply one hide event to the `hides` view (event position and time only).
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    match event.kind.as_str() {
        "hidden" => {
            let d: Hidden = serde_json::from_value(event.data.clone())?;
            let kind = Subject::parse(&d.subject).map_or("card", |s| s.kind().as_str());
            tx.execute(
                "INSERT OR REPLACE INTO hides
                     (repo_id, subject, kind, label, stamp, hidden_pos, hidden_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    event.repo_id,
                    d.subject,
                    kind,
                    d.label,
                    d.stamp,
                    event.pos,
                    event.ts
                ],
            )?;
        }
        "unhidden" => {
            let d: Unhidden = serde_json::from_value(event.data.clone())?;
            tx.execute(
                "DELETE FROM hides WHERE repo_id = ?1 AND subject = ?2",
                params![event.repo_id, d.subject],
            )?;
        }
        _ => {}
    }
    Ok(())
}

/// One row of the `hides` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HideRow {
    pub repo_id: String,
    pub subject: String,
    pub kind: HiddenKind,
    pub label: String,
    /// The stamp the owner saw when hiding.
    pub stamp: i64,
    /// When it was hidden (the event's time).
    pub hidden_at: String,
}

/// Every hide row of the repo (or of all repos).
pub fn rows(conn: &Connection, repo: Option<&str>) -> Result<Vec<HideRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT repo_id, subject, kind, label, stamp, hidden_at FROM hides
         WHERE (?1 IS NULL OR repo_id = ?1) ORDER BY hidden_pos DESC",
    )?;
    let rows = stmt.query_map([repo], |r| {
        let kind: String = r.get(2)?;
        Ok(HideRow {
            repo_id: r.get(0)?,
            subject: r.get(1)?,
            kind: match kind.as_str() {
                "track" => HiddenKind::Track,
                "project" => HiddenKind::Project,
                _ => HiddenKind::Card,
            },
            label: r.get(3)?,
            stamp: r.get(4)?,
            hidden_at: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The hide row of a subject, if one exists.
pub fn row_of(conn: &Connection, repo: &str, subject: &str) -> Result<Option<HideRow>> {
    Ok(rows(conn, Some(repo))?
        .into_iter()
        .find(|r| r.subject == subject))
}

/// The newest position among events of `kinds` in the repo whose task is `task` (any task
/// when `None`).
fn newest(conn: &Connection, repo: &str, kinds: &[&str], task: Option<&str>) -> Result<i64> {
    let list = kinds
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT COALESCE(MAX(pos), 0) FROM events
         WHERE repo_id = ?1 AND kind IN ({list}) AND (?2 IS NULL OR task_id = ?2)"
    );
    Ok(conn.query_row(&sql, params![repo, task], |r| r.get(0))?)
}

/// The newest event of `kinds` for a session.
fn newest_of_session(
    conn: &Connection,
    repo: &str,
    session: &str,
    kinds: &[&str],
    extra: &str,
) -> Result<i64> {
    let list = kinds
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT COALESCE(MAX(pos), 0) FROM events
         WHERE repo_id = ?1 AND session_id = ?2 AND kind IN ({list}) {extra}"
    );
    Ok(conn.query_row(&sql, params![repo, session], |r| r.get(0))?)
}

/// The stamp of a needs-you card, by its id (`kind:target`): the position of the newest
/// event that raises that card again. 0 when none.
///
/// | card | newest… |
/// |---|---|
/// | `drift_red:<session>` | `drift_flagged`, or a turn to red (`nav_state_changed`), of the session |
/// | `question:<session>` | `owner_question` of the session |
/// | `permission_wait:<session>` | `needs_owner` (a prompt, not a drift or a check) of the session |
/// | `verify_overdue:<repo>:<task>` | `claimed_done` or `verification_failed` of the task |
/// | `leftover:<repo>:<session>` | `leftover_found` in the repo |
/// | `plan_scope:<repo>` | `plan_scope_asked` in the repo |
pub fn stamp_of_card(conn: &Connection, repo: &str, card_id: &str) -> Result<i64> {
    let mut parts = card_id.splitn(3, ':');
    let kind = parts.next().unwrap_or_default();
    match kind {
        "drift_red" => {
            let session = parts.next().unwrap_or_default();
            newest_of_session(
                conn,
                repo,
                session,
                &["drift_flagged", "nav_state_changed"],
                "AND (kind = 'drift_flagged' OR json_extract(data, '$.to') = 'red')",
            )
        }
        "question" => {
            let session = parts.next().unwrap_or_default();
            newest_of_session(conn, repo, session, &["owner_question"], "")
        }
        "permission_wait" => {
            let session = parts.next().unwrap_or_default();
            newest_of_session(
                conn,
                repo,
                session,
                &["needs_owner"],
                "AND COALESCE(json_extract(data, '$.kind'), '') NOT IN ('verify', 'drift')",
            )
        }
        "verify_overdue" => {
            let _repo = parts.next();
            let task = parts.next().unwrap_or_default();
            newest(
                conn,
                repo,
                &["claimed_done", "verification_failed"],
                Some(task),
            )
        }
        "leftover" => newest(conn, repo, &["leftover_found"], None),
        "plan_scope" => newest(conn, repo, &["plan_scope_asked"], None),
        _ => Ok(0),
    }
}

/// What counts as something new on a track's bars.
const TRACK_KINDS: &[&str] = &[
    "task_claimed",
    "claimed_done",
    "verified",
    "verification_failed",
    "node_added",
];

/// The stamp of every track of the repo: the newest event on a node of the track that
/// someone worked on, checked, added to or finished.
pub fn track_stamps(conn: &Connection, repo: &str) -> Result<HashMap<u32, i64>> {
    let mut out: HashMap<u32, i64> = HashMap::new();
    let list = TRACK_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT task_id, MAX(pos) FROM events
         WHERE repo_id = ?1 AND task_id IS NOT NULL AND kind IN ({list}) GROUP BY task_id"
    ))?;
    for row in stmt.query_map([repo], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (task, pos) = row?;
        let e = out.entry(track_of(&task)).or_insert(0);
        *e = (*e).max(pos);
    }
    let mut stmt = conn.prepare(
        "SELECT json_extract(data, '$.track'), pos FROM events
         WHERE repo_id = ?1 AND kind IN ('track_closed', 'track_reopened', 'run_closed')",
    )?;
    for row in stmt.query_map([repo], |r| {
        Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (track, pos) = row?;
        let no = track.and_then(|t| u32::try_from(t).ok()).unwrap_or(1);
        let e = out.entry(no).or_insert(0);
        *e = (*e).max(pos);
    }
    Ok(out)
}

/// The stamp of one track.
pub fn stamp_of_track(conn: &Connection, repo: &str, no: u32) -> Result<i64> {
    Ok(track_stamps(conn, repo)?.get(&no).copied().unwrap_or(0))
}

/// The stamp of a project: a session started in it, or anything inside it that would have
/// brought a card or a track back.
pub fn stamp_of_project(conn: &Connection, repo: &str) -> Result<i64> {
    let started = newest(conn, repo, &["session_started"], None)?;
    let cards = newest(
        conn,
        repo,
        &[
            "drift_flagged",
            "owner_question",
            "needs_owner",
            "claimed_done",
            "verification_failed",
            "leftover_found",
            "plan_scope_asked",
        ],
        None,
    )?;
    let tracks = track_stamps(conn, repo)?.into_values().max().unwrap_or(0);
    Ok(started.max(cards).max(tracks))
}

/// The current stamp of a subject in `repo`.
pub fn stamp_of(conn: &Connection, repo: &str, subject: &Subject) -> Result<i64> {
    match subject {
        Subject::Card(id) => stamp_of_card(conn, repo, id),
        Subject::Track { no, .. } => stamp_of_track(conn, repo, *no),
        Subject::Project(_) => stamp_of_project(conn, repo),
    }
}

/// Is a hide row still in effect at `stamp_now`?
pub fn is_hidden(row: &HideRow, stamp_now: i64) -> bool {
    row.stamp == stamp_now
}
