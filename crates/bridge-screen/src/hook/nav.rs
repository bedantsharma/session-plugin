//! The navigator's part of a hook: after the hook's own events, label units, track the
//! tolerance band and pick at most one note (spec §7, §11.1).

use bridge_screen_core::config::Config;
use bridge_screen_core::drift::{self, ScoreInput};
use bridge_screen_core::error::Result;
use bridge_screen_core::navigator::{self, Channel, Step};
use bridge_screen_core::session::{self, Session, current_unit};
use bridge_screen_core::store::Actor;
use bridge_screen_core::tangent;
use jiff::Timestamp;
use rusqlite::Connection;

use super::{HookEvent, HookInput};

/// Hooks that may label units or send notes.
pub fn navigates(event: HookEvent) -> bool {
    matches!(
        event,
        HookEvent::Stop | HookEvent::UserPromptSubmit | HookEvent::PostToolUse
    )
}

/// What the navigator needs to know about this hook call.
pub struct NavCtx<'a> {
    pub event: HookEvent,
    pub input: &'a HookInput,
    pub config: &'a Config,
    /// For Stop: the unit it closed.
    pub stopped_unit: Option<i64>,
    pub actor: Actor,
    /// Only for decisions (live away time); never written into views.
    pub now: Timestamp,
}

/// The unit a Stop is about to close and its drift score (`None` without a plan). The
/// agent's last message is used here in memory only.
pub fn score_open_unit(
    conn: &Connection,
    session: &Session,
    input: &HookInput,
) -> Result<Option<(i64, Option<f64>)>> {
    let Some(unit) = current_unit(conn, &session.session_id)? else {
        return Ok(None);
    };
    let against = drift::context(
        conn,
        &session.repo_id,
        unit.task_id.as_deref(),
        (&session.session_id, unit.unit_no),
    )?;
    let score = against.map(|c| {
        drift::score(&ScoreInput {
            unit_files: &unit.files,
            summary: input.last_assistant_message.as_deref().unwrap_or(""),
            task_files: &c.task_files,
            intent_text: &c.intent_text,
            mapped_dirs: &c.mapped_dirs,
        })
        .score
    });
    Ok(Some((unit.unit_no, score)))
}

/// The navigator's events and note for this hook, given the views after its own events.
pub fn step(conn: &Connection, nc: &NavCtx) -> Result<Step> {
    let Some(session) = session::get(conn, &nc.input.session_id)? else {
        return Ok(Step::default());
    };
    let th = tangent::thresholds(conn, &session.repo_id, nc.config)?;
    let mut step = match nc.event {
        HookEvent::Stop => match nc.stopped_unit {
            Some(n) => match session::unit(conn, &session.session_id, n)? {
                Some(unit) => {
                    navigator::label_at_stop(conn, &session, &unit, nc.input.stop_hook_active, &th)?
                }
                None => Step::default(),
            },
            None => Step::default(),
        },
        HookEvent::UserPromptSubmit => Step {
            events: navigator::label_left_unlabeled(conn, &session, &th)?
                .into_iter()
                .collect(),
            note: None,
        },
        _ => Step::default(),
    };
    for draft in &mut step.events {
        draft.actor = nc.actor.clone();
    }
    Ok(step)
}

/// After [`step`]'s events: the tolerance band and the notes for this hook's channel.
pub fn state_step(conn: &Connection, nc: &NavCtx) -> Result<Step> {
    let channel = match nc.event {
        HookEvent::Stop => Channel::Stop,
        HookEvent::UserPromptSubmit => Channel::PromptSubmit,
        HookEvent::PostToolUse => Channel::PostToolUse,
        _ => return Ok(Step::default()),
    };
    let Some(session) = session::get(conn, &nc.input.session_id)? else {
        return Ok(Step::default());
    };
    let th = tangent::thresholds(conn, &session.repo_id, nc.config)?;
    let mut step = navigator::nav_step(conn, &session.session_id, channel, &th, nc.now)?;
    for draft in &mut step.events {
        draft.actor = nc.actor.clone();
    }
    Ok(step)
}
