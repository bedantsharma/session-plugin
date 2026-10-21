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

mod tolerance {
    use super::{at, ev, open_detour};
    use crate::config::Config;
    use crate::navigator::{Channel, nav_step};
    use crate::plan::{EditContext, NodeKind, PlanOp, plan_edit};
    use crate::repo::RepoInfo;
    use crate::session;
    use crate::store::{Actor, Store, append_in};
    use crate::tangent::{Label, LabelSource, NavState, Thresholds, evaluate, tolerance_secs};
    use serde_json::json;

    fn th() -> Thresholds {
        Thresholds::from_config(&Config::default())
    }

    /// Repo `r` with G → 1.1 Login form (100 min) and 1.2 Reset (no estimate); session `s`.
    fn planned(task: Option<&str>) -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.register_repo(&RepoInfo {
            id: "r".into(),
            name: "shop".into(),
            root: "/r".into(),
        })
        .unwrap();
        let add = |parent: Option<&str>, kind, title: &str, est| PlanOp::AddNode {
            parent: parent.map(str::to_string),
            kind,
            title: title.into(),
            intent: kind.requires_intent().then(|| "why".to_string()),
            estimate_min: est,
            expected_files: vec![],
        };
        plan_edit(
            &mut s,
            &EditContext {
                repo_id: "r".into(),
                session_id: "owner".into(),
                actor: Actor::default(),
                reason: None,
                replan: false,
            },
            &[
                add(None, NodeKind::Goal, "Ship", None),
                add(Some("G"), NodeKind::Milestone, "Accounts", None),
                add(Some("1"), NodeKind::Task, "Login form", Some(100)),
                add(Some("1"), NodeKind::Task, "Reset", None),
            ],
        )
        .unwrap();
        ev(
            &mut s,
            "session_started",
            json!({"cwd": "/r", "source": "startup"}),
            0,
        );
        if let Some(t) = task {
            ev(
                &mut s,
                "task_claimed",
                json!({"task_id": t, "mode": "auto", "confidence": 0.9}),
                0,
            );
        }
        s
    }

    fn eval(s: &Store, now: i64) -> (NavState, i64) {
        let sess = session::get(s.conn(), "s").unwrap().unwrap();
        let e = evaluate(s.conn(), &sess, &th(), at(now)).unwrap();
        (e.state, e.away_secs)
    }

    fn detour_unit(s: &mut Store, unit_no: i64, from: i64, to: i64) {
        ev(s, "prompt", json!({"chars": 1}), from);
        ev(s, "unit_stopped", json!({}), to);
        ev(
            s,
            "unit_labeled",
            json!({"unit_no": unit_no, "label": "detour", "source": "declared"}),
            to,
        );
    }

    #[test]
    fn tolerance_without_estimate_or_task() {
        assert_eq!(tolerance_secs(&th(), None), 900);
        assert_eq!(tolerance_secs(&th(), Some(30)), 900);
        assert_eq!(tolerance_secs(&th(), Some(100)), 1200);

        // A session that never claimed a task still counts away time.
        let mut s = planned(None);
        ev(
            &mut s,
            "detour_opened",
            json!({"detour_id": "D1", "reason": "setup", "blocks": null, "resume_cue": "x"}),
            0,
        );
        detour_unit(&mut s, 1, 0, 600);
        assert_eq!(eval(&s, 700), (NavState::Green, 600));
        detour_unit(&mut s, 2, 700, 1100);
        assert_eq!(eval(&s, 1200), (NavState::Amber, 1000));

        // A task without an estimate uses the 15-minute floor.
        let mut s = planned(Some("1.2"));
        open_detour(&mut s, "D1", "I1", "task", "1.2", 0);
        detour_unit(&mut s, 1, 0, 1900);
        assert_eq!(eval(&s, 2000), (NavState::Red, 1900));
    }

    #[test]
    fn transitions_with_live_time_and_waits() {
        // Tolerance for 1.1: max(15 min, 20% of 100 min) = 20 min.
        let mut s = planned(Some("1.1"));
        open_detour(&mut s, "D1", "I1", "task", "1.1", 0);
        detour_unit(&mut s, 1, 0, 1500);
        assert_eq!(eval(&s, 1600), (NavState::Amber, 1500));
        // A unit still open inside the detour counts live, minus waits on the owner.
        ev(&mut s, "prompt", json!({"chars": 1}), 2000);
        ev(
            &mut s,
            "needs_owner",
            json!({"kind": "permission_prompt"}),
            2100,
        );
        ev(
            &mut s,
            "tool_intent",
            json!({"tool": "Bash", "files": []}),
            2400,
        );
        assert_eq!(eval(&s, 3000), (NavState::Amber, 2200));
        assert_eq!(eval(&s, 3300), (NavState::Red, 2500));
        // The owner says continue: what was seen is acknowledged.
        ev(
            &mut s,
            "owner_action",
            json!({"action": "drift", "answer": "continue", "target": "s"}),
            3300,
        );
        assert_eq!(eval(&s, 3300), (NavState::Green, 1000));
    }

    #[test]
    fn only_a_unit_labelled_drift_without_a_detour_is_red() {
        let mut s = planned(Some("1.1"));
        ev(&mut s, "prompt", json!({"chars": 1}), 0);
        ev(&mut s, "unit_stopped", json!({"score": 0.9}), 60);
        ev(
            &mut s,
            "drift_flagged",
            json!({"unit_no": 1, "score": 0.9}),
            60,
        );
        // Flagged but not yet labelled: the agent is being asked, nothing is red yet.
        assert_eq!(eval(&s, 100).0, NavState::Green);
        // The agent says on-task: the disagreement is recorded, never argued (spec §7.2).
        ev(
            &mut s,
            "unit_labeled",
            json!({"unit_no": 1, "label": "on_task", "source": "agent", "heuristic": "drift"}),
            70,
        );
        assert_eq!(eval(&s, 100).0, NavState::Green);
        // Labelled a detour by the agent: a declared reason.
        ev(
            &mut s,
            "unit_labeled",
            json!({"unit_no": 1, "label": "detour", "source": "agent", "heuristic": "drift"}),
            75,
        );
        assert_eq!(eval(&s, 100).0, NavState::Green);
        // Labelled drift (by the agent, the owner or the fallback): red until acknowledged.
        ev(
            &mut s,
            "unit_labeled",
            json!({"unit_no": 1, "label": "drift", "source": "agent", "heuristic": "drift"}),
            80,
        );
        assert_eq!(eval(&s, 100).0, NavState::Red);
        ev(
            &mut s,
            "owner_action",
            json!({"action": "drift", "answer": "continue", "target": "s"}),
            90,
        );
        assert_eq!(eval(&s, 100).0, NavState::Green);
        // A middle-band unit the agent calls drift is red too.
        ev(&mut s, "prompt", json!({"chars": 1}), 100);
        ev(&mut s, "unit_stopped", json!({"score": 0.5}), 160);
        ev(
            &mut s,
            "unit_labeled",
            json!({"unit_no": 2, "label": "drift", "source": "agent"}),
            170,
        );
        assert_eq!(eval(&s, 200).0, NavState::Red);
    }

    fn step(s: &mut Store, channel: Channel, now: i64) -> (Vec<String>, Option<String>) {
        s.write(|tx| {
            let step = nav_step(tx, "s", channel, &th(), at(now))?;
            let kinds = step.events.iter().map(|e| e.kind.clone()).collect();
            for e in step.events {
                append_in(tx, e)?;
            }
            Ok((kinds, step.note))
        })
        .unwrap()
    }

    #[test]
    fn amber_warning_once_per_state() {
        let mut s = planned(Some("1.1"));
        open_detour(&mut s, "D1", "I1", "task", "1.1", 0);
        detour_unit(&mut s, 1, 0, 1800);
        let (kinds, note) = step(&mut s, Channel::PostToolUse, 1900);
        assert_eq!(kinds, vec!["nav_state_changed", "note_sent"]);
        assert_eq!(
            note.as_deref(),
            Some(
                "bridge-screen: you've been on \"fix D1\" for 30 min (~0.3× the task estimate). Continue if it still blocks 1.1 Login form; otherwise close the detour and return."
            )
        );
        assert_eq!(
            session::get(s.conn(), "s").unwrap().unwrap().nav_state,
            NavState::Amber
        );
        // Same unit, same state: nothing.
        assert_eq!(step(&mut s, Channel::PostToolUse, 1950), (vec![], None));
        // Next unit, still the same state: rate-limited.
        ev(&mut s, "prompt", json!({"chars": 1}), 2000);
        assert_eq!(step(&mut s, Channel::PostToolUse, 2010), (vec![], None));
        // Far outside tolerance: the state changed, so the red note goes out at once
        // (unit 2 had none yet), and the owner is flagged.
        let (kinds, note) = step(&mut s, Channel::PostToolUse, 2700);
        assert_eq!(kinds, vec!["nav_state_changed", "needs_owner", "note_sent"]);
        assert_eq!(
            note.as_deref(),
            Some(
                "bridge-screen: this detour is well past tolerance; the owner has been flagged. Keep going if it's essential, or close it and return to 1.1 Login form."
            )
        );
        ev(&mut s, "unit_stopped", json!({}), 2700);
        ev(
            &mut s,
            "unit_labeled",
            json!({"unit_no": 2, "label": "detour", "source": "declared"}),
            2700,
        );
        // Still red in the next unit: not repeated.
        ev(&mut s, "prompt", json!({"chars": 1}), 2800);
        assert_eq!(step(&mut s, Channel::PostToolUse, 2810), (vec![], None));
        // Other channels carry no tolerance notes.
        ev(&mut s, "prompt", json!({"chars": 1}), 2900);
        assert_eq!(step(&mut s, Channel::PromptSubmit, 2910).1, None);
    }

    #[test]
    fn drift_pause_changes_the_red_note() {
        let mut s = planned(Some("1.1"));
        s.conn()
            .execute(
                "UPDATE repos SET settings = '{\"drift_pause\": true}' WHERE id = 'r'",
                [],
            )
            .unwrap();
        open_detour(&mut s, "D1", "I1", "task", "1.1", 0);
        detour_unit(&mut s, 1, 0, 3000);
        let (_, note) = step(&mut s, Channel::PostToolUse, 3100);
        assert_eq!(
            note.as_deref(),
            Some(
                "bridge-screen: this detour is well past tolerance; the owner has been flagged. Finish your current step, commit, then stop and ask the owner before continuing."
            )
        );
    }

    #[test]
    fn owner_reply_is_delivered_once() {
        let mut s = planned(Some("1.1"));
        ev(&mut s, "prompt", json!({"chars": 1}), 0);
        ev(
            &mut s,
            "owner_action",
            json!({"action": "drift", "answer": "stop", "target": "s"}),
            10,
        );
        let (_, note) = step(&mut s, Channel::PromptSubmit, 20);
        assert_eq!(
            note.as_deref(),
            Some("bridge-screen: the owner says stop the detour and return to 1.1 Login form.")
        );
        ev(&mut s, "prompt", json!({"chars": 1}), 30);
        assert_eq!(step(&mut s, Channel::PromptSubmit, 40).1, None);
        assert_eq!(step(&mut s, Channel::PostToolUse, 40).1, None);
    }

    /// Run the Stop labelling for unit `n` and record what it decides.
    fn stop_label(s: &mut Store, n: i64) -> (Vec<String>, Option<String>) {
        s.write(|tx| {
            let sess = session::get(tx, "s")?.unwrap();
            let unit = session::unit(tx, "s", n)?.unwrap();
            let step = crate::navigator::label_at_stop(tx, &sess, &unit, false, &th())?;
            let kinds = step.events.iter().map(|e| e.kind.clone()).collect();
            for e in step.events {
                append_in(tx, e)?;
            }
            Ok((kinds, step.note))
        })
        .unwrap()
    }

    #[test]
    fn requested_work_is_not_drift() {
        // 1.2 has no estimate: tolerance is the 15-minute floor.
        let mut s = planned(Some("1.2"));
        ev(
            &mut s,
            "detour_opened",
            json!({"detour_id": "D1", "reason": "add a licence", "blocks": "1.2",
                   "resume_cue": "back to 1.2", "requested": true}),
            0,
        );
        ev(
            &mut s,
            "item_suspended",
            json!({"item_id": "I1", "kind": "task", "ref_id": "1.2",
                   "resume_cue": "back to 1.2", "by_detour": "D1"}),
            0,
        );
        assert!(
            super::super::get_detour(s.conn(), "D1")
                .unwrap()
                .unwrap()
                .requested
        );
        // A live unit inside the asked-for detour, far past 2x tolerance: still green.
        ev(&mut s, "prompt", json!({"chars": 1}), 0);
        assert_eq!(eval(&s, 2000), (NavState::Green, 0));
        // It finishes with a drift-looking score: labelled "requested" at Stop, declared,
        // with no question to the agent.
        ev(&mut s, "unit_stopped", json!({"score": 0.95}), 2000);
        let (kinds, note) = stop_label(&mut s, 1);
        assert_eq!((kinds, note), (vec!["unit_labeled".to_string()], None));
        let u = session::unit(s.conn(), "s", 1).unwrap().unwrap();
        assert_eq!(
            (u.label, u.label_source),
            (Some(Label::Requested), Some(LabelSource::Declared))
        );
        assert_eq!(eval(&s, 2100), (NavState::Green, 0));
        assert_eq!(step(&mut s, Channel::PostToolUse, 2100), (vec![], None));
        // The agent (or owner) labelling a high-score unit "requested": no red, no sample.
        ev(
            &mut s,
            "unit_labeled",
            json!({"unit_no": 1, "label": "requested", "source": "agent", "heuristic": "drift"}),
            2100,
        );
        assert_eq!(eval(&s, 2100), (NavState::Green, 0));
        assert_eq!(super::samples(&s), vec![]);
        // Back to the task; the next unit is real drift: red.
        ev(
            &mut s,
            "detour_closed",
            json!({"detour_id": "D1", "outcome": "done"}),
            2200,
        );
        ev(&mut s, "item_resumed", json!({"item_id": "I1"}), 2200);
        ev(&mut s, "prompt", json!({"chars": 1}), 2300);
        ev(&mut s, "unit_stopped", json!({"score": 0.95}), 2400);
        ev(
            &mut s,
            "unit_labeled",
            json!({"unit_no": 2, "label": "drift", "source": "agent", "heuristic": "drift"}),
            2400,
        );
        assert_eq!(eval(&s, 2500), (NavState::Red, 100));
    }

    #[test]
    fn requested_label_parses_its_aliases() {
        for s in ["requested", "asked", "asked-for"] {
            assert_eq!(super::super::Label::parse(s), Some(Label::Requested), "{s}");
        }
        let l: Label = serde_json::from_value(json!("asked-for")).unwrap();
        assert_eq!(l, Label::Requested);
        assert_eq!(
            serde_json::to_value(Label::Requested).unwrap(),
            json!("requested")
        );
    }
}
