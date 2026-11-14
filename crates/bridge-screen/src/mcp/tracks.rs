//! The MCP side of tracks: `start_track` (spec 2026-11-14 §2).

use bridge_screen_core::plan::EditContext;
use bridge_screen_core::plan::track::{self, goal_id, raw};
use bridge_screen_core::store::Actor;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Args, Bridge, msg};

/// Start a separate piece of work in this folder.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartTrackInput {
    /// The track's goal: one line, at most 200 characters.
    pub title: String,
    /// Why it matters and what done looks like: 2-3 sentences.
    pub intent: String,
    /// When the folder already has an open track: what the owner said when you asked
    /// "continue track X or start a new one?". Required then; never made up.
    #[serde(default)]
    pub owner_said: Option<String>,
}

impl Bridge {
    pub(super) fn do_start_track(&self, args: &Args<StartTrackInput>) -> Result<String, String> {
        let mut store = self.open()?;
        let session = self.caller(&store, "start_track", &args.raw)?;
        let input = args.parsed.as_ref().map_err(Clone::clone)?;
        let ctx = EditContext {
            repo_id: session.repo_id.clone(),
            session_id: session.session_id.clone(),
            actor: Actor::default(),
            reason: Some(match input.owner_said.as_deref().map(str::trim) {
                Some(said) if !said.is_empty() => {
                    format!("track started; the owner said: {said}")
                }
                _ => "first track of the folder".to_string(),
            }),
            replan: false,
        };
        let goal = store
            .write(|tx| {
                track::start_track(
                    tx,
                    &ctx,
                    &input.title,
                    &input.intent,
                    input.owner_said.as_deref(),
                )
            })
            .map_err(msg)?;
        let no = track::track_of(&goal);
        debug_assert_eq!(goal, goal_id(no));
        Ok(format!(
            "Started track {no} '{}' (goal {goal}). Add its first milestone with plan_edit under \
             parent {goal}; its ids will read {}, {}, … and restart from 1.",
            input.title.trim(),
            raw(no, "1"),
            raw(no, "1.1")
        ))
    }
}
