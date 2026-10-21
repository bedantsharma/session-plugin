//! `watch` on ratatui's TestBackend with a fake actor.

use bridge_screen_core::config::Config;
use bridge_screen_core::owner::OwnerAction;
use bridge_screen_core::repo::RepoInfo;
use bridge_screen_core::store::{EventDraft, Store, append_in_at};
use bridge_screen_core::view::{self, Glance};
use jiff::{SignedDuration, Timestamp};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use serde_json::json;

use super::{Actor, App, Guard, Target, fit, render};

fn t0() -> Timestamp {
    "2026-10-14T09:00:00Z".parse().unwrap()
}

/// A glance with `red` red sessions (each a drift item) and one idle session.
fn glance(red: usize) -> Glance {
    let mut s = Store::open_in_memory().unwrap();
    s.register_repo(&RepoInfo {
        id: "r1".into(),
        name: "shop".into(),
        root: "/code/shop".into(),
    })
    .unwrap();
    let mut ev = |session: &str, secs: i64, kind: &str, data: serde_json::Value| {
        s.write(|tx| {
            append_in_at(
                tx,
                EventDraft::new("r1", session, kind, data),
                t0() + SignedDuration::from_secs(secs),
            )?;
            Ok(())
        })
        .unwrap();
    };
    ev(
        "owner",
        0,
        "node_added",
        json!({"id": "G", "parent_id": null, "kind": "goal", "title": "Online shop",
               "intent": "buy", "estimate_min": null, "expected_files": []}),
    );
    ev(
        "owner",
        0,
        "node_added",
        json!({"id": "1", "parent_id": "G", "kind": "milestone", "title": "Accounts",
               "intent": "sign in", "estimate_min": null, "expected_files": []}),
    );
    for n in 0..red {
        let sid = format!("red{n:03}aaa");
        ev(
            &sid,
            1,
            "session_started",
            json!({"cwd": "/w", "source": "startup", "worktree": "/w"}),
        );
        ev(&sid, 2, "prompt", json!({}));
        ev(
            &sid,
            3,
            "detour_opened",
            json!({"detour_id": format!("D{n}"), "reason": "CI cache is broken on the main branch", "resume_cue": "x"}),
        );
        ev(&sid, 2402, "unit_stopped", json!({}));
        ev(
            &sid,
            2402,
            "nav_state_changed",
            json!({"from": "green", "to": "red", "away_secs": 2400}),
        );
    }
    ev(
        "idle0bbb",
        5,
        "session_started",
        json!({"cwd": "/w", "source": "startup"}),
    );
    view::glance(
        s.conn(),
        &Config::default(),
        t0() + SignedDuration::from_secs(3000),
    )
    .unwrap()
}

#[derive(Default)]
struct Fake {
    acted: Vec<OwnerAction>,
    details: Vec<Target>,
}

impl Actor for Fake {
    fn glance(&mut self) -> Option<Glance> {
        None
    }
    fn cursor(&mut self) -> Option<i64> {
        Some(1)
    }
    fn detail(&mut self, target: &Target) -> Vec<String> {
        self.details.push(target.clone());
        vec!["3 units · on task 12m".into()]
    }
    fn act(&mut self, action: &OwnerAction) -> Result<String, String> {
        self.acted.push(action.clone());
        Ok("Told red000aaa: continue.".into())
    }
}

fn draw(app: &App, w: u16, h: u16) -> (Vec<String>, Terminal<TestBackend>) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| render(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    let lines = (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf.cell((x, y)).map_or(" ", |c| c.symbol()))
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    (lines, t)
}

fn key(app: &mut App, code: KeyCode, actor: &mut Fake) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE), actor);
}

fn text(lines: &[String]) -> String {
    lines.join("\n")
}

#[test]
fn renders_the_glance_at_100x30() {
    let app = App::new(glance(1), true);
    let (lines, _) = draw(&app, 100, 30);
    let all = text(&lines);
    assert!(
        lines[0].starts_with("✱ bridge-screen · 2 sessions · 1 projects"),
        "{all}"
    );
    assert!(lines[0].contains("■ 1 need you"), "{all}");
    assert!(all.contains("Needs you"), "{all}");
    assert!(
        all.contains("■ drift     red000 · far off its task"),
        "{all}"
    );
    assert!(all.contains("shop"), "{all}");
    assert!(all.contains("0% done"), "{all}");
    assert!(all.contains("■ needs you S red000"), "{all}");
    assert!(all.contains("◆ idle      S idle0b"), "{all}");
    assert!(lines[29].contains("q quit"), "{all}");
}

#[test]
fn narrow_screens_get_one_short_line_per_item() {
    for (w, h) in [(60, 20), (40, 20)] {
        let app = App::new(glance(1), false);
        let (lines, _) = draw(&app, w, h);
        let all = text(&lines);
        assert!(all.contains("■ drift red000"), "{w}: {all}");
        assert!(all.contains("shop 0% done"), "{w}: {all}");
        assert!(all.contains("red000 needs you 40m"), "{w}: {all}");
        for l in &lines {
            assert!(l.chars().count() <= usize::from(w), "{l}");
            // Nothing is cut mid-word without an ellipsis.
            if l.chars().count() == usize::from(w) {
                assert!(
                    l.ends_with('…') || !l.ends_with(char::is_alphanumeric) || l.contains("quit"),
                    "{l}"
                );
            }
        }
    }
    assert_eq!(fit("detour 40m on the cache", 12), "detour 40m…");
}

#[test]
fn escalation_is_emphasised() {
    let app = App::new(glance(5), true);
    let (lines, t) = draw(&app, 100, 40);
    assert!(lines[0].contains("■ 5 need you"), "{}", text(&lines));
    let x = u16::try_from(
        lines[0]
            .find('■')
            .map(|b| lines[0][..b].chars().count())
            .unwrap(),
    )
    .unwrap();
    let cell = t.backend().buffer().cell((x, 0)).unwrap().clone();
    assert!(cell.modifier.contains(ratatui::style::Modifier::REVERSED));
    assert!(cell.modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn selection_moves_with_arrows_and_jk_and_enter_expands() {
    let mut app = App::new(glance(1), true);
    let mut fake = Fake::default();
    assert_eq!(app.rows().len(), 4);
    key(&mut app, KeyCode::Down, &mut fake);
    key(&mut app, KeyCode::Char('j'), &mut fake);
    key(&mut app, KeyCode::Char('j'), &mut fake);
    key(&mut app, KeyCode::Char('j'), &mut fake);
    assert_eq!(app.selected, 3, "stops at the last row");
    key(&mut app, KeyCode::Char('k'), &mut fake);
    key(&mut app, KeyCode::Up, &mut fake);
    assert_eq!(app.selected, 1);
    key(&mut app, KeyCode::Enter, &mut fake);
    assert_eq!(fake.details, vec![Target::Project("r1".into())]);
    let (lines, _) = draw(&app, 100, 30);
    assert!(text(&lines).contains("    3 units · on task 12m"));
    key(&mut app, KeyCode::Enter, &mut fake);
    assert_eq!(app.detail, None);
}

#[test]
fn help_overlay_toggles() {
    let mut app = App::new(glance(0), false);
    let mut fake = Fake::default();
    key(&mut app, KeyCode::Char('?'), &mut fake);
    let (lines, _) = draw(&app, 100, 30);
    assert!(text(&lines).contains("a: act on the selected needs-you item"));
    key(&mut app, KeyCode::Char('?'), &mut fake);
    let (lines, _) = draw(&app, 100, 30);
    assert!(!text(&lines).contains("a: act on the selected"));
}

#[test]
fn a_opens_the_picker_and_acts_through_the_actor() {
    let mut app = App::new(glance(1), true);
    let mut fake = Fake::default();
    key(&mut app, KeyCode::Char('a'), &mut fake);
    let (lines, _) = draw(&app, 100, 30);
    let all = text(&lines);
    assert!(
        all.contains("1  Continue") && all.contains("3  Stop"),
        "{all}"
    );
    // Esc cancels.
    key(&mut app, KeyCode::Esc, &mut fake);
    assert_eq!(fake.acted, vec![]);
    // Continue asks for an optional reason; Enter on empty skips it.
    key(&mut app, KeyCode::Char('a'), &mut fake);
    key(&mut app, KeyCode::Char('1'), &mut fake);
    let (lines, _) = draw(&app, 100, 30);
    assert!(text(&lines).contains("reason (optional): ▏"));
    key(&mut app, KeyCode::Enter, &mut fake);
    assert_eq!(
        serde_json::to_value(&fake.acted[0]).unwrap(),
        json!({"kind": "drift", "session": "red000aaa", "answer": "continue"})
    );
    assert_eq!(app.message.as_deref(), Some("Told red000aaa: continue."));
    // With a reason typed.
    key(&mut app, KeyCode::Char('a'), &mut fake);
    key(&mut app, KeyCode::Char('1'), &mut fake);
    for c in "fix unblocks it".chars() {
        key(&mut app, KeyCode::Char(c), &mut fake);
    }
    key(&mut app, KeyCode::Enter, &mut fake);
    assert_eq!(
        serde_json::to_value(&fake.acted[1]).unwrap()["reason"],
        "fix unblocks it"
    );
    // Make it a task: required fields can't be skipped; Esc in a prompt cancels.
    key(&mut app, KeyCode::Char('a'), &mut fake);
    key(&mut app, KeyCode::Char('2'), &mut fake);
    key(&mut app, KeyCode::Enter, &mut fake); // parent prefilled
    key(&mut app, KeyCode::Enter, &mut fake); // empty title: stays
    key(&mut app, KeyCode::Esc, &mut fake);
    assert_eq!(fake.acted.len(), 2);
}

#[test]
fn the_terminal_is_restored_after_a_panic_in_the_draw_loop() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let restored = Arc::new(AtomicBool::new(false));
    let flag = restored.clone();
    let result = std::panic::catch_unwind(move || {
        let _guard = Guard::new(move || flag.store(true, Ordering::SeqCst));
        let app = App::new(glance(1), true);
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| {
            render(f, &app);
            panic!("boom while drawing");
        })
        .unwrap();
    });
    assert!(result.is_err());
    assert!(restored.load(Ordering::SeqCst));
}

#[test]
fn without_colour_no_cell_has_a_colour() {
    let app = App::new(glance(2), false);
    let (_, t) = draw(&app, 100, 30);
    for cell in t.backend().buffer().content() {
        assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset));
    }
    let app = App::new(glance(2), true);
    let (_, t) = draw(&app, 100, 30);
    assert!(
        t.backend()
            .buffer()
            .content()
            .iter()
            .any(|c| c.fg == Color::Indexed(173) || c.fg == Color::Red)
    );
}

#[test]
fn the_picker_stays_on_its_item_when_the_queue_changes_under_it() {
    let g = glance(2);
    let mut app = App::new(g.clone(), false);
    let mut fake = Fake::default();
    let first = g.needs_you[0].session_id.clone().unwrap();
    key(&mut app, KeyCode::Char('a'), &mut fake);
    // A refresh reorders the queue while the picker is open.
    let mut reordered = g.clone();
    reordered.needs_you.reverse();
    app.set_glance(reordered);
    let _ = draw(&app, 100, 30);
    key(&mut app, KeyCode::Char('3'), &mut fake);
    assert_eq!(
        serde_json::to_value(&fake.acted[0]).unwrap()["session"],
        first.as_str()
    );
    // The item goes away while the picker is open: nothing is sent, nothing panics.
    key(&mut app, KeyCode::Char('a'), &mut fake);
    let mut gone = g.clone();
    gone.needs_you
        .retain(|i| i.session_id.as_deref() != Some(first.as_str()));
    app.set_glance(gone.clone());
    let _ = draw(&app, 100, 30);
    key(&mut app, KeyCode::Char('1'), &mut fake);
    assert_eq!(fake.acted.len(), 1);
}

#[test]
fn the_project_line_shows_the_forecast_and_cost_and_sessions_their_cost() {
    use bridge_screen_core::forecast::Forecast;
    use bridge_screen_core::view::{CostRow, ForecastRow};
    let mut g = glance(0);
    let cost = |text: &str| CostRow {
        tokens: 3_100_000,
        tokens_text: "3.1M".into(),
        usd: Some(12.4),
        unpriced_models: vec![],
        text: text.into(),
        summary: format!("{text} spent · 3.1M tokens"),
    };
    g.projects[0].forecast = Some(ForecastRow {
        forecast: Forecast {
            p50_hours: 10.0,
            p90_hours: 20.0,
            p50_date: Some("2026-10-30".into()),
            p90_date: Some("2026-11-08".into()),
            remaining_min: 600,
            unestimated: 0,
            points: 5,
            hours_per_day: Some(2.0),
            reason: None,
            median_ratio: Some(1.5),
        },
        text: "likely by Oct 30 · could take until Nov 8".into(),
        compact: Some("Oct 30–Nov 8".into()),
        short: "likely Oct 30–Nov 8".into(),
        help: "50% and 90% chance, based on how long 5 verified tasks actually took".into(),
        basis: "based on 5 finished tasks".into(),
    });
    g.projects[0].cost = Some(cost("$12.40"));
    g.projects[0].sessions[0].cost = Some(cost("$1.20"));
    let app = App::new(g, false);
    let (lines, _) = draw(&app, 140, 30);
    let all = text(&lines);
    assert!(
        all.contains("0% done · likely Oct 30–Nov 8 · $12.40"),
        "{all}"
    );
    assert!(all.contains("0m · $1.20"), "{all}");
}

/// Every owner-facing string a glance carries: titles, details, words, labels.
fn screen_strings(v: &serde_json::Value, key: &str, out: &mut Vec<String>) {
    const SHOWN: &[&str] = &[
        "title",
        "detail",
        "nav_text",
        "light_word",
        "state_label",
        "label",
        "text",
        "short",
        "summary",
        "basis",
        "compact",
        "tuned",
        "repo_name",
    ];
    match v {
        serde_json::Value::String(s) if SHOWN.contains(&key) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| screen_strings(x, key, out)),
        serde_json::Value::Object(o) => o.iter().for_each(|(k, x)| screen_strings(x, k, out)),
        _ => {}
    }
}

/// A glance with one of everything: drift red, a question, a permission wait, a detour
/// the owner asked for, an overdue check, leftovers, unsaved work, an unconfirmed
/// handoff, a done milestone and an exploring one.
#[allow(clippy::too_many_lines)] // one event per line, as data
fn busy_glance() -> Glance {
    let mut s = Store::open_in_memory().unwrap();
    s.register_repo(&RepoInfo {
        id: "r1".into(),
        name: "shop".into(),
        root: "/code/shop".into(),
    })
    .unwrap();
    let mut ev = |session: &str, secs: i64, kind: &str, data: serde_json::Value| {
        s.write(|tx| {
            append_in_at(
                tx,
                EventDraft::new("r1", session, kind, data),
                t0() + SignedDuration::from_secs(secs),
            )?;
            Ok(())
        })
        .unwrap();
    };
    for (id, parent, kind, est) in [
        ("G", None, "goal", None),
        ("1", Some("G"), "milestone", None),
        ("1.1", Some("1"), "task", Some(30)),
        ("2", Some("G"), "milestone", None),
        ("2.1", Some("2"), "task", Some(30)),
        ("2.2", Some("2"), "task", None),
    ] {
        ev(
            "owner",
            0,
            "node_added",
            json!({"id": id, "parent_id": parent, "kind": kind, "title": format!("Part {id}"),
                   "intent": "why", "estimate_min": est, "expected_files": []}),
        );
    }
    ev(
        "owner",
        0,
        "hill_moved",
        json!({"id": "2", "hill": "uphill"}),
    );
    let start = |ev: &mut dyn FnMut(&str, i64, &str, serde_json::Value), sid: &str, t: i64| {
        ev(
            sid,
            t,
            "session_started",
            json!({"cwd": "/w", "source": "startup", "worktree": "/w", "branch": "feat"}),
        );
    };
    // Drift red, far off its task.
    start(&mut ev, "red0aaaa", 1);
    ev("red0aaaa", 2, "prompt", json!({}));
    ev(
        "red0aaaa",
        3,
        "detour_opened",
        json!({"detour_id": "D1", "reason": "CI cache", "resume_cue": "x"}),
    );
    ev("red0aaaa", 2402, "unit_stopped", json!({}));
    ev(
        "red0aaaa",
        2402,
        "nav_state_changed",
        json!({"from": "green", "to": "red", "away_secs": 2400}),
    );
    // Work the owner asked for.
    start(&mut ev, "ask0bbbb", 10);
    ev(
        "ask0bbbb",
        10,
        "task_claimed",
        json!({"task_id": "2.1", "mode": "auto", "confidence": 0.9}),
    );
    ev("ask0bbbb", 11, "prompt", json!({}));
    ev(
        "ask0bbbb",
        12,
        "detour_opened",
        json!({"detour_id": "D2", "reason": "add a licence", "resume_cue": "x",
               "requested": true}),
    );
    // A question and a permission wait.
    start(&mut ev, "que0cccc", 20);
    ev("que0cccc", 21, "prompt", json!({}));
    ev("que0cccc", 80, "unit_stopped", json!({}));
    ev(
        "que0cccc",
        80,
        "owner_question",
        json!({"unit_no": 1, "source": "turn_end"}),
    );
    start(&mut ev, "per0dddd", 30);
    ev("per0dddd", 31, "prompt", json!({}));
    ev(
        "per0dddd",
        32,
        "needs_owner",
        json!({"kind": "permission_prompt"}),
    );
    // Unsaved work, a missed save, an unconfirmed handoff, an overdue check.
    start(&mut ev, "flg0eeee", 40);
    ev(
        "flg0eeee",
        41,
        "briefed",
        json!({"status": "unstable", "readback_required": true}),
    );
    ev(
        "flg0eeee",
        42,
        "checkpoint_due",
        json!({"reason": "detour_closed"}),
    );
    ev("flg0eeee", 43, "checkpoint_missed", json!({"unit_no": 1}));
    for (kind, data) in [
        ("claimed_done", json!({"task_id": "2.2", "evidence": "ok"})),
        ("verify_nudged", json!({"task_id": "2.2", "level": 0})),
        ("verify_nudged", json!({"task_id": "2.2", "level": 0})),
        ("needs_owner", json!({"kind": "verify", "task_id": "2.2"})),
    ] {
        ev("flg0eeee", 50, kind, data);
    }
    for (id, report_only) in [("l1", false), ("l2", true)] {
        ev(
            "flg0eeee",
            60,
            "leftover_found",
            json!({"leftover_id": id, "from_session": "gone0fff", "worktree": "/w",
                   "path": format!("{id}.rs"), "suggested": "resume",
                   "report_only": report_only}),
        );
    }
    // A finished milestone.
    ev(
        "owner",
        70,
        "verified",
        json!({"verif_id": "v1", "task_id": "1.1", "level": 4, "sha": "a", "files": [],
               "evidence": "ok", "by": "owner"}),
    );
    view::glance(
        s.conn(),
        &Config::default(),
        t0() + SignedDuration::from_secs(3000),
    )
    .unwrap()
}

#[test]
fn no_jargon_on_screens() {
    let g = busy_glance();
    // The scenario really has one of everything.
    let kinds: Vec<&str> = g.needs_you.iter().map(|i| i.kind.as_str()).collect();
    for k in [
        "drift_red",
        "question",
        "permission_wait",
        "verify_overdue",
        "leftover",
    ] {
        assert!(kinds.contains(&k), "{k} missing from {kinds:?}");
    }
    let mut shown = Vec::new();
    screen_strings(&serde_json::to_value(&g).unwrap(), "", &mut shown);
    // The flags as each screen words them.
    for p in &g.projects {
        for s in &p.sessions {
            shown.extend(s.flags.iter().map(|f| f.kind.text().to_string()));
        }
    }
    let status = crate::status::render(
        &g,
        &crate::status::Options {
            color: false,
            dashboard: None,
        },
    );
    shown.push(status.clone());
    let app = App::new(g.clone(), false);
    let (lines, _) = draw(&app, 160, 60);
    shown.push(text(&lines));
    let bad: Vec<(String, Vec<String>)> = shown
        .iter()
        .map(|s| (s.clone(), crate::words::jargon_in(s)))
        .filter(|(_, j)| !j.is_empty())
        .collect();
    assert!(bad.is_empty(), "raw codes on screen: {bad:#?}");
    // The new words are there.
    assert!(status.contains("asked for"), "{status}");
    assert!(
        status.contains("hasn't confirmed it understood the handoff yet"),
        "{status}"
    );
}
