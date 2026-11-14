//! Text injected into the agent's context by hooks. Short and fixed-format.

use std::fmt::Write as _;

use anyhow::Result;
use bridge_screen_core::claim::{Candidate, candidates};
use bridge_screen_core::plan::track::{Track, open_tracks};
use bridge_screen_core::session::Session;
use rusqlite::Connection;

/// How many candidates a claim reminder lists.
const REMINDER_CANDIDATES: usize = 3;

/// A reminder goes out on the 1st, 4th, 7th, … prompt of a session without a task.
const REMINDER_EVERY: i64 = 3;

/// UserPromptSubmit: while the session has no task, every third prompt lists the tasks the
/// prompt most likely belongs to, across the open tracks (with the track's name when there
/// are two or more). `unit_no` is the unit this prompt opened.
pub fn claim_reminder(
    conn: &Connection,
    session: &Session,
    unit_no: i64,
    prompt: &str,
) -> Result<Option<String>> {
    if session.active_task.is_some() || (unit_no - 1) % REMINDER_EVERY != 0 {
        return Ok(None);
    }
    let open = open_tracks(conn, &session.repo_id)?;
    if open.is_empty() {
        return Ok(None);
    }
    let found = candidates(conn, &session.repo_id, prompt, REMINDER_CANDIDATES)?;
    let several = open.len() > 1;
    let mut text = String::from("bridge-screen: no active task yet. ");
    if found.is_empty() {
        text.push_str("No plan task matches this request by keyword.\n");
    } else {
        text.push_str("Likely matches for this request:\n  ");
        text.push_str(&candidate_line(&found, several.then_some(open.as_slice())));
        text.push('\n');
    }
    text.push_str(
        "If one clearly fits, call claim_task with mode \"auto\". If unsure where the work stands, ask the owner. If none fits, claim a new task under the right parent.",
    );
    if several && found.is_empty() {
        let _ = write!(
            text,
            " This folder has {} open tracks ({}): if this is a new piece of work, ask the owner whether to continue a track or start a new one (start_track).",
            open.len(),
            open.iter()
                .map(|t| format!("{} {}", t.no, t.title))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(Some(text))
}

/// `1.2 Login form (score .62)  ·  1.3 Session store (.31)`; with `tracks` (two or more are
/// open) each match also names its track: `T2-1.1 Readme [Docs refresh] (.31)`.
fn candidate_line(found: &[Candidate], tracks: Option<&[Track]>) -> String {
    found
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let score = short_score(c.score);
            let name = tracks
                .and_then(|ts| ts.iter().find(|t| t.no == c.track))
                .map(|t| format!(" [{}]", t.title))
                .unwrap_or_default();
            if i == 0 {
                format!("{} {}{name} (score {score})", c.task_id, c.title)
            } else {
                format!("{} {}{name} ({score})", c.task_id, c.title)
            }
        })
        .collect::<Vec<_>>()
        .join("  ·  ")
}

/// `.62` for 0.62, `1.00` for 1.
fn short_score(score: f32) -> String {
    let text = format!("{score:.2}");
    text.strip_prefix('0').map_or(text.clone(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{candidate_line, short_score};
    use bridge_screen_core::claim::Candidate;
    use bridge_screen_core::plan::NodeStatus;

    #[test]
    fn scores_are_short() {
        assert_eq!(short_score(0.625), ".62");
        assert_eq!(short_score(0.1), ".10");
        assert_eq!(short_score(1.0), "1.00");
    }

    #[test]
    fn candidates_line_up() {
        let c = |id: &str, title: &str, score: f32| Candidate {
            task_id: id.into(),
            title: title.into(),
            score,
            status: NodeStatus::Todo,
            track: 1,
        };
        assert_eq!(
            candidate_line(
                &[
                    c("1.2", "Login form", 0.62),
                    c("1.3", "Session store", 0.31),
                    c("2.1", "Schema", 0.1)
                ],
                None
            ),
            "1.2 Login form (score .62)  ·  1.3 Session store (.31)  ·  2.1 Schema (.10)"
        );
    }
}
