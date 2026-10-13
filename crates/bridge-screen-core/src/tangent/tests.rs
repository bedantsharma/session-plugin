use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::{
    ItemKind, Label, LabelSource, NavState, detours_for_task, focus, open_detours, suspended,
};
use crate::session::{self, task_time, units};
use crate::store::{EventDraft, Store, append_in_at};

const T0: &str = "2026-10-13T09:00:00Z";

fn at(secs: i64) -> Timestamp {
    T0.parse::<Timestamp>().unwrap() + SignedDuration::from_secs(secs)
}

fn ev(store: &mut Store, kind: &str, data: Value, secs: i64) {
    store
        .write(|tx| append_in_at(tx, EventDraft::new("r", "s", kind, data), at(secs)))
        .unwrap();
}

/// Session `s` in repo `r`, holding task 1.1.
fn session_on_task() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    ev(
        &mut s,
        "session_started",
        json!({"cwd": "/r", "source": "startup"}),
        0,
    );
    ev(
        &mut s,
        "task_claimed",
        json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        0,
    );
    s
}

fn open_detour(s: &mut Store, id: &str, item: &str, kind: &str, ref_id: &str, secs: i64) {
    ev(
        s,
        "detour_opened",
        json!({"detour_id": id, "reason": format!("fix {id}"), "blocks": "1.1",
               "resume_cue": format!("back to {ref_id}")}),
        secs,
    );
    ev(
        s,
        "item_suspended",
        json!({"item_id": item, "kind": kind, "ref_id": ref_id,
               "resume_cue": format!("back to {ref_id}"), "by_detour": id}),
        secs,
    );
}

fn unit(s: &mut Store, from: i64, to: i64) {
    ev(s, "prompt", json!({"chars": 1}), from);
    ev(s, "unit_stopped", json!({}), to);
}

fn detour_secs(s: &Store, id: &str) -> i64 {
    super::get_detour(s.conn(), id)
        .unwrap()
        .unwrap()
        .active_secs
}

#[test]
fn detour_time_is_charged_to_task_and_detour() {
    let mut s = session_on_task();
    ev(&mut s, "prompt", json!({"chars": 1}), 0);
    open_detour(&mut s, "D1", "I1", "task", "1.1", 10);
    ev(&mut s, "unit_stopped", json!({}), 100);
    unit(&mut s, 200, 260);
    ev(
        &mut s,
        "detour_closed",
        json!({"detour_id": "D1", "outcome": "done"}),
        270,
    );
    ev(&mut s, "item_resumed", json!({"item_id": "I1"}), 270);
    unit(&mut s, 300, 330);

    assert_eq!(task_time(s.conn(), "r", "1.1").unwrap(), Some((190, 3)));
    assert_eq!(detour_secs(&s, "D1"), 160);
    let us = units(s.conn(), "s").unwrap();
    let ids: Vec<Option<&str>> = us.iter().map(|u| u.detour_id.as_deref()).collect();
    assert_eq!(ids, vec![Some("D1"), Some("D1"), None]);
    let d = super::get_detour(s.conn(), "D1").unwrap().unwrap();
    assert_eq!(d.no, 1);
    assert_eq!(d.opened_unit, Some(1));
    assert_eq!(d.outcome.as_deref(), Some("done"));
    assert_eq!(d.closed_at, Some(at(270).to_string()));
    assert_eq!(d.blocks_task.as_deref(), Some("1.1"));
    assert_eq!(detours_for_task(s.conn(), "r", "1.1").unwrap().len(), 1);
    let f = focus(s.conn(), "s").unwrap();
    assert_eq!((f.task.as_deref(), f.detour), (Some("1.1"), None));
    assert_eq!(suspended(s.conn(), "s").unwrap(), vec![]);
}

#[test]
fn nested_detours_resume_any_order() {
    let mut s = session_on_task();
    // Unit 1: D1 suspends the task.
    ev(&mut s, "prompt", json!({"chars": 1}), 0);
    open_detour(&mut s, "D1", "I1", "task", "1.1", 5);
    ev(&mut s, "unit_stopped", json!({}), 100);
    // Unit 2: D2 inside D1 suspends D1.
    ev(&mut s, "prompt", json!({"chars": 1}), 200);
    open_detour(&mut s, "D2", "I2", "detour", "D1", 205);
    let f = focus(s.conn(), "s").unwrap();
    assert_eq!(f.detour.unwrap().detour_id, "D2");
    let d2 = super::get_detour(s.conn(), "D2").unwrap().unwrap();
    assert_eq!((d2.no, d2.parent_detour.as_deref()), (2, Some("D1")));
    let items: Vec<(String, ItemKind)> = suspended(s.conn(), "s")
        .unwrap()
        .into_iter()
        .map(|i| (i.ref_id, i.kind))
        .collect();
    assert_eq!(
        items,
        vec![
            ("1.1".to_string(), ItemKind::Task),
            ("D1".to_string(), ItemKind::Detour)
        ]
    );
    ev(&mut s, "unit_stopped", json!({}), 300);
    // Unit 3: straight back to the task, both detours still open.
    ev(&mut s, "prompt", json!({"chars": 1}), 400);
    ev(&mut s, "item_resumed", json!({"item_id": "I1"}), 405);
    let f = focus(s.conn(), "s").unwrap();
    assert_eq!((f.task.as_deref(), f.detour), (Some("1.1"), None));
    ev(&mut s, "unit_stopped", json!({}), 500);
    // Close the outer detour first: focus stays on the task; its suspended item goes away.
    ev(
        &mut s,
        "detour_closed",
        json!({"detour_id": "D1", "outcome": "done"}),
        510,
    );
    let f = focus(s.conn(), "s").unwrap();
    assert_eq!((f.task.as_deref(), f.detour), (Some("1.1"), None));
    assert_eq!(suspended(s.conn(), "s").unwrap(), vec![]);
    let open: Vec<String> = open_detours(s.conn(), "s")
        .unwrap()
        .into_iter()
        .map(|d| d.detour_id)
        .collect();
    assert_eq!(open, vec!["D2"]);
    // Unit 4 is on the task; closing D2 (not the focus) changes nothing else.
    ev(&mut s, "prompt", json!({"chars": 1}), 600);
    ev(
        &mut s,
        "detour_closed",
        json!({"detour_id": "D2", "outcome": "abandoned"}),
        650,
    );
    ev(&mut s, "unit_stopped", json!({}), 700);

    assert_eq!(detour_secs(&s, "D1"), 100);
    // Unit 3 started inside D2, so D2 was open during it.
    assert_eq!(detour_secs(&s, "D2"), 200);
    assert_eq!(task_time(s.conn(), "r", "1.1").unwrap(), Some((400, 4)));
    assert_eq!(open_detours(s.conn(), "s").unwrap(), vec![]);
    let f = focus(s.conn(), "s").unwrap();
    assert_eq!((f.task.as_deref(), f.detour), (Some("1.1"), None));
}

#[test]
fn closing_the_focused_inner_detour_returns_to_the_outer_one() {
    let mut s = session_on_task();
    ev(&mut s, "prompt", json!({"chars": 1}), 0);
    open_detour(&mut s, "D1", "I1", "task", "1.1", 5);
    open_detour(&mut s, "D2", "I2", "detour", "D1", 10);
    ev(
        &mut s,
        "detour_closed",
        json!({"detour_id": "D2", "outcome": "done"}),
        20,
    );
    ev(&mut s, "item_resumed", json!({"item_id": "I2"}), 20);
    assert_eq!(
        focus(s.conn(), "s").unwrap().detour.unwrap().detour_id,
        "D1"
    );
    // Resuming a detour item mid-unit makes it the unit's detour.
    ev(&mut s, "unit_stopped", json!({}), 60);
    assert_eq!(detour_secs(&s, "D1"), 60);
    assert_eq!(detour_secs(&s, "D2"), 0);
}

fn label(s: &mut Store, unit_no: i64, label: &str, source: &str, secs: i64) {
    ev(
        s,
        "unit_labeled",
        json!({"unit_no": unit_no, "label": label, "source": source}),
        secs,
    );
}

#[test]
fn labels_drive_away_time() {
    let mut s = session_on_task();
    unit(&mut s, 0, 60);
    label(&mut s, 1, "detour", "declared", 60);
    unit(&mut s, 100, 220);
    label(&mut s, 2, "drift", "heuristic", 220);
    let sess = session::get(s.conn(), "s").unwrap().unwrap();
    assert_eq!(sess.away_secs, 180);
    assert_eq!(sess.away_since, Some(at(0).to_string()));
    ev(
        &mut s,
        "owner_action",
        json!({"action": "drift", "answer": "continue", "target": "s"}),
        230,
    );
    assert_eq!(
        session::get(s.conn(), "s").unwrap().unwrap().ack_away_secs,
        180
    );
    unit(&mut s, 300, 330);
    label(&mut s, 3, "on_task", "heuristic", 330);
    let sess = session::get(s.conn(), "s").unwrap().unwrap();
    assert_eq!(
        (sess.away_secs, sess.away_since, sess.ack_away_secs),
        (0, None, 0)
    );
    let labels: Vec<(Option<Label>, Option<LabelSource>)> = units(s.conn(), "s")
        .unwrap()
        .into_iter()
        .map(|u| (u.label, u.label_source))
        .collect();
    assert_eq!(
        labels,
        vec![
            (Some(Label::Detour), Some(LabelSource::Declared)),
            (Some(Label::Drift), Some(LabelSource::Heuristic)),
            (Some(Label::OnTask), Some(LabelSource::Heuristic)),
        ]
    );
    // A correction of an earlier unit recomputes the run: unit 3 is still on-task.
    label(&mut s, 2, "on_task", "owner", 400);
    assert_eq!(session::get(s.conn(), "s").unwrap().unwrap().away_secs, 0);
}

fn samples(s: &Store) -> Vec<(String, f64, String, String)> {
    let mut stmt = s
        .conn()
        .prepare("SELECT unit_ref, score, label, source FROM drift_samples ORDER BY rowid")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn disagreement_creates_a_drift_sample() {
    let mut s = session_on_task();
    ev(&mut s, "prompt", json!({"chars": 1}), 0);
    ev(&mut s, "unit_stopped", json!({"score": 0.9}), 60);
    ev(
        &mut s,
        "drift_flagged",
        json!({"unit_no": 1, "score": 0.9}),
        60,
    );
    ev(
        &mut s,
        "unit_labeled",
        json!({"unit_no": 1, "label": "on_task", "source": "agent", "heuristic": "drift",
               "why": "the build was broken"}),
        70,
    );
    let u = &units(s.conn(), "s").unwrap()[0];
    assert_eq!(u.label, Some(Label::OnTask));
    assert_eq!(u.label_source, Some(LabelSource::Agent));
    assert!(u.disagreement && u.flagged);
    assert_eq!(u.drift_score, Some(0.9));
    assert_eq!(
        samples(&s),
        vec![("s:1".into(), 0.9, "on_task".into(), "agent".into())]
    );

    // The heuristic's own labels and agreeing agent labels add no sample.
    unit(&mut s, 100, 160);
    ev(
        &mut s,
        "unit_labeled",
        json!({"unit_no": 2, "label": "on_task", "source": "heuristic", "score": 0.1,
               "heuristic": "on_task"}),
        160,
    );
    ev(
        &mut s,
        "unit_labeled",
        json!({"unit_no": 2, "label": "on_task", "source": "agent", "heuristic": "on_task"}),
        170,
    );
    assert_eq!(samples(&s).len(), 1);
    assert!(!units(s.conn(), "s").unwrap()[1].disagreement);
    // An owner label always adds one.
    label(&mut s, 2, "drift", "owner", 180);
    assert_eq!(
        samples(&s)[1],
        ("s:2".into(), 0.1, "drift".into(), "owner".into())
    );
}

#[test]
fn nav_state_notes_and_tuning_project() {
    let mut s = session_on_task();
    ev(
        &mut s,
        "nav_state_changed",
        json!({"from": "green", "to": "amber", "away_secs": 1000}),
        10,
    );
    assert_eq!(
        session::get(s.conn(), "s").unwrap().unwrap().nav_state,
        NavState::Amber
    );
    ev(
        &mut s,
        "note_sent",
        json!({"unit_no": 1, "kind": "tolerance_warning", "channel": "post_tool_use",
               "state_key": "tol:amber:none"}),
        11,
    );
    let n: i64 = s
        .conn()
        .query_row("SELECT count(*) FROM notes_sent", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    ev(
        &mut s,
        "config_changed",
        json!({"scope": "repo", "key": "drift_lower", "old": 0.3, "new": 0.35, "samples": 20}),
        12,
    );
    let row: (Option<f64>, Option<f64>, i64) = s
        .conn()
        .query_row(
            "SELECT drift_lower, drift_upper, samples_at_last_run FROM repo_tuning WHERE repo_id = 'r'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, (Some(0.35), None, 20));
}
