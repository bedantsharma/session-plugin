//! Unverified work (spec §8.5): claimed or L1 tasks lead the next briefing and are nudged
//! at the prompt; after two ignored nudges they go to the owner's needs-you queue.

use rusqlite::{Connection, OptionalExtension, params};

use super::{VerifyNudged, current_level};
use crate::error::Result;
use crate::session::{NeedsOwner, NeedsOwnerKind};
use crate::store::EventDraft;

/// Nudges ignored (the level unchanged) before the owner is asked.
pub const ESCALATE_AFTER: i64 = 2;

/// `verify_nudged` for a task at `level`, plus `needs_owner {kind: verify}` when this is
/// the nudge that makes it overdue: at least [`ESCALATE_AFTER`] nudges, the level the
/// same as at the first one, not escalated yet.
pub fn nudge_drafts(
    conn: &Connection,
    repo_id: &str,
    session_id: &str,
    task_id: &str,
    level: i64,
) -> Result<Vec<EventDraft>> {
    let data = VerifyNudged {
        task_id: task_id.to_string(),
        level,
    };
    let mut drafts = vec![
        EventDraft::new(
            repo_id,
            session_id,
            "verify_nudged",
            serde_json::to_value(data)?,
        )
        .task(task_id),
    ];
    let row: Option<(i64, i64, bool)> = conn
        .prepare_cached(
            "SELECT nudges, level_at_first, escalated FROM verify_nudges
             WHERE repo_id = ?1 AND task_id = ?2",
        )?
        .query_row(params![repo_id, task_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let overdue = match row {
        Some((nudges, first, escalated)) => {
            !escalated && nudges + 1 >= ESCALATE_AFTER && first == level
        }
        None => ESCALATE_AFTER <= 1,
    };
    if overdue {
        let owner = NeedsOwner {
            kind: NeedsOwnerKind::Verify,
            task_id: Some(task_id.to_string()),
        };
        drafts.push(
            EventDraft::new(
                repo_id,
                session_id,
                "needs_owner",
                serde_json::to_value(owner)?,
            )
            .task(task_id),
        );
    }
    Ok(drafts)
}

/// Tasks waiting on the owner's verification (escalated), with titles, by id.
pub fn overdue(conn: &Connection, repo_id: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT v.task_id, COALESCE(n.title, '') FROM verify_nudges v
         LEFT JOIN plan_nodes n ON n.repo_id = v.repo_id AND n.id = v.task_id
         WHERE v.repo_id = ?1 AND v.escalated = 1 AND COALESCE(n.status, '') != 'dropped'
         ORDER BY v.task_id",
    )?;
    let rows = stmt.query_map([repo_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Tasks this session claimed done before `before` (an earlier unit) that are still
/// claimed or at L1, with their level, by id.
pub fn pending_claims(
    conn: &Connection,
    session_id: &str,
    before: jiff::Timestamp,
) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.repo_id, c.task_id, c.ts FROM claims_done c
         JOIN plan_nodes n ON n.repo_id = c.repo_id AND n.id = c.task_id
         WHERE c.session_id = ?1 AND n.status IN ('claimed', 'l1')
         ORDER BY c.task_id",
    )?;
    let rows = stmt.query_map([session_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut out: Vec<(String, i64)> = Vec::new();
    for row in rows {
        let (repo, task, ts) = row?;
        let earlier = ts.parse::<jiff::Timestamp>().is_ok_and(|t| t < before);
        if earlier && !out.iter().any(|(t, _)| *t == task) {
            let level = current_level(conn, &repo, &task)?;
            out.push((task, level));
        }
    }
    Ok(out)
}
