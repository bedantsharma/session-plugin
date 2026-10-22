//! The MCP side of waits (spec §9.1, §11.2): `waiting_on`, `clear_wait`, `promote_wait`.

use bridge_screen_core::store::Actor;
use bridge_screen_core::waits;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Args, Bridge, msg};

/// What this session is waiting on, and why.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitingOnInput {
    /// A task id ("2.4"), "<repo>:<task id>" for a task in another repo, or free text.
    pub target: String,
    /// Why you are waiting on it, in one line.
    pub reason: String,
}

/// One of this session's waits.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitIdInput {
    /// The wait's id, as `waiting_on` or `status` gave it (e.g. "W3").
    pub wait_id: String,
}

impl Bridge {
    pub(super) fn do_waiting_on(&self, args: &Args<WaitingOnInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "waiting_on", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let opened = store
            .write(|tx| {
                waits::open(
                    tx,
                    &session,
                    &input.target,
                    &input.reason,
                    &Actor::default(),
                )
            })
            .map_err(msg)?;
        let id = &opened.wait.wait_id;
        let mut reply = if opened.wait.target_task.is_some() {
            format!(
                "Waiting on {} ({id}). You'll get a note here when it's done.",
                opened.display
            )
        } else {
            format!(
                "Waiting on {} ({id}). Clear it with clear_wait when it arrives.",
                opened.display
            )
        };
        if opened.offer_link {
            reply.push_str(
                " If your task will always need it, ask the owner whether to make it a plan link (promote_wait).",
            );
        }
        Ok(reply)
    }

    pub(super) fn do_clear_wait(&self, args: &Args<WaitIdInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "clear_wait", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let w = store
            .write(|tx| {
                waits::clear(
                    tx,
                    &input.wait_id,
                    Some(&session.session_id),
                    &Actor::default(),
                )
            })
            .map_err(msg)?;
        Ok(format!(
            "Cleared {}: no longer waiting on {}.",
            w.wait_id, w.target_text
        ))
    }

    pub(super) fn do_promote_wait(&self, args: &Args<WaitIdInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "promote_wait", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        store
            .write(|tx| {
                waits::promote(
                    tx,
                    &input.wait_id,
                    Some(&session.session_id),
                    &Actor::default(),
                )
            })
            .map_err(msg)
    }
}
