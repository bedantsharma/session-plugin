//! Taking back the owner's verdict (spec §8.5): their newest sign-off still in force, or
//! their "broken" while the task is still failed. Owner only (CLI and dashboard); the
//! agent's tools have no way to do it. The task's status is recomputed from what is left;
//! the forecast's history is never rewritten.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::VerificationWithdrawn;
use crate::error::{Error, Result};
use crate::plan::{NodeStatus, get_node};
use crate::store::{EventDraft, OWNER_SESSION, Store, append_in};
use crate::tangent::OwnerAction;

/// The owner's verdict that can be taken back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Withdrawable {
    /// A sign-off's `verif_id`, or a "broken" verdict's event id.
    pub verification_id: String,
    /// `true` for a sign-off (✓), `false` for "broken" (✗).
    pub signoff: bool,
    /// The sign-off's level (0 for "broken").
    pub level: i64,
}

/// The owner's newest verdict on `task` that is still in force, if any.
pub fn withdrawable(conn: &Connection, repo_id: &str, task: &str) -> Result<Option<Withdrawable>> {
    // The newest sign-off by the owner that still counts, with the position of its event.
    let signoff: Option<(String, i64, i64)> = conn
        .prepare_cached(
            "SELECT v.verif_id, v.level, e.pos FROM verifications v
             JOIN events e ON e.repo_id = v.repo_id AND e.kind = 'verified'
                          AND json_extract(e.data, '$.verif_id') = v.verif_id
             WHERE v.repo_id = ?1 AND v.task_id = ?2 AND v.verified_by = 'owner'
               AND v.state IN ('current', 'stale')
             ORDER BY e.pos DESC LIMIT 1",
        )?
        .query_row(params![repo_id, task], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    // The newest failure of the task, when it is the owner's, still in force (the task is
    // failed) and not taken back already.
    let failed = get_node(conn, repo_id, task)?.is_some_and(|n| n.status == NodeStatus::Failed);
    let broken: Option<(String, i64)> = if failed {
        conn.prepare_cached(
            "SELECT e.id, e.pos FROM events e
             WHERE e.repo_id = ?1 AND e.kind = 'verification_failed'
               AND json_extract(e.data, '$.task_id') = ?2
               AND json_extract(e.data, '$.by') = 'owner'
               AND e.pos = (SELECT MAX(f.pos) FROM events f
                            WHERE f.repo_id = ?1 AND f.kind = 'verification_failed'
                              AND json_extract(f.data, '$.task_id') = ?2)
               AND NOT EXISTS (SELECT 1 FROM events w
                               WHERE w.repo_id = ?1 AND w.kind = 'verification_withdrawn'
                                 AND json_extract(w.data, '$.verification_id') = e.id)",
        )?
        .query_row(params![repo_id, task], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
    } else {
        None
    };
    Ok(match (signoff, broken) {
        (Some((id, level, s)), Some((_, b))) if s > b => Some(Withdrawable {
            verification_id: id,
            signoff: true,
            level,
        }),
        (_, Some((id, _))) => Some(Withdrawable {
            verification_id: id,
            signoff: false,
            level: 0,
        }),
        (Some((id, level, _)), None) => Some(Withdrawable {
            verification_id: id,
            signoff: true,
            level,
        }),
        (None, None) => None,
    })
}

/// Take back the owner's newest verdict on `task` (`verify <task> --withdraw`). Refused
/// as stale, recording nothing, when there is none. Returns the line to show.
pub fn withdraw(store: &mut Store, repo_id: &str, task: &str) -> Result<String> {
    let title = crate::claim::task_title(store.conn(), repo_id, task)?.unwrap_or_default();
    let name = format!("{task} {title}").trim_end().to_string();
    store.write(|tx| {
        let Some(w) = withdrawable(tx, repo_id, task)? else {
            return Err(Error::Stale(format!(
                "nothing to withdraw on {task}: you haven't signed it off or marked it broken"
            )));
        };
        let data = VerificationWithdrawn {
            task_id: task.to_string(),
            verification_id: w.verification_id.clone(),
        };
        append_in(
            tx,
            EventDraft::new(
                repo_id,
                OWNER_SESSION,
                "verification_withdrawn",
                serde_json::to_value(data)?,
            )
            .task(task),
        )?;
        let action = OwnerAction {
            action: "verify".into(),
            answer: Some("withdrawn".into()),
            target: Some(task.to_string()),
            reason: None,
        };
        append_in(
            tx,
            EventDraft::new(
                repo_id,
                OWNER_SESSION,
                "owner_action",
                serde_json::to_value(action)?,
            )
            .task(task),
        )?;
        Ok(if w.signoff {
            format!("Took back your sign-off on {name}; it's back to where it was before.")
        } else {
            format!(
                "Took back your \"broken\" verdict on {name}; it's back to where it was before."
            )
        })
    })
}
