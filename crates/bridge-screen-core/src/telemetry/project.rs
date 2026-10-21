//! The projector for `usage`: `usage_replies` (one row per model reply, first seen wins)
//! and `transcript_cursors`.
//!
//! A reply goes to the event session's unit that was open at the reply's timestamp (the
//! latest unit with `started_at ≤ ts ≤ stopped_at`, an open unit having no end), and so
//! to that unit's task and detour. Timestamps come from the event, never from "now".

use jiff::Timestamp;
use rusqlite::{OptionalExtension, Transaction, params};

use super::Usage;
use crate::error::Result;
use crate::store::Event;

/// Event kinds routed to this projector.
pub const USAGE_KINDS: &[&str] = &["usage"];

/// Apply one `usage` event.
pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    let d: Usage = serde_json::from_value(event.data.clone())?;
    cursor(tx, event, &d)?;
    let units = Units::load(tx, &event.session_id)?;
    let mut insert = tx.prepare_cached(
        "INSERT OR IGNORE INTO usage_replies
             (message_id, repo_id, session_id, agent_id, agent_type, model, ts, unit_no, task_id,
              detour_id, input, output, cache_read, cache_write_5m, cache_write_1h)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )?;
    for r in &d.replies {
        let unit = r.ts.parse::<Timestamp>().ok().and_then(|t| units.at(t));
        let i = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        insert.execute(params![
            r.message_id,
            event.repo_id,
            event.session_id,
            d.agent_id,
            d.agent_type,
            r.model,
            r.ts,
            unit.map(|u| u.no),
            unit.and_then(|u| u.task.clone()),
            unit.and_then(|u| u.detour.clone()),
            i(r.input),
            i(r.output),
            i(r.cache_read),
            i(r.cache_write_5m),
            i(r.cache_write_1h),
        ])?;
    }
    Ok(())
}

/// The cursor only moves forward, except after a restart from 0 (a shrunk or replaced
/// file), when it is set to where that read ended.
fn cursor(tx: &Transaction, event: &Event, d: &Usage) -> Result<()> {
    let offset = i64::try_from(d.offset_after).unwrap_or(i64::MAX);
    let stored: Option<i64> = tx
        .query_row(
            "SELECT offset FROM transcript_cursors WHERE path = ?1",
            [&d.path],
            |r| r.get(0),
        )
        .optional()?;
    let new = match stored {
        Some(old) if !d.restarted => old.max(offset),
        _ => offset,
    };
    tx.execute(
        "INSERT INTO transcript_cursors (path, session_id, agent_id, offset)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET offset = excluded.offset",
        params![d.path, event.session_id, d.agent_id, new],
    )?;
    Ok(())
}

/// One unit's span and what it was charged to.
struct Span {
    no: i64,
    start: Timestamp,
    /// `None` while the unit is open.
    end: Option<Timestamp>,
    task: Option<String>,
    detour: Option<String>,
}

/// A session's units, newest first.
struct Units(Vec<Span>);

impl Units {
    fn load(tx: &Transaction, session: &str) -> Result<Self> {
        let mut stmt = tx.prepare_cached(
            "SELECT unit_no, started_at, stopped_at, task_id, detour_id FROM units
             WHERE session_id = ?1 ORDER BY unit_no DESC",
        )?;
        let rows = stmt.query_map([session], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut spans = Vec::new();
        for row in rows {
            let (no, start, stop, task, detour) = row?;
            let Ok(start) = start.parse() else {
                continue;
            };
            spans.push(Span {
                no,
                start,
                end: stop.and_then(|s| s.parse().ok()),
                task,
                detour,
            });
        }
        Ok(Self(spans))
    }

    /// The latest unit that started at or before `t` and hadn't stopped before it.
    fn at(&self, t: Timestamp) -> Option<&Span> {
        self.0
            .iter()
            .find(|u| u.start <= t)
            .filter(|u| u.end.is_none_or(|end| t <= end))
    }
}
