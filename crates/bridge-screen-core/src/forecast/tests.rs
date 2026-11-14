use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::store::{Actor, EventDraft, Store, append_in_at};

const DAY: i64 = 86_400;

fn t0() -> Timestamp {
    "2026-10-14T09:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0() + SignedDuration::from_secs(secs)
}

fn utc() -> TimeZone {
    TimeZone::UTC
}

fn script(s: &mut Store, events: &[(&str, i64, &str, Value)]) {
    s.write(|tx| {
        for (session, secs, kind, data) in events {
            let draft = EventDraft::new("r", *session, *kind, data.clone());
            append_in_at(tx, draft, at(*secs))?;
        }
        Ok(())
    })
    .unwrap();
}

fn node(id: &str, parent: &str, estimate: Option<u32>) -> Value {
    json!({"id": id, "parent_id": parent, "kind": "task", "title": format!("T{id}"),
           "estimate_min": estimate, "expected_files": []})
}

fn plan(s: &mut Store, tasks: &[(&str, Option<u32>)]) {
    let mut events = vec![
        (
            "owner",
            0,
            "node_added",
            json!({"id": "G", "parent_id": null, "kind": "goal", "title": "Goal",
                   "intent": "why", "expected_files": []}),
        ),
        (
            "owner",
            0,
            "node_added",
            json!({"id": "1", "parent_id": "G", "kind": "milestone", "title": "M",
                   "intent": "why", "expected_files": []}),
        ),
    ];
    for (id, est) in tasks {
        events.push(("owner", 0, "node_added", node(id, "1", *est)));
    }
    script(s, &events);
}

/// Session `s<task>` works on `task` for `minutes` from `start` (seconds after t0), then
/// it is verified at `level`.
fn work(s: &mut Store, task: &str, start: i64, minutes: i64, level: i64) {
    let sid = format!("s{task}");
    let sid = sid.as_str();
    let end = start + minutes * 60;
    let v = json!({"verif_id": format!("v{task}"), "task_id": task, "level": level,
                   "sha": "abc", "files": [], "evidence": "ok", "by": "agent"});
    let mut events = vec![
        (
            sid,
            start,
            "session_started",
            json!({"cwd": "/w", "source": "startup", "model": "claude-opus-5-5"}),
        ),
        (
            sid,
            start,
            "task_claimed",
            json!({"task_id": task, "mode": "auto", "confidence": 0.9}),
        ),
        (sid, start, "prompt", json!({"chars": 1})),
        (sid, end, "unit_stopped", json!({"unit_no": 1})),
    ];
    if level > 0 {
        events.push((sid, end + 1, "verified", v));
    }
    script(s, &events);
}

#[test]
fn calibration_uses_leaves_verified_at_l2_in_the_window() {
    let mut s = Store::open_in_memory().unwrap();
    plan(
        &mut s,
        &[
            ("1.1", Some(30)),
            ("1.2", Some(10)),
            ("1.3", Some(20)),
            ("1.4", None),
            ("1.5", Some(10)),
            ("1.6", Some(60)),
        ],
    );
    let now = at(200 * DAY);
    work(&mut s, "1.1", 199 * DAY, 60, 2); // ratio 2.0
    work(&mut s, "1.2", 199 * DAY, 10, 1); // only L1
    work(&mut s, "1.3", 0, 40, 3); // done 200 days ago: outside the window
    work(&mut s, "1.4", 199 * DAY, 30, 2); // no estimate
    script(
        &mut s,
        &[(
            "owner",
            199 * DAY,
            "verified",
            json!({"verif_id": "v15", "task_id": "1.5", "level": 2, "sha": "a",
                   "files": [], "evidence": "ok", "by": "owner"}),
        )],
    ); // no active time
    work(&mut s, "1.6", 199 * DAY + 100, 30, 4); // ratio 0.5
    // 1.1's replies: more Haiku output than Opus.
    script(
        &mut s,
        &[(
            "s1.1",
            199 * DAY + 10,
            "usage",
            json!({"path": "/t", "offset_after": 1, "replies": [
                {"message_id": "a", "model": "claude-haiku-4-5", "ts": at(199 * DAY + 5).to_string(),
                 "output": 100},
                {"message_id": "b", "model": "claude-opus-5-5", "ts": at(199 * DAY + 6).to_string(),
                 "output": 50}]}),
        )],
    );
    let cal = calibration(s.conn(), "r", now).unwrap();
    let got: Vec<(&str, f64, &str)> = cal
        .pooled
        .iter()
        .map(|p| (p.task_id.as_str(), p.ratio, p.model.as_str()))
        .collect();
    assert_eq!(
        got,
        // Oldest first: 1.6 finished before 1.1.
        [
            ("1.6", 0.5, "claude-opus-5-5"),
            ("1.1", 2.0, "claude-haiku-4-5")
        ]
    );
    assert_eq!(cal.by_model["claude-haiku-4-5"].len(), 1);
    assert_eq!(cal.pooled[1].actual_min, 60.0);
}

/// Three finished tasks (ratios 1, 2, 3) on day 0; 1.4 (60 min) and 1.5 (no estimate) left.
fn three_done(s: &mut Store) {
    plan(
        s,
        &[
            ("1.1", Some(10)),
            ("1.2", Some(10)),
            ("1.3", Some(10)),
            ("1.4", Some(60)),
            ("1.5", None),
        ],
    );
    work(s, "1.1", 0, 10, 2);
    work(s, "1.2", 1000, 20, 2);
    work(s, "1.3", 3000, 30, 2);
}

#[test]
fn forecast_refuses_thin_data_and_is_deterministic() {
    let mut s = Store::open_in_memory().unwrap();
    plan(
        &mut s,
        &[
            ("1.1", Some(10)),
            ("1.2", Some(10)),
            ("1.3", Some(60)),
            ("1.4", None),
        ],
    );
    work(&mut s, "1.1", 0, 10, 2);
    work(&mut s, "1.2", 1000, 20, 2);
    let now = at(DAY / 2);
    let f = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!(
        f.reason.as_deref(),
        Some("Not enough finished tasks to forecast yet — 1 more is needed")
    );
    assert_eq!((&f.p50_date, &f.p90_date), (&None, &None));
    assert_eq!((f.points, f.remaining_min, f.unestimated), (2, 60, 1));
    assert_eq!(f.dates(), None);

    let mut s = Store::open_in_memory().unwrap();
    three_done(&mut s);
    let f = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!(f.reason, None);
    assert_eq!((f.points, f.remaining_min, f.unestimated), (3, 60, 1));
    assert!(f.p50_hours <= f.p90_hours);
    assert!(f.dates().is_some());
    assert_eq!(f.median_ratio, Some(2.0));
    // The same inputs give the same forecast.
    assert_eq!(run(s.conn(), "r", now, &utc()).unwrap(), f);
    // The draws come from the ratios: 60 × {1, 2, 3} minutes.
    for h in [f.p50_hours, f.p90_hours] {
        assert!([1.0, 2.0, 3.0].contains(&h), "{h}");
    }
    // Weeks later, nothing happened lately: hours but no dates.
    let late = run(s.conn(), "r", at(30 * DAY), &utc()).unwrap();
    assert_eq!(
        late.reason.as_deref(),
        Some("No recent activity to turn hours into dates")
    );
    assert_eq!((late.p50_date, late.hours_per_day), (None, None));
}

#[test]
fn p50_never_exceeds_p90_over_many_shapes() {
    let mut s = Store::open_in_memory().unwrap();
    three_done(&mut s);
    for runs in [1, 2, 3, 10, 999] {
        let f = run_with(s.conn(), "r", at(DAY / 2), &utc(), runs).unwrap();
        assert!(f.p50_hours <= f.p90_hours, "{runs}: {f:?}");
    }
}

#[test]
fn exact_estimates_give_the_sum_and_spent_time_is_subtracted() {
    let mut s = Store::open_in_memory().unwrap();
    plan(
        &mut s,
        &[
            ("1.1", Some(10)),
            ("1.2", Some(20)),
            ("1.3", Some(30)),
            ("1.4", Some(60)),
            ("1.5", Some(120)),
        ],
    );
    work(&mut s, "1.1", 0, 10, 2);
    work(&mut s, "1.2", 1000, 20, 2);
    work(&mut s, "1.3", 3000, 30, 2);
    let now = at(DAY / 2);
    let f = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!((f.p50_hours, f.p90_hours), (3.0, 3.0));
    // 30 minutes already spent on 1.4: 30 + 120 left.
    work(&mut s, "1.4", 6000, 30, 0);
    let f = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!((f.p50_hours, f.p90_hours), (2.5, 2.5));
    // More spent than the estimate never goes below zero.
    work(&mut s, "1.5", 9000, 200, 0);
    let f = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!((f.p50_hours, f.p90_hours), (0.5, 0.5));
}

#[test]
fn hours_per_day_and_dates() {
    let mut s = Store::open_in_memory().unwrap();
    plan(&mut s, &[("1.1", Some(10)), ("1.2", Some(10))]);
    // Day 0: 3 h; day 2: 1 h; day 20 before "now" is outside the 14 days.
    work(&mut s, "1.1", 0, 180, 0);
    work(&mut s, "1.2", 2 * DAY, 60, 0);
    let now = at(3 * DAY);
    assert_eq!(
        hours_per_day(s.conn(), "r", now, &utc()).unwrap(),
        Some(2.0)
    );
    assert_eq!(
        hours_per_day(s.conn(), "r", at(30 * DAY), &utc()).unwrap(),
        None
    );
    // 2026-10-17 + ceil(5 h / 2 h a day) = 3 days.
    assert_eq!(
        date_after(now, &utc(), 5.0, 2.0).as_deref(),
        Some("2026-10-20")
    );
    assert_eq!(
        date_after(now, &utc(), 0.0, 2.0).as_deref(),
        Some("2026-10-17")
    );
    assert_eq!(
        date_after(now, &utc(), 4.0, 2.0).as_deref(),
        Some("2026-10-19")
    );
    assert_eq!(date_after(now, &utc(), 4.0, 0.0), None);
    // The day is the owner's: 23:30 in UTC is the next day in Tokyo.
    let late: Timestamp = "2026-10-17T23:30:00Z".parse().unwrap();
    let tokyo = TimeZone::get("Asia/Tokyo").unwrap();
    assert_eq!(
        date_after(late, &tokyo, 0.0, 1.0).as_deref(),
        Some("2026-10-18")
    );
}

#[test]
fn history_keeps_only_forecasts_that_moved() {
    let mut s = Store::open_in_memory().unwrap();
    three_done(&mut s);
    let now = at(DAY / 2);
    assert!(record(&mut s, "r", "owner", now, &utc()).unwrap());
    // Nothing changed: not recorded again.
    assert!(!record(&mut s, "r", "owner", now, &utc()).unwrap());
    let h = history(s.conn(), "r", 1).unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].forecast.points, 3);
    let base = h[0].forecast.clone();
    let tweak = |p50: f64| Forecast {
        p50_hours: p50,
        ..base.clone()
    };
    // Under 1% is noise; 1% or more, or a new reason, is kept.
    assert!(!moved(&base, &tweak(base.p50_hours * 1.005)));
    assert!(moved(&base, &tweak(base.p50_hours * 1.02)));
    let refused = Forecast {
        reason: Some("x".into()),
        ..base.clone()
    };
    assert!(moved(&base, &refused));
    // New work estimated: the forecast moves and is kept.
    script(
        &mut s,
        &[("owner", 100, "node_added", node("1.9", "1", Some(600)))],
    );
    assert!(record(&mut s, "r", "owner", now, &utc()).unwrap());
    assert_eq!(history(s.conn(), "r", 1).unwrap().len(), 2);
    assert_eq!(
        latest(s.conn(), "r", 1).unwrap().unwrap().remaining_min,
        660
    );
}

#[test]
fn words_for_the_forecast() {
    let now: Timestamp = "2026-10-21T12:00:00Z".parse().unwrap();
    let f = Forecast {
        p50_hours: 14.0,
        p90_hours: 30.5,
        p50_date: Some("2026-10-30".into()),
        p90_date: Some("2027-01-08".into()),
        remaining_min: 600,
        unestimated: 2,
        points: 12,
        hours_per_day: Some(2.0),
        reason: None,
        median_ratio: Some(1.83),
    };
    assert_eq!(
        range_text(&f, now, &utc()),
        "likely by Oct 30 · could take until Jan 8, 2027"
    );
    assert_eq!(
        compact_range(&f, now, &utc()).as_deref(),
        Some("Oct 30–Jan 8, 2027")
    );
    assert_eq!(
        basis_text(&f),
        "based on 12 finished tasks; tasks typically take 1.8× their estimate"
    );
    assert_eq!(
        (hours_text(14.0), hours_text(2.56), hours_text(0.66)),
        ("14h".into(), "2.5h".into(), "39m".into())
    );
    let refused = Forecast {
        reason: Some("No recent activity to turn hours into dates".into()),
        ..f
    };
    assert_eq!(
        range_text(&refused, now, &utc()),
        "No recent activity to turn hours into dates"
    );
    assert_eq!(compact_range(&refused, now, &utc()), None);
}

#[test]
fn one_date_reads_as_one_date() {
    let now: Timestamp = "2026-10-21T12:00:00Z".parse().unwrap();
    let f = Forecast {
        p50_hours: 1.0,
        p90_hours: 1.2,
        p50_date: Some("2026-10-25".into()),
        p90_date: Some("2026-10-25".into()),
        remaining_min: 60,
        unestimated: 0,
        points: 4,
        hours_per_day: Some(2.0),
        reason: None,
        median_ratio: None,
    };
    assert_eq!(
        range_text(&f, now, &utc()),
        "likely by Oct 25 · 90% chance by then too"
    );
    assert_eq!(compact_range(&f, now, &utc()).as_deref(), Some("Oct 25"));
}

/// The basis line and the calibration page give the same figure for "how much longer
/// than estimated tasks take": the median, so one long task doesn't skew it.
#[test]
fn the_basis_uses_the_same_ratio_as_the_calibration_page() {
    let mut s = Store::open_in_memory().unwrap();
    plan(
        &mut s,
        &[
            ("1.1", Some(10)),
            ("1.2", Some(10)),
            ("1.3", Some(10)),
            ("1.4", Some(60)),
        ],
    );
    work(&mut s, "1.1", 0, 10, 2);
    work(&mut s, "1.2", 1000, 10, 2);
    work(&mut s, "1.3", 3000, 40, 2);
    let now = at(DAY / 2);
    let f = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!(f.median_ratio, Some(1.0));
    assert_eq!(
        basis_text(&f),
        "based on 3 finished tasks; tasks typically take 1× their estimate"
    );
    let drill =
        crate::view::drill::calibration(s.conn(), "r", &crate::config::Config::default(), now)
            .unwrap();
    assert_eq!(
        drill.pooled_text.as_deref(),
        Some("tasks typically take 1× their estimate")
    );
}

/// Events that change nothing the forecast uses (another session's prompt, a
/// notification) leave it exactly as it was: the dates don't flicker between refreshes.
#[test]
fn unrelated_events_leave_the_forecast_alone() {
    let mut s = Store::open_in_memory().unwrap();
    plan(
        &mut s,
        &[
            ("1.1", Some(10)),
            ("1.2", Some(10)),
            ("1.3", Some(10)),
            ("1.4", Some(17)),
            ("1.5", Some(23)),
            ("1.6", Some(31)),
            ("1.7", Some(41)),
            ("1.8", Some(53)),
        ],
    );
    work(&mut s, "1.1", 0, 7, 2);
    work(&mut s, "1.2", 1000, 19, 2);
    work(&mut s, "1.3", 3000, 33, 2);
    let now = at(DAY / 2);
    let before = run(s.conn(), "r", now, &utc()).unwrap();
    for i in 0..10 {
        script(
            &mut s,
            &[("other", 5000 + i, "notification", json!({"kind": "idle"}))],
        );
        assert_eq!(run(s.conn(), "r", now, &utc()).unwrap(), before, "{i}");
    }
}

#[test]
fn closing_a_run_keeps_its_calibration_but_not_its_tasks() {
    let mut s = Store::open_in_memory().unwrap();
    plan(
        &mut s,
        &[("1.1", Some(30)), ("1.2", Some(30)), ("1.3", Some(30))],
    );
    let now = at(10 * DAY);
    for (i, t) in ["1.1", "1.2", "1.3"].iter().enumerate() {
        work(&mut s, t, 9 * DAY + i64::try_from(i).unwrap() * 3600, 30, 2);
    }
    let before = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!(before.points, 3);
    s.write(|tx| crate::plan::runs::close_in(tx, "r", 1, "owner", &Actor::default()))
        .unwrap();
    // A new run with one open task: the forecast covers only it, on the old calibration.
    script(
        &mut s,
        &[
            (
                "owner",
                10 * DAY,
                "node_added",
                json!({"id": "2", "parent_id": "G", "kind": "milestone", "title": "M2",
                       "intent": "why", "expected_files": []}),
            ),
            ("owner", 10 * DAY, "node_added", node("2.1", "2", Some(40))),
        ],
    );
    let after = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!(after.points, 3, "calibration history is kept across runs");
    assert_eq!(after.remaining_min, 40, "closed-run tasks are not forecast");
}

// ---- tracks (spec 2026-11-14 §2) ----

/// Track 1 (`G`, milestone 1, tasks 1.1–1.3 done at L2 and 1.4 still open) and track 2
/// (`T2-G`, `T2-1`, tasks `T2-1.1` open, `T2-1.2` open) as of `now`.
fn two_tracks(worked_on_track_two: bool) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    plan(
        &mut s,
        &[
            ("1.1", Some(30)),
            ("1.2", Some(30)),
            ("1.3", Some(30)),
            ("1.4", Some(60)),
        ],
    );
    script(
        &mut s,
        &[
            (
                "owner",
                0,
                "node_added",
                json!({"id": "T2-G", "parent_id": null, "kind": "goal", "title": "Docs",
                       "intent": "why", "expected_files": []}),
            ),
            (
                "owner",
                0,
                "node_added",
                json!({"id": "T2-1", "parent_id": "T2-G", "kind": "milestone", "title": "Pages",
                       "intent": "why", "expected_files": []}),
            ),
            ("owner", 0, "node_added", node("T2-1.1", "T2-1", Some(60))),
        ],
    );
    work(&mut s, "1.1", DAY, 30, 2);
    work(&mut s, "1.2", DAY, 30, 2);
    work(&mut s, "1.3", DAY, 30, 2);
    if worked_on_track_two {
        // Track 2 had a long day on a different day than track 1's work.
        work(&mut s, "T2-1.1", 2 * DAY, 120, 0);
    }
    s
}

#[test]
fn forecast_per_track_uses_own_pace() {
    let s = two_tracks(true);
    let now = at(3 * DAY);
    let one = run_track(s.conn(), "r", 1, now, &utc()).unwrap();
    let two = run_track(s.conn(), "r", 2, now, &utc()).unwrap();
    // Each counts only its own remaining work.
    assert_eq!((one.remaining_min, two.remaining_min), (60, 60));
    // Track 1 worked one 1.5-hour day; track 2 one 2-hour day: each at its own pace.
    assert!((one.hours_per_day.unwrap() - 1.5).abs() < 1e-9, "{one:?}");
    assert!((two.hours_per_day.unwrap() - 2.0).abs() < 1e-9, "{two:?}");
    // Calibration stays repo-wide: both are based on the same three finished tasks.
    assert_eq!((one.points, two.points), (3, 3));
    // The pooled forecast counts both and the repo's pace.
    let all = run(s.conn(), "r", now, &utc()).unwrap();
    assert_eq!(all.remaining_min, 120);
}

#[test]
fn forecast_refuses_track_without_recent_activity() {
    let s = two_tracks(false);
    let now = at(3 * DAY);
    let one = run_track(s.conn(), "r", 1, now, &utc()).unwrap();
    let two = run_track(s.conn(), "r", 2, now, &utc()).unwrap();
    assert!(one.dates().is_some(), "{one:?}");
    // Nobody worked on track 2: no date, and it says why.
    assert!(two.dates().is_none());
    assert_eq!(
        two.reason.as_deref(),
        Some("No recent activity to turn hours into dates")
    );
}

#[test]
fn forecast_history_has_track() {
    let mut s = two_tracks(true);
    let now = at(3 * DAY);
    assert!(record(&mut s, "r", "owner", now, &utc()).unwrap());
    assert_eq!(history(s.conn(), "r", 1).unwrap().len(), 1);
    assert_eq!(history(s.conn(), "r", 2).unwrap().len(), 1);
    assert_eq!(latest(s.conn(), "r", 2).unwrap().unwrap().remaining_min, 60);
    // Track 1's event is written the old way; track 2's names its track.
    let kinds: Vec<Value> = crate::store::read_all(s.conn(), Some("r"))
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "forecast_ran")
        .map(|e| e.data)
        .collect();
    assert_eq!(kinds.len(), 2);
    assert!(kinds[0].get("track").is_none());
    assert_eq!(kinds[1]["track"], 2);
    // Replaying rebuilds the same history.
    let before = (
        history(s.conn(), "r", 1).unwrap(),
        history(s.conn(), "r", 2).unwrap(),
    );
    s.rebuild().unwrap();
    assert_eq!(
        before,
        (
            history(s.conn(), "r", 1).unwrap(),
            history(s.conn(), "r", 2).unwrap()
        )
    );
}

#[test]
fn reviewed_tasks_teach_calibration() {
    let mut s = Store::open_in_memory().unwrap();
    plan(&mut s, &[("1.1", Some(30)), ("1.2", Some(30))]);
    script(
        &mut s,
        &[(
            "owner",
            1,
            "node_edited",
            json!({"id": "1.1", "check": "review", "reason": "research"}),
        )],
    );
    work(&mut s, "1.1", DAY, 60, 0);
    script(
        &mut s,
        &[(
            "s1.1",
            DAY + 3601,
            "claimed_done",
            json!({"task_id": "1.1", "evidence":
                "Read the three vendor reports and compared pricing, limits and support terms in a table."}),
        )],
    );
    let cal = calibration(s.conn(), "r", at(2 * DAY)).unwrap();
    assert_eq!(cal.pooled.len(), 1);
    assert_eq!(cal.pooled[0].task_id, "1.1");
    assert!((cal.pooled[0].ratio - 2.0).abs() < 1e-9);
}
