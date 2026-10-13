//! The event envelope (spec §5.1) and appending events.

use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// Schema version of event `data` payloads.
pub const EVENT_VERSION: i64 = 1;

/// Session id used for events the owner causes (CLI, dashboard).
pub const OWNER_SESSION: &str = "owner";

/// Which agent or subagent caused an event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// An event before it is stored: no id, timestamp, position or sequence yet.
#[derive(Debug, Clone, PartialEq)]
pub struct EventDraft {
    pub repo_id: String,
    pub session_id: String,
    pub kind: String,
    pub actor: Actor,
    pub task_id: Option<String>,
    pub data: Value,
}

impl EventDraft {
    pub fn new(
        repo_id: impl Into<String>,
        session_id: impl Into<String>,
        kind: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            repo_id: repo_id.into(),
            session_id: session_id.into(),
            kind: kind.into(),
            actor: Actor::default(),
            task_id: None,
            data,
        }
    }

    #[must_use]
    pub fn task(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    #[must_use]
    pub fn actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }
}

/// A stored event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub pos: i64,
    pub id: String,
    pub v: i64,
    pub ts: String,
    pub repo_id: String,
    pub session_id: String,
    pub seq: i64,
    pub kind: String,
    pub actor: Actor,
    pub task_id: Option<String>,
    pub data: Value,
}

/// Append one event and update the views it affects, inside the caller's transaction.
pub fn append_in(tx: &Transaction, draft: EventDraft) -> Result<Event> {
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
        [&draft.session_id],
        |r| r.get(0),
    )?;
    let id = ulid::Ulid::generate().to_string();
    let ts = jiff::Timestamp::now().to_string();
    tx.execute(
        "INSERT INTO events (id, v, ts, repo_id, session_id, seq, kind, actor, task_id, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            EVENT_VERSION,
            ts,
            draft.repo_id,
            draft.session_id,
            seq,
            draft.kind,
            serde_json::to_string(&draft.actor)?,
            draft.task_id,
            serde_json::to_string(&draft.data)?,
        ],
    )?;
    let event = Event {
        pos: tx.last_insert_rowid(),
        id,
        v: EVENT_VERSION,
        ts,
        repo_id: draft.repo_id,
        session_id: draft.session_id,
        seq,
        kind: draft.kind,
        actor: draft.actor,
        task_id: draft.task_id,
        data: draft.data,
    };
    crate::projection::apply(tx, &event)?;
    Ok(event)
}

/// All events (optionally for one repo), in the order they were appended.
pub fn read_all(conn: &Connection, repo_id: Option<&str>) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT pos, id, v, ts, repo_id, session_id, seq, kind, actor, task_id, data
         FROM events WHERE (?1 IS NULL OR repo_id = ?1) ORDER BY pos",
    )?;
    let rows = stmt.query_map([repo_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, Option<String>>(9)?,
            r.get::<_, String>(10)?,
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (pos, id, v, ts, repo_id, session_id, seq, kind, actor, task_id, data) = row?;
        events.push(Event {
            pos,
            id,
            v,
            ts,
            repo_id,
            session_id,
            seq,
            kind,
            actor: serde_json::from_str(&actor)?,
            task_id,
            data: serde_json::from_str(&data)?,
        });
    }
    Ok(events)
}
