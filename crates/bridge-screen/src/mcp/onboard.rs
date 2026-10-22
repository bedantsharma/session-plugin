//! The MCP side of onboarding: `init_plan` (spec §12.2) and the estimate ask at a task's
//! first claim (spec §12.1).

use std::fmt::Write as _;

use bridge_screen_core::error::Result as CoreResult;
use bridge_screen_core::init::{self, InitBy, InitDraft};
use bridge_screen_core::plan::{NodeStatus, get_node};
use bridge_screen_core::store::{Actor, Store};
use bridge_screen_core::verify::{self, L1Outcome};

use super::{Args, Bridge, msg};
use crate::hook::handoff::worktree_of;

/// Ask for an estimate when a task without one is claimed for the first time (and isn't
/// done already).
pub(super) fn asks_for_estimate(store: &Store, repo_id: &str, task: &str) -> CoreResult<bool> {
    let Some(node) = get_node(store.conn(), repo_id, task)? else {
        return Ok(false);
    };
    if node.estimate_min.is_some()
        || !matches!(
            node.status,
            NodeStatus::Todo | NodeStatus::Active | NodeStatus::Failed
        )
    {
        return Ok(false);
    }
    let claims: i64 = store.conn().query_row(
        "SELECT count(*) FROM events WHERE repo_id = ?1 AND kind = 'task_claimed' AND task_id = ?2",
        rusqlite::params![repo_id, task],
        |r| r.get(0),
    )?;
    Ok(claims <= 1)
}

impl Bridge {
    pub(super) fn do_init_plan(&self, args: &Args<InitDraft>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "init_plan", &args.raw)?;
        let draft = args.parsed.as_ref().map_err(Clone::clone)?;
        let actor = Actor::default();
        let by = InitBy {
            repo_id: &session.repo_id,
            session_id: &session.session_id,
            actor: &actor,
        };
        let applied = store.write(|tx| init::apply(tx, &by, draft)).map_err(msg)?;
        // L1 for each done task at the session's HEAD, as claim_done does.
        let wt = worktree_of(&session);
        let mut passed = 0;
        for task in &applied.done {
            if let Ok(outcome @ L1Outcome::Passed { .. }) =
                verify::check_l1(store.conn(), &wt, &session.repo_id, task)
                && let Some(draft) =
                    verify::l1_draft(&session.repo_id, &session.session_id, task, &outcome)
                        .map_err(msg)?
            {
                store.append(draft).map_err(msg)?;
                passed += 1;
            }
        }
        let mut reply = format!(
            "Plan created: {} milestone{}, {} task{}",
            applied.milestones,
            if applied.milestones == 1 { "" } else { "s" },
            applied.tasks,
            if applied.tasks == 1 { "" } else { "s" },
        );
        if applied.done.is_empty() {
            reply.push('.');
        } else {
            let _ = write!(
                reply,
                "; {} look done (checked: {passed} exist at HEAD).",
                applied.done.len()
            );
        }
        Ok(reply)
    }
}
