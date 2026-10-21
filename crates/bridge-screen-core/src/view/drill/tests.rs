use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::repo::RepoInfo;
use crate::store::{EventDraft, Store, append_in_at};

fn t0() -> Timestamp {
    "2026-10-14T09:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0() + SignedDuration::from_secs(secs)
}

#[allow(clippy::needless_pass_by_value)] // call sites pass json! literals
fn ev(s: &mut Store, session: &str, secs: i64, kind: &str, data: Value) {
    s.write(|tx| {
        let mut draft = EventDraft::new("r", session, kind, data.clone());
        if let Some(t) = data["task_id"].as_str() {
            draft = draft.task(t);
        }
        append_in_at(tx, draft, at(secs))?;
        Ok(())
    })
    .unwrap();
}

fn node(id: &str, parent: Option<&str>, kind: &str, estimate: Option<u32>) -> Value {
    json!({"id": id, "parent_id": parent, "kind": kind, "title": format!("T{id}"),
           "intent": "why", "estimate_min": estimate, "expected_files": []})
}

fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.register_repo(&RepoInfo {
        id: "r".into(),
        name: "shop".into(),
        root: "/code/shop".into(),
    })
    .unwrap();
    for (i, (id, parent, kind, est)) in [
        ("G", None, "goal", None),
        ("1", Some("G"), "milestone", None),
        ("1.1", Some("1"), "task", Some(30)),
        ("1.2", Some("1"), "task", Some(30)),
    ]
    .into_iter()
    .enumerate()
    {
        ev(
            &mut s,
            "owner",
            i64::try_from(i).unwrap(),
            "node_added",
            node(id, parent, kind, est),
        );
    }
    s
}

fn verified(id: &str, task: &str, level: i64, sha: &str) -> Value {
    json!({"verif_id": id, "task_id": task, "level": level, "sha": sha, "evidence": "tests",
           "by": "agent"})
}

#[test]
fn project_tree_scope_line_and_replans() {
    let mut s = store();
    ev(
        &mut s,
        "owner",
        10,
        "verified",
        verified("v1", "1.1", 2, "aaa"),
    );
    ev(
        &mut s,
        "owner",
        20,
        "replan",
        json!({"reason": "split the login work", "op_count": 2}),
    );
    ev(
        &mut s,
        "owner",
        20,
        "node_added",
        node("1.3", Some("1"), "task", Some(60)),
    );
    ev(&mut s, "owner", 20, "node_removed", json!({"id": "1.2"}));
    ev(
        &mut s,
        "s1",
        30,
        "session_started",
        json!({"cwd": "/w", "source": "startup"}),
    );
    ev(
        &mut s,
        "s1",
        31,
        "task_claimed",
        json!({"task_id": "1.3", "mode": "auto", "confidence": 1.0}),
    );
    ev(&mut s, "s1", 32, "prompt", json!({}));
    ev(&mut s, "s1", 152, "unit_stopped", json!({}));
    let p = project(s.conn(), "r", at(200)).unwrap();
    assert_eq!(p.name, "shop");
    let ids: Vec<&str> = p.tree.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["G", "1", "1.1", "1.3"]);
    let t13 = p.tree.iter().find(|n| n.id == "1.3").unwrap();
    assert_eq!((t13.estimate_min, t13.actual_min), (Some(60), 2));
    assert_eq!(
        p.replans,
        vec![Replan {
            ts: at(20).to_string(),
            reason: Some("split the login work".into()),
            op_count: 2,
            session_id: "owner".into()
        }]
    );
    let line: Vec<(u64, u64)> = p.scope.iter().map(|p| (p.total_min, p.done_min)).collect();
    // The goal alone, 1.1, then 1.2, then 1.1 done, then 1.3 added, then 1.2 removed.
    assert_eq!(
        line,
        vec![(0, 0), (30, 0), (60, 0), (60, 30), (120, 30), (90, 30)]
    );
    let times: Vec<Timestamp> = p.scope.iter().map(|p| p.ts.parse().unwrap()).collect();
    assert!(times.windows(2).all(|w| w[0] <= w[1]), "monotonic in time");
}

#[test]
fn scope_lines_are_downsampled_evenly() {
    let pts: Vec<usize> = (0..1000).collect();
    let d = downsample(pts, SCOPE_MAX_POINTS);
    assert_eq!(d.len(), SCOPE_MAX_POINTS);
    assert_eq!((d[0], d[d.len() - 1]), (0, 999));
    assert!(d.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(downsample(vec![1, 2, 3], 500), vec![1, 2, 3]);
}

#[test]
#[allow(clippy::too_many_lines)] // one scripted session
fn session_timeline_split_and_owner_answers() {
    let mut s = store();
    ev(
        &mut s,
        "s1",
        0,
        "session_started",
        json!({"cwd": "/w", "source": "startup", "worktree": "/w", "branch": "feat"}),
    );
    ev(
        &mut s,
        "s1",
        1,
        "task_claimed",
        json!({"task_id": "1.1", "mode": "auto", "confidence": 1.0}),
    );
    ev(&mut s, "s1", 10, "prompt", json!({}));
    ev(
        &mut s,
        "s1",
        20,
        "file_changed",
        json!({"tool": "Edit", "files": ["a.rs", "b.rs"]}),
    );
    ev(&mut s, "s1", 70, "unit_stopped", json!({"score": 0.1}));
    ev(
        &mut s,
        "s1",
        70,
        "unit_labeled",
        json!({"unit_no": 1, "label": "on_task", "source": "heuristic"}),
    );
    ev(&mut s, "s1", 100, "prompt", json!({}));
    ev(
        &mut s,
        "s1",
        100,
        "detour_opened",
        json!({"detour_id": "dx", "reason": "CI is red", "resume_cue": "rerun a.rs tests",
               "blocks": "1.1"}),
    );
    ev(
        &mut s,
        "s1",
        100,
        "item_suspended",
        json!({"item_id": "i1", "kind": "task", "ref_id": "1.1", "resume_cue": "rerun a.rs tests",
               "by_detour": "dx"}),
    );
    ev(&mut s, "s1", 400, "unit_stopped", json!({}));
    ev(
        &mut s,
        "s1",
        400,
        "unit_labeled",
        json!({"unit_no": 2, "label": "detour", "source": "declared"}),
    );
    ev(
        &mut s,
        "s1",
        410,
        "checkpoint",
        json!({"sha": "abc123", "task_id": "1.1", "next_action": "rerun tests"}),
    );
    ev(
        &mut s,
        "s1",
        420,
        "handoff_note",
        json!({"task_id": "1.1", "in_flight": "CI half fixed", "if_then": ["if red, then retry"]}),
    );
    s.write(|tx| {
        append_in_at(
            tx,
            EventDraft::new(
                "r",
                "owner",
                "owner_action",
                json!({"action": "drift", "answer": "continue", "target": "s1",
                       "reason": "the CI fix unblocks the release"}),
            ),
            at(430),
        )?;
        Ok(())
    })
    .unwrap();
    let d = session(s.conn(), "s1", &Config::default(), at(500))
        .unwrap()
        .unwrap();
    assert_eq!(d.units.len(), 2);
    assert_eq!(d.units[0].files, 2);
    assert_eq!(d.units[0].label, Some(Label::OnTask));
    assert_eq!(d.units[1].detour.as_deref(), Some("D1"));
    assert_eq!(d.split.on_task_s, 60);
    assert_eq!(
        d.split.detours,
        vec![DetourTime {
            label: "D1".into(),
            reason: "CI is red".into(),
            secs: 300
        }]
    );
    assert_eq!(d.suspended[0].what, "1.1 T1.1");
    assert_eq!(d.suspended[0].resume_cue, "rerun a.rs tests");
    assert_eq!(d.checkpoints[0].sha, "abc123");
    assert_eq!(d.handoff_note.as_ref().unwrap().in_flight, "CI half fixed");
    assert_eq!(
        d.owner_answers,
        vec![OwnerAnswer {
            ts: at(430).to_string(),
            answer: "continue".into(),
            reason: Some("the CI fix unblocks the release".into())
        }]
    );
    assert!(
        session(s.conn(), "nope", &Config::default(), at(0))
            .unwrap()
            .is_none()
    );
}

#[test]
fn task_decisions_verifications_actors_and_retries() {
    let mut s = store();
    s.write(|tx| {
        let mut d = EventDraft::new(
            "r",
            "s1",
            "session_started",
            json!({"cwd": "/w", "source": "startup", "model": "opus"}),
        );
        d.actor = Actor {
            agent_id: None,
            agent_type: Some("main".into()),
            model: Some("opus".into()),
        };
        append_in_at(tx, d, at(0))?;
        Ok(())
    })
    .unwrap();
    ev(
        &mut s,
        "s1",
        1,
        "task_claimed",
        json!({"task_id": "1.1", "mode": "auto", "confidence": 1.0}),
    );
    ev(&mut s, "s1", 2, "prompt", json!({}));
    ev(&mut s, "s1", 62, "unit_stopped", json!({}));
    for (id, what, sup) in [
        ("D1", "Use sqlite", None),
        ("D2", "Use sqlite with WAL", Some("D1")),
    ] {
        ev(
            &mut s,
            "s1",
            60,
            "decision",
            json!({"decision_id": id, "task_id": "1.1", "what": what, "why": "simple",
                   "alternatives": ["postgres"], "supersedes": sup}),
        );
    }
    ev(
        &mut s,
        "s1",
        70,
        "verified",
        verified("v1", "1.1", 2, "aaa"),
    );
    ev(
        &mut s,
        "owner",
        80,
        "verification_failed",
        json!({"task_id": "1.1", "level": 4, "evidence": "login 404s", "by": "owner"}),
    );
    ev(&mut s, "s1", 90, "prompt", json!({}));
    ev(&mut s, "s1", 95, "unit_stopped", json!({}));
    ev(
        &mut s,
        "s1",
        100,
        "verified",
        verified("v2", "1.1", 2, "bbb"),
    );
    ev(
        &mut s,
        "s1",
        110,
        "squashed",
        json!({"branch": "feat", "old_shas": ["bbb"], "new_sha": "ccc", "backup_ref": "x"}),
    );
    let t = task(s.conn(), "r", "1.1", &Config::default()).unwrap();
    assert_eq!(t.title, "T1.1");
    assert_eq!(t.decisions.len(), 1, "superseded decisions sit in a chain");
    let chain: Vec<&str> = t.decisions[0]
        .iter()
        .map(|d| d.decision_id.as_str())
        .collect();
    assert_eq!(chain, vec!["D1", "D2"]);
    let shas: Vec<&str> = t.verifications.iter().map(|v| v.sha.as_str()).collect();
    assert_eq!(shas, vec!["aaa", "ccc"], "the squash re-pointed the second");
    assert_eq!(
        t.actors,
        vec![
            ActorRow {
                agent_type: Some("main".into()),
                model: Some("opus".into())
            },
            ActorRow {
                agent_type: None,
                model: Some("opus".into())
            }
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
    assert_eq!(t.retries, 1);
    assert_eq!(t.usage, None);
    assert_eq!(t.actual_min, 1);
}

#[test]
fn history_filters_and_pages() {
    let mut s = store();
    for i in 0..5 {
        ev(&mut s, "s9", 10 + i, "note", json!({"n": i}));
    }
    ev(&mut s, "s9", 20, "note", json!({"big": "x".repeat(3000)}));
    let all = history(
        s.conn(),
        "r",
        &HistoryFilter {
            limit: 200,
            ..HistoryFilter::default()
        },
    )
    .unwrap();
    assert_eq!(all.events.len(), 10);
    assert_eq!(all.next_before, None);
    assert!(
        all.events.windows(2).all(|w| w[0].pos > w[1].pos),
        "newest first"
    );
    let big = all.events[0].data.as_str().unwrap();
    assert!(big.len() <= HISTORY_DATA_MAX + 3 && big.ends_with('…'));
    let notes = HistoryFilter {
        kind: Some("note".into()),
        session: Some("s9".into()),
        limit: 2,
        ..HistoryFilter::default()
    };
    let p1 = history(s.conn(), "r", &notes).unwrap();
    assert_eq!(p1.events.len(), 2);
    let p2 = history(
        s.conn(),
        "r",
        &HistoryFilter {
            before: p1.next_before,
            ..notes.clone()
        },
    )
    .unwrap();
    assert!(p2.events[0].pos < p1.events[1].pos);
    let tasks = history(
        s.conn(),
        "r",
        &HistoryFilter {
            task: Some("1.2".into()),
            limit: 50,
            ..HistoryFilter::default()
        },
    )
    .unwrap();
    assert_eq!(
        tasks.events.len(),
        0,
        "node_added rows carry no task column here"
    );
}

#[test]
fn tuning_lists_changes_newest_first_with_a_revert() {
    let mut s = store();
    let cfg = Config::default();
    assert_eq!(tuning(s.conn(), "r", &cfg).unwrap().revert, None);
    for (secs, key, old, new) in [
        (10, "drift_lower", 0.3, 0.35),
        (20, "drift_upper", 0.7, 0.65),
    ] {
        ev(
            &mut s,
            "owner",
            secs,
            "config_changed",
            json!({"scope": "repo", "key": key, "old": old, "new": new, "samples": 20}),
        );
    }
    let t = tuning(s.conn(), "r", &cfg).unwrap();
    assert!(t.tuned);
    let keys: Vec<&str> = t.changes.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys, vec!["drift_upper", "drift_lower"]);
    assert_eq!(
        serde_json::to_value(&t.revert.as_ref().unwrap().action).unwrap(),
        json!({"kind": "reset_drift", "repo": "r"})
    );
    // Stable JSON (snapshot).
    assert_eq!(
        serde_json::to_string_pretty(&t).unwrap(),
        r#"{
  "repo_id": "r",
  "current": {
    "drift_lower": 0.35,
    "drift_upper": 0.65,
    "tolerance_pct": 20.0
  },
  "defaults": {
    "drift_lower": 0.3,
    "drift_upper": 0.7,
    "tolerance_pct": 20.0
  },
  "tuned": true,
  "samples": 0,
  "changes": [
    {
      "ts": "2026-10-14T09:00:20Z",
      "key": "drift_upper",
      "old": 0.7,
      "new": 0.65,
      "samples": 20
    },
    {
      "ts": "2026-10-14T09:00:10Z",
      "key": "drift_lower",
      "old": 0.3,
      "new": 0.35,
      "samples": 20
    }
  ],
  "revert": {
    "label": "Revert to defaults",
    "action": {
      "kind": "reset_drift",
      "repo": "r"
    },
    "inputs": [],
    "tone": "neutral"
  }
}"#
    );
}

fn reply(id: &str, model: &str, secs: i64, input: u64, output: u64) -> Value {
    json!({"message_id": id, "model": model, "ts": at(secs).to_string(), "input": input,
           "output": output})
}

#[test]
fn task_drill_has_cost_by_model_and_agent_steps_and_ratio() {
    let mut s = store();
    for (id, parent, kind) in [("1.1.1", "1.1", "subtask"), ("1.1.2", "1.1", "subtask")] {
        ev(
            &mut s,
            "owner",
            5,
            "node_added",
            node(id, Some(parent), kind, Some(15)),
        );
    }
    ev(
        &mut s,
        "s1",
        10,
        "session_started",
        json!({"cwd": "/w", "source": "startup"}),
    );
    ev(
        &mut s,
        "s1",
        10,
        "task_claimed",
        json!({"task_id": "1.2", "mode": "auto", "confidence": 1.0}),
    );
    ev(&mut s, "s1", 11, "prompt", json!({"chars": 1}));
    ev(
        &mut s,
        "s1",
        20,
        "usage",
        json!({"path": "/t", "offset_after": 1, "replies": [
            reply("a", "claude-opus-5-5", 12, 1_000_000, 100_000),
            reply("b", "mystery", 13, 0, 5_000)]}),
    );
    ev(
        &mut s,
        "s1",
        21,
        "usage",
        json!({"path": "/sub", "offset_after": 1, "agent_id": "x", "agent_type": "Explore",
               "replies": [reply("c", "claude-sonnet-5-5", 14, 500_000, 0)]}),
    );
    ev(&mut s, "s1", 1811, "unit_stopped", json!({}));
    ev(
        &mut s,
        "s1",
        1812,
        "verified",
        verified("v2", "1.2", 2, "abc"),
    );
    let cfg = Config::default();
    let t = task(s.conn(), "r", "1.2", &cfg).unwrap();
    let u = t.usage.unwrap();
    assert_eq!(u.total.text, "≥ $7.00 (some models unpriced)");
    let models: Vec<(&str, &str)> = u
        .by_model
        .iter()
        .map(|m| (m.label.as_str(), m.cost_text.as_str()))
        .collect();
    assert_eq!(
        models,
        [
            ("Opus 5.5", "$6.00"),
            ("Sonnet 5.5", "$1.00"),
            ("mystery", "cost unknown")
        ]
    );
    let agents: Vec<(&str, &str)> = u
        .by_agent
        .iter()
        .map(|a| (a.label.as_str(), a.cost_text.as_str()))
        .collect();
    assert_eq!(
        agents,
        [
            ("Main agent", "≥ $6.00 (some models unpriced)"),
            ("Explore", "$1.00")
        ]
    );
    // 30 minutes against a 30-minute estimate, now done.
    assert_eq!(t.ratio, Some(1.0));
    assert_eq!(t.steps.len(), 0);
    // 1.1's subtasks: the first not done is current.
    ev(
        &mut s,
        "owner",
        1900,
        "verified",
        verified("v3", "1.1.1", 2, "abc"),
    );
    let t = task(s.conn(), "r", "1.1", &cfg).unwrap();
    let steps: Vec<(&str, bool)> = t
        .steps
        .iter()
        .map(|st| (st.id.as_str(), st.current))
        .collect();
    assert_eq!(steps, [("1.1.1", false), ("1.1.2", true)]);
    assert_eq!((t.usage, t.ratio), (None, None));
}

#[test]
fn session_drill_has_tokens_per_unit_and_its_cost() {
    let mut s = store();
    ev(
        &mut s,
        "s1",
        0,
        "session_started",
        json!({"cwd": "/w", "source": "startup"}),
    );
    ev(&mut s, "s1", 1, "prompt", json!({"chars": 1}));
    ev(
        &mut s,
        "s1",
        5,
        "usage",
        json!({"path": "/t", "offset_after": 1, "replies": [
            reply("a", "claude-opus-5-5", 2, 10_000, 2_400)]}),
    );
    ev(&mut s, "s1", 60, "unit_stopped", json!({}));
    ev(&mut s, "s1", 61, "prompt", json!({"chars": 1}));
    let d = session(s.conn(), "s1", &Config::default(), at(100))
        .unwrap()
        .unwrap();
    assert_eq!(d.units[0].tokens_text.as_deref(), Some("12k"));
    assert_eq!(d.units[1].tokens_text, None);
    assert_eq!(d.cost.unwrap().summary, "$0.09 spent · 12k tokens");
}

#[test]
fn calibration_drill_ratios_history_and_prices() {
    let mut s = store();
    let mut cfg = Config::default();
    cfg.prices.insert(
        "my-model".into(),
        crate::config::Price {
            input: 1.0,
            output: 2.0,
            cache_read: 0.1,
            cache_write_5m: 1.25,
            cache_write_1h: 2.0,
        },
    );
    // Empty: no points, the thin-data reason, no history.
    let c = calibration(s.conn(), "r", &cfg, at(0)).unwrap();
    assert_eq!((c.models.len(), c.pooled_text.as_deref()), (0, None));
    assert_eq!(
        c.forecast.text,
        "Not enough finished tasks to forecast yet — 3 more are needed"
    );
    assert_eq!(c.history.len(), 0);
    let mine = c.prices.iter().find(|p| p.model == "my-model").unwrap();
    assert_eq!(mine.source, "yours");
    let opus = c
        .prices
        .iter()
        .find(|p| p.model == "claude-opus-5-5")
        .unwrap();
    assert_eq!(
        (opus.name.as_str(), opus.source.as_str()),
        ("Opus 5.5", "default")
    );
    // 1.1 took 60 minutes for 30, on Opus; an unpriced model also spent tokens.
    ev(
        &mut s,
        "s1",
        10,
        "session_started",
        json!({"cwd": "/w", "source": "startup", "model": "claude-opus-5-5"}),
    );
    ev(
        &mut s,
        "s1",
        10,
        "task_claimed",
        json!({"task_id": "1.1", "mode": "auto", "confidence": 1.0}),
    );
    ev(&mut s, "s1", 11, "prompt", json!({"chars": 1}));
    ev(
        &mut s,
        "s1",
        20,
        "usage",
        json!({"path": "/t", "offset_after": 1,
               "replies": [reply("z", "mystery", 12, 0, 10)]}),
    );
    ev(&mut s, "s1", 3611, "unit_stopped", json!({}));
    ev(
        &mut s,
        "s1",
        3612,
        "verified",
        verified("v1", "1.1", 2, "abc"),
    );
    ev(
        &mut s,
        "owner",
        3700,
        "forecast_ran",
        json!({"p50_hours": 0.0, "p90_hours": 0.0, "remaining_min": 30, "unestimated": 0,
               "points": 1, "reason": "Not enough finished tasks to forecast yet — 2 more are needed"}),
    );
    let c = calibration(s.conn(), "r", &cfg, at(4000)).unwrap();
    assert_eq!(c.models.len(), 1);
    // The task's only replies are the unpriced model's: it is the task's model.
    assert_eq!(c.models[0].model, "mystery");
    assert_eq!(c.models[0].text, "tasks take 2× their estimate");
    assert!(!c.models[0].own);
    assert_eq!(
        c.pooled_text.as_deref(),
        Some("tasks take 2× their estimate")
    );
    assert_eq!(c.history.len(), 1);
    assert_eq!(c.unpriced_models, ["mystery"]);
    assert_eq!(
        c.notes,
        ["No price for mystery: add one under [prices] in config.toml."]
    );
}
