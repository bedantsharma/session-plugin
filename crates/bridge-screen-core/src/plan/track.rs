//! Tracks (spec 2026-11-14 §2): one folder can hold several unrelated pieces of work, each
//! with its own goal, bar, numbering and runs.
//!
//! A track is its goal node. Track 1 keeps every id it has always had (`G`, `3`, `3.1.2`);
//! track `k` ≥ 2 prefixes the same grammar with `T<k>-` (`T2-G`, `T2-1`, `T2-1.3`). The
//! track is read from the id alone, so every call that takes one raw id needs no track
//! argument. This module owns that grammar; nothing else splits ids.
//!
//! No `tracks` table: the list is read from the goal nodes plus `track_closed` /
//! `track_reopened` events when asked, like runs, so a rebuild has nothing to reproduce.

use std::collections::HashMap;

use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};

use super::model::{NodeKind, NodeStatus};
use crate::error::{Error, Result};
use crate::store::{Actor, EventDraft, append_in};

/// The owner finished the track (`track_closed`) or took that back (`track_reopened`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackEvent {
    pub track: u32,
}

/// `T2-1.3` → `Some(2)`; `None` when `id` has no `T<digits>-` prefix (or the number is 0).
fn prefix_of(id: &str) -> Option<(u32, &str)> {
    let mut chars = id.chars();
    if !matches!(chars.next(), Some('T' | 't')) {
        return None;
    }
    let rest = &id[1..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let after = rest[digits.len()..].strip_prefix('-')?;
    let no = digits.parse::<u32>().ok().filter(|n| *n > 0)?;
    if after.is_empty() {
        return None;
    }
    Some((no, after))
}

/// The track an id belongs to: the number in its `T<k>-` prefix, else 1.
pub fn track_of(id: &str) -> u32 {
    prefix_of(id).map_or(1, |(n, _)| n)
}

/// The id without its track prefix (`T2-1.3` → `1.3`; `1.3` → `1.3`).
pub fn local(id: &str) -> &str {
    prefix_of(id).map_or(id, |(_, rest)| rest)
}

/// The raw id for `local` in `track` (track 1 has no prefix).
pub fn raw(track: u32, local: &str) -> String {
    if track <= 1 {
        local.to_string()
    } else {
        format!("T{track}-{local}")
    }
}

/// The goal id of a track: `G`, `T2-G`, …
pub fn goal_id(track: u32) -> String {
    raw(track, "G")
}

/// Is `id` a track's goal id?
pub fn is_goal_id(id: &str) -> bool {
    local(id).eq_ignore_ascii_case("G")
}

/// The first number of the local id (`T2-3.1.2` → 3); `None` for the goal.
pub fn top_number(id: &str) -> Option<u32> {
    local(id).split('.').next().and_then(|p| p.parse().ok())
}

/// A track is open until the owner finishes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackState {
    Open,
    Closed,
}

/// One track: its number, goal and state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Track {
    pub no: u32,
    pub goal_id: String,
    pub title: String,
    pub intent: Option<String>,
    pub state: TrackState,
}

impl Track {
    pub fn is_open(&self) -> bool {
        self.state == TrackState::Open
    }
}

/// The newest `track_closed` / `track_reopened` of each track: `true` = closed.
fn closed_set(conn: &Connection, repo_id: &str) -> Result<HashMap<u32, bool>> {
    let mut stmt = conn.prepare_cached(
        "SELECT kind, data FROM events
         WHERE repo_id = ?1 AND kind IN ('track_closed', 'track_reopened') ORDER BY pos",
    )?;
    let rows = stmt.query_map([repo_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (kind, data) = row?;
        let e: TrackEvent = serde_json::from_str(&data)?;
        out.insert(e.track, kind == "track_closed");
    }
    Ok(out)
}

/// Every track of the repo (goal nodes that were not removed), by number.
pub fn tracks(conn: &Connection, repo_id: &str) -> Result<Vec<Track>> {
    let closed = closed_set(conn, repo_id)?;
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, intent FROM plan_nodes
         WHERE repo_id = ?1 AND kind = 'goal' AND parent_id IS NULL AND status != 'dropped'",
    )?;
    let rows = stmt.query_map([repo_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, title, intent) = row?;
        let no = track_of(&id);
        out.push(Track {
            no,
            goal_id: id,
            title,
            intent,
            state: if closed.get(&no).copied().unwrap_or(false) {
                TrackState::Closed
            } else {
                TrackState::Open
            },
        });
    }
    out.sort_by_key(|t| t.no);
    Ok(out)
}

/// The tracks the owner has not finished.
pub fn open_tracks(conn: &Connection, repo_id: &str) -> Result<Vec<Track>> {
    Ok(tracks(conn, repo_id)?
        .into_iter()
        .filter(Track::is_open)
        .collect())
}

/// One track, if it exists and was not removed.
pub fn track(conn: &Connection, repo_id: &str, no: u32) -> Result<Option<Track>> {
    Ok(tracks(conn, repo_id)?.into_iter().find(|t| t.no == no))
}

/// The track a single-plan screen shows: the lowest open track, else the lowest, else 1.
/// Used where one plan is still assumed.
pub fn primary(conn: &Connection, repo_id: &str) -> Result<u32> {
    let all = tracks(conn, repo_id)?;
    Ok(all
        .iter()
        .find(|t| t.is_open())
        .or_else(|| all.first())
        .map_or(1, |t| t.no))
}

/// The number the next track gets: one more than the highest ever used, removed tracks
/// included (numbers are never reused). 1 when there never was one.
pub fn next_track_no(conn: &Connection, repo_id: &str) -> Result<u32> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM plan_nodes WHERE repo_id = ?1 AND kind = 'goal' AND parent_id IS NULL",
    )?;
    let ids = stmt.query_map([repo_id], |r| r.get::<_, String>(0))?;
    let mut highest = 0;
    for id in ids {
        highest = highest.max(track_of(&id?));
    }
    Ok(highest + 1)
}

/// Is any goal there, removed or not? (The first goal ever is `G`.)
pub fn any_goal_ever(conn: &Connection, repo_id: &str) -> Result<bool> {
    let n: i64 = conn
        .prepare_cached(
            "SELECT count(*) FROM plan_nodes
             WHERE repo_id = ?1 AND kind = 'goal' AND parent_id IS NULL",
        )?
        .query_row([repo_id], |r| r.get(0))?;
    Ok(n > 0)
}

/// `track 1 'Ship it', track 2 'Docs'` — the open tracks, for messages.
pub fn open_list_text(open: &[Track]) -> String {
    open.iter()
        .map(|t| format!("track {} '{}'", t.no, t.title))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Is the track finished? A track that does not exist counts as open.
pub fn is_closed(conn: &Connection, repo_id: &str, no: u32) -> Result<bool> {
    Ok(closed_set(conn, repo_id)?
        .get(&no)
        .copied()
        .unwrap_or(false))
}

/// The leaf tasks of a track that are neither done nor removed.
pub fn unfinished_leaves(
    conn: &Connection,
    repo_id: &str,
    no: u32,
) -> Result<(Vec<(String, String)>, usize)> {
    let tree: Vec<_> = super::load_tree(conn, repo_id, false)?
        .into_iter()
        .filter(|n| track_of(&n.id) == no)
        .collect();
    let leaves: Vec<_> = tree
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Task | NodeKind::Subtask))
        .filter(|n| !tree.iter().any(|c| c.parent_id.as_deref() == Some(&n.id)))
        .collect();
    let open = leaves
        .iter()
        .filter(|n| !n.status.counts_as_done())
        .map(|n| (n.id.clone(), n.title.clone()))
        .collect();
    Ok((open, leaves.len()))
}

/// Finish a track on the owner's say-so. Refused, in words and with nothing written, while a
/// task is neither done nor removed.
pub fn close_track_in(
    tx: &Transaction,
    repo_id: &str,
    session_id: &str,
    actor: &Actor,
    no: u32,
) -> Result<Track> {
    let t =
        track(tx, repo_id, no)?.ok_or_else(|| Error::Invalid(format!("there is no track {no}")))?;
    if !t.is_open() {
        return Err(Error::Stale(format!("'{}' is already finished", t.title)));
    }
    let (open, total) = unfinished_leaves(tx, repo_id, no)?;
    if !open.is_empty() {
        let runs = super::runs::load_all(tx, repo_id)?;
        let names: Vec<String> = open
            .iter()
            .take(3)
            .map(|(id, title)| format!("{} {title}", runs.shown(id)))
            .collect();
        let more = open.len().saturating_sub(3);
        return Err(Error::Invalid(format!(
            "can't finish track '{}' yet: {} of {total} tasks aren't done ({}{}). If the rest \
             was abandoned, remove it first; nothing was changed.",
            t.title,
            open.len(),
            names.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        )));
    }
    append_in(
        tx,
        EventDraft::new(
            repo_id,
            session_id,
            "track_closed",
            serde_json::to_value(TrackEvent { track: no })?,
        )
        .actor(actor.clone()),
    )?;
    Ok(Track {
        state: TrackState::Closed,
        ..t
    })
}

/// Take a finished track back into the open list.
pub fn reopen_track_in(
    tx: &Transaction,
    repo_id: &str,
    session_id: &str,
    actor: &Actor,
    no: u32,
) -> Result<Track> {
    let t =
        track(tx, repo_id, no)?.ok_or_else(|| Error::Invalid(format!("there is no track {no}")))?;
    if t.is_open() {
        return Err(Error::Stale(format!("'{}' is already open", t.title)));
    }
    append_in(
        tx,
        EventDraft::new(
            repo_id,
            session_id,
            "track_reopened",
            serde_json::to_value(TrackEvent { track: no })?,
        )
        .actor(actor.clone()),
    )?;
    Ok(Track {
        state: TrackState::Open,
        ..t
    })
}

/// Which tracks have a live (not removed) goal node — a cheap check for "is there a plan".
pub fn has_live_goal(conn: &Connection, repo_id: &str) -> Result<bool> {
    let n: i64 = conn
        .prepare_cached(
            "SELECT count(*) FROM plan_nodes
             WHERE repo_id = ?1 AND kind = 'goal' AND parent_id IS NULL AND status != 'dropped'",
        )?
        .query_row(params![repo_id], |r| r.get(0))?;
    Ok(n > 0)
}

/// Is `status` a live node's? (Helper for readers that skip removed work.)
pub fn live(status: NodeStatus) -> bool {
    status != NodeStatus::Dropped
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::plan::{EditContext, PlanError, PlanOp, get_node, plan_edit, runs, start_track_in};
    use crate::store::{Actor, EventDraft, Store};

    fn ctx() -> EditContext {
        EditContext {
            repo_id: "r".into(),
            session_id: "owner".into(),
            actor: Actor::default(),
            reason: Some("setup".into()),
            replan: false,
        }
    }

    fn add(parent: Option<&str>, kind: NodeKind, title: &str) -> PlanOp {
        PlanOp::AddNode {
            parent: parent.map(str::to_string),
            kind,
            title: title.into(),
            intent: matches!(kind, NodeKind::Goal | NodeKind::Milestone).then(|| "why".into()),
            estimate_min: matches!(kind, NodeKind::Task).then_some(30),
            expected_files: Vec::new(),
            check: None,
        }
    }

    fn edit(s: &mut Store, ops: &[PlanOp]) -> crate::error::Result<Vec<String>> {
        plan_edit(s, &ctx(), ops).map(|o| o.added_ids)
    }

    /// Track 1: G, 1, 1.1, 1.2. Track 2: T2-G, T2-1, T2-1.1.
    fn two_tracks() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        edit(
            &mut s,
            &[
                add(None, NodeKind::Goal, "Ship"),
                add(Some("G"), NodeKind::Milestone, "Core"),
                add(Some("1"), NodeKind::Task, "Schema"),
                add(Some("1"), NodeKind::Task, "Append"),
            ],
        )
        .unwrap();
        let id = s
            .write(|tx| start_track_in(tx, &ctx(), "Docs refresh", "Docs are current"))
            .unwrap();
        assert_eq!(id, "T2-G");
        edit(
            &mut s,
            &[
                add(Some("T2-G"), NodeKind::Milestone, "Pages"),
                add(Some("T2-1"), NodeKind::Task, "Readme"),
            ],
        )
        .unwrap();
        s
    }

    fn pass(s: &mut Store, task: &str) {
        s.append(
            EventDraft::new(
                "r",
                "owner",
                "verified",
                json!({"verif_id": format!("v{task}"), "task_id": task, "level": 2,
                       "sha": "abc", "files": [], "evidence": "ok", "by": "agent"}),
            )
            .task(task),
        )
        .unwrap();
    }

    #[test]
    fn old_ids_are_track_one() {
        let mut s = Store::open_in_memory().unwrap();
        edit(
            &mut s,
            &[
                add(None, NodeKind::Goal, "Ship"),
                add(Some("G"), NodeKind::Milestone, "Core"),
                add(Some("1"), NodeKind::Task, "Schema"),
            ],
        )
        .unwrap();
        let all = tracks(s.conn(), "r").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].no, all[0].goal_id.as_str()), (1, "G"));
        assert!(all[0].is_open());
        let r = runs::load(s.conn(), "r", 1).unwrap();
        assert_eq!((r.shown("1.1"), r.shown("G")), ("1.1".into(), "G".into()));
        pass(&mut s, "1.1");
        assert_eq!(
            crate::verify::progress(s.conn(), "r", 1).unwrap().done_pct,
            100
        );
        // A run_closed event written before tracks existed is track 1's.
        s.append(EventDraft::new(
            "r",
            "owner",
            "run_closed",
            json!({"at_milestone_next": 2, "closed_ids": ["1"], "closed_at": "2026-10-20T00:00:00Z"}),
        ))
        .unwrap();
        assert_eq!(runs::load(s.conn(), "r", 1).unwrap().current_first, 2);
        assert_eq!(runs::load(s.conn(), "r", 2).unwrap().current_first, 1);
    }

    #[test]
    fn second_goal_refused_while_track_open() {
        let mut s = two_tracks();
        let err = edit(&mut s, &[add(None, NodeKind::Goal, "Another")]).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("track 1 'Ship'")
                && text.contains("track 2 'Docs refresh'")
                && text.contains("start_track"),
            "{text}"
        );
        assert!(matches!(
            err,
            crate::error::Error::Plan(PlanError::GoalExists { .. })
        ));
    }

    #[test]
    fn goal_allowed_after_all_closed() {
        let mut s = Store::open_in_memory().unwrap();
        edit(&mut s, &[add(None, NodeKind::Goal, "Ship")]).unwrap();
        s.append(EventDraft::new(
            "r",
            "owner",
            "track_closed",
            json!({"track": 1}),
        ))
        .unwrap();
        assert_eq!(open_tracks(s.conn(), "r").unwrap().len(), 0);
        assert_eq!(
            edit(&mut s, &[add(None, NodeKind::Goal, "Next")]).unwrap(),
            vec!["T2-G"]
        );
    }

    #[test]
    fn ids_in_track_two_start_at_one() {
        let s = two_tracks();
        let ids: Vec<String> = crate::plan::load_tree(s.conn(), "r", false)
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .filter(|id| track_of(id) == 2)
            .collect();
        assert_eq!(ids, vec!["T2-G", "T2-1", "T2-1.1"]);
        let mut s = s;
        let added = edit(
            &mut s,
            &[
                add(Some("T2-1"), NodeKind::Task, "Guide"),
                add(Some("T2-1.2"), NodeKind::Subtask, "Intro"),
            ],
        )
        .unwrap();
        assert_eq!(added, vec!["T2-1.2", "T2-1.2.1"]);
        // Track 1 is untouched.
        assert_eq!(
            edit(&mut s, &[add(Some("1"), NodeKind::Task, "More")]).unwrap(),
            vec!["1.3"]
        );
    }

    #[test]
    fn track_numbers_never_reused() {
        let mut s = Store::open_in_memory().unwrap();
        edit(&mut s, &[add(None, NodeKind::Goal, "One")]).unwrap();
        // Track 1 removed (a track is its goal; the dropped state is the same either way).
        s.append(EventDraft::new("r", "owner", "node_removed", json!({"id": "G"})).task("G"))
            .unwrap();
        assert_eq!(tracks(s.conn(), "r").unwrap().len(), 0);
        assert_eq!(next_track_no(s.conn(), "r").unwrap(), 2);
        let id = s
            .write(|tx| start_track_in(tx, &ctx(), "Two", "why"))
            .unwrap();
        assert_eq!(id, "T2-G");
        s.append(EventDraft::new("r", "owner", "node_removed", json!({"id": "T2-G"})).task("T2-G"))
            .unwrap();
        assert_eq!(next_track_no(s.conn(), "r").unwrap(), 3);
        assert!(
            get_node(s.conn(), "r", "G")
                .unwrap()
                .is_some_and(|g| g.status == NodeStatus::Dropped)
        );
    }

    #[test]
    fn runs_are_per_track() {
        let mut s = two_tracks();
        pass(&mut s, "T2-1.1");
        assert!(runs::finished(s.conn(), "r", 2).unwrap());
        assert!(!runs::finished(s.conn(), "r", 1).unwrap());
        // A new milestone in track 2 closes track 2's finished run only.
        edit(
            &mut s,
            &[
                add(Some("T2-G"), NodeKind::Milestone, "Guides"),
                add(Some("T2-2"), NodeKind::Task, "Howto"),
            ],
        )
        .unwrap();
        let two = runs::load(s.conn(), "r", 2).unwrap();
        assert_eq!(two.closed.len(), 1);
        assert_eq!(two.current_first, 2);
        let one = runs::load(s.conn(), "r", 1).unwrap();
        assert_eq!((one.closed.len(), one.current_first), (0, 1));
        // Display numbers restart per track and per run; the prefix is dropped.
        let all = runs::load_all(s.conn(), "r").unwrap();
        assert_eq!(all.shown("T2-2.1"), "1.1");
        assert_eq!(all.shown("T2-1.1"), "1.1");
        assert_eq!(all.shown("1.2"), "1.2");
        assert_eq!(all.shown("T2-G"), "G");
        // Track 1's tree and progress ignore track 2 entirely.
        let ids: Vec<String> = runs::run_tree(s.conn(), "r", 1)
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(ids, vec!["G", "1", "1.1", "1.2"]);
        // Replaying every event rebuilds the same runs.
        s.rebuild().unwrap();
        assert_eq!(runs::load_all(s.conn(), "r").unwrap(), all);
    }

    #[test]
    fn progress_per_track_and_pooled() {
        let mut s = two_tracks();
        pass(&mut s, "1.1");
        let p1 = crate::verify::progress(s.conn(), "r", 1).unwrap();
        let p2 = crate::verify::progress(s.conn(), "r", 2).unwrap();
        let all = crate::verify::progress_all(s.conn(), "r").unwrap();
        assert_eq!((p1.done_pct, p1.total_min), (50, 60));
        assert_eq!((p2.done_pct, p2.total_min), (0, 30));
        // Estimate-weighted: 30 of 90 minutes.
        assert_eq!((all.done_min, all.total_min, all.done_pct), (30, 90, 33));
    }

    #[test]
    fn plan_edit_into_closed_track_refused() {
        let mut s = two_tracks();
        pass(&mut s, "T2-1.1");
        s.write(|tx| close_track_in(tx, "r", "owner", &Actor::default(), 2))
            .unwrap();
        let err = edit(&mut s, &[add(Some("T2-1"), NodeKind::Task, "More")]).unwrap_err();
        assert!(
            err.to_string().contains("track 'Docs refresh' is finished"),
            "{err}"
        );
        assert!(edit(&mut s, &[add(Some("T2-G"), NodeKind::Milestone, "More")]).is_err());
        // Track 1 still takes work; the owner reopening track 2 lets it take work again.
        edit(&mut s, &[add(Some("1"), NodeKind::Task, "More")]).unwrap();
        s.write(|tx| reopen_track_in(tx, "r", "owner", &Actor::default(), 2))
            .unwrap();
        edit(&mut s, &[add(Some("T2-1"), NodeKind::Task, "More")]).unwrap();
    }

    #[test]
    fn run_closed_without_track_is_track_one() {
        let d: runs::RunClosed = serde_json::from_value(
            json!({"at_milestone_next": 3, "closed_ids": [], "closed_at": "t"}),
        )
        .unwrap();
        assert_eq!(d.track, 1);
        // Track 1's event is written the old way, track 2's names its track.
        let one = serde_json::to_value(&d).unwrap();
        assert!(one.get("track").is_none());
        let two = serde_json::to_value(runs::RunClosed { track: 2, ..d }).unwrap();
        assert_eq!(two["track"], 2);
    }

    #[test]
    fn prefix_grammar() {
        assert_eq!(track_of("T2-1.3"), 2);
        assert_eq!(local("T2-1.3"), "1.3");
        assert_eq!(track_of("t2-G"), 2);
        assert_eq!(local("t2-G"), "G");
        assert_eq!(track_of("1.3"), 1);
        assert_eq!(local("1.3"), "1.3");
        assert_eq!(track_of("G"), 1);
        for bad in ["T-1", "T0-1", "T2", "T2-", "Tx-1", "TT2-1", "", "T-"] {
            assert_eq!(track_of(bad), 1, "{bad}");
            assert_eq!(local(bad), bad, "{bad}");
        }
        assert_eq!(raw(1, "1.2"), "1.2");
        assert_eq!(raw(3, "1.2"), "T3-1.2");
        assert_eq!(goal_id(1), "G");
        assert_eq!(goal_id(4), "T4-G");
        assert_eq!(top_number("T2-3.1.2"), Some(3));
        assert_eq!(top_number("G"), None);
        assert_eq!(top_number("T2-G"), None);
        assert_eq!(top_number("5"), Some(5));
        assert!(is_goal_id("T2-G") && is_goal_id("G") && !is_goal_id("1"));
    }
}
