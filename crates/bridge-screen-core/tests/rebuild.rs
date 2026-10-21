use bridge_screen_core::claim::{ClaimMode, ClaimRequest, ClaimTarget, claim};
use bridge_screen_core::plan::{EditContext, Hill, NodeKind, PlanOp, plan_edit};
use bridge_screen_core::store::{Actor, EventDraft, Store, append_in_at};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

fn ctx(repo: &str, replan: bool) -> EditContext {
    EditContext {
        repo_id: repo.into(),
        session_id: "owner".into(),
        actor: Actor::default(),
        reason: Some("test".into()),
        replan,
    }
}

fn add(parent: Option<&str>, kind: NodeKind, title: &str) -> PlanOp {
    PlanOp::AddNode {
        parent: parent.map(str::to_string),
        kind,
        title: title.into(),
        intent: kind.requires_intent().then(|| "why".to_string()),
        estimate_min: Some(20),
        expected_files: vec![format!("src/{title}.rs")],
    }
}

/// Every view row, as text, in a stable order.
fn dump(store: &Store) -> Vec<String> {
    let mut out = Vec::new();
    for (table, order) in [
        ("plan_nodes", "repo_id, id"),
        ("plan_deps", "repo_id, node_id, dep_repo_id, depends_on"),
        ("sessions", "session_id"),
        ("units", "session_id, unit_no"),
        ("task_time", "repo_id, task_id"),
        ("detours", "detour_id"),
        ("suspended_items", "item_id"),
        ("drift_samples", "repo_id, unit_ref, ts, label"),
        ("repo_tuning", "repo_id"),
        ("notes_sent", "session_id, unit_no, ts, kind"),
        ("checkpoints", "sha"),
        ("leftovers", "leftover_id"),
        ("handoff_notes", "session_id, ts"),
        ("claims_done", "repo_id, task_id, ts"),
        ("verifications", "verif_id"),
        ("decisions", "repo_id, decision_id"),
        ("verify_nudges", "repo_id, task_id"),
        ("repo_settings", "repo_id"),
        ("usage_replies", "message_id"),
        ("transcript_cursors", "path"),
        ("forecast_history", "repo_id, ts"),
        ("plan_sources", "repo_id, path"),
        ("plan_source_items", "repo_id, path, task_no, step_no"),
        ("waits", "wait_id"),
    ] {
        let mut stmt = store
            .conn()
            .prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
            .unwrap();
        let cols = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                Ok((0..cols)
                    .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                    .collect::<Vec<_>>()
                    .join("|"))
            })
            .unwrap();
        out.extend(rows.map(|r| format!("{table}: {}", r.unwrap())));
    }
    out
}

#[test]
fn rebuild_reproduces_incremental_views() {
    let mut s = Store::open_in_memory().unwrap();
    for repo in ["a", "b"] {
        plan_edit(
            &mut s,
            &ctx(repo, false),
            &[
                add(None, NodeKind::Goal, "goal"),
                add(Some("G"), NodeKind::Milestone, "core"),
                add(Some("1"), NodeKind::Task, "schema"),
                add(Some("1"), NodeKind::Task, "append"),
                add(Some("1.1"), NodeKind::Subtask, "sql"),
                add(Some("G"), NodeKind::Milestone, "screens"),
            ],
        )
        .unwrap();
    }
    plan_edit(
        &mut s,
        &ctx("a", false),
        &[
            PlanOp::AddDep {
                id: "1.2".into(),
                depends_on: "1.1".into(),
                repo: None,
            },
            PlanOp::SetHill {
                id: "1".into(),
                hill: Hill::Downhill,
            },
            PlanOp::EditNode {
                id: "1.1".into(),
                title: Some("schema v1".into()),
                expected_files: None,
            },
            PlanOp::MoveNode {
                id: "1.2".into(),
                new_parent: "2".into(),
            },
            PlanOp::SetEstimate {
                id: "1.1.1".into(),
                estimate_min: 45,
            },
        ],
    )
    .unwrap();
    plan_edit(
        &mut s,
        &ctx("a", true),
        &[
            PlanOp::RemoveNode { id: "1".into() },
            add(Some("2"), NodeKind::Task, "late"),
        ],
    )
    .unwrap();

    let before = dump(&s);
    assert!(before.len() > 10);
    let stats = s.rebuild().unwrap();
    assert_eq!(stats.events, 6 + 6 + 5 + 3);
    assert_eq!(dump(&s), before);
}

/// Two sessions in one repo, interleaved: waits, a missing Stop, a resume and an end.
/// Compaction mid-unit, a crash and resume with a unit open, and a late hook after an end.
fn restarts() -> Vec<(&'static str, i64, &'static str, Value)> {
    vec![
        // Compaction mid-unit, then a crash and a resume with the unit still open.
        (
            "s1",
            420,
            "session_started",
            json!({"cwd": "/a", "source": "compact"}),
        ),
        (
            "s1",
            430,
            "needs_owner",
            json!({"kind": "permission_prompt"}),
        ),
        (
            "s1",
            5000,
            "session_started",
            json!({"cwd": "/a", "source": "resume"}),
        ),
        ("s1", 5010, "prompt", json!({"prompt_id": "p3", "chars": 7})),
        // A late hook after a session ended changes nothing.
        (
            "s3",
            5990,
            "session_started",
            json!({"cwd": "/a", "source": "startup"}),
        ),
        ("s3", 6000, "session_ended", json!({"reason": "logout"})),
        ("s3", 6001, "needs_owner", json!({"kind": "idle_prompt"})),
    ]
}

#[allow(clippy::too_many_lines)] // one event per line, as data
fn script_sessions(s: &mut Store) -> usize {
    let t0: Timestamp = "2026-10-13T09:00:00Z".parse().unwrap();
    let mut script: Vec<(&str, i64, &str, Value)> = vec![
        (
            "s1",
            0,
            "session_started",
            json!({"cwd": "/a", "branch": "main", "source": "startup"}),
        ),
        (
            "s2",
            1,
            "session_started",
            json!({"cwd": "/a/wt", "branch": "feat", "source": "startup"}),
        ),
        ("s1", 2, "prompt", json!({"prompt_id": "p1", "chars": 12})),
        ("s2", 3, "prompt", json!({"prompt_id": "q1", "chars": 40})),
        (
            "s1",
            5,
            "tool_intent",
            json!({"tool": "Edit", "files": ["src/a.rs"]}),
        ),
        (
            "s1",
            9,
            "file_changed",
            json!({"tool": "Edit", "files": ["src/a.rs"]}),
        ),
        (
            "s2",
            10,
            "needs_owner",
            json!({"kind": "permission_prompt"}),
        ),
        (
            "s2",
            70,
            "file_changed",
            json!({"tool": "Write", "files": ["src/b.rs"]}),
        ),
        (
            "s1",
            80,
            "unit_stopped",
            json!({"unit_no": 1, "summary": "did a"}),
        ),
        (
            "s2",
            81,
            "subagent_stopped",
            json!({"agent_type": "Explore", "summary": "x"}),
        ),
        ("s2", 90, "prompt", json!({"prompt_id": "q2", "chars": 5})),
        ("s2", 95, "precompact", json!({})),
        (
            "s2",
            120,
            "unit_stopped",
            json!({"unit_no": 2, "summary": null}),
        ),
        ("s1", 200, "needs_owner", json!({"kind": "idle_prompt"})),
        ("s1", 300, "session_ended", json!({"reason": "other"})),
        (
            "s1",
            400,
            "session_started",
            json!({"cwd": "/a", "source": "resume"}),
        ),
        ("s1", 410, "prompt", json!({"prompt_id": "p2", "chars": 3})),
        // Questions to the owner: s2's ask tool answered, s1's turn-end question open.
        (
            "s2",
            100,
            "owner_question",
            json!({"unit_no": 2, "source": "ask_tool"}),
        ),
        (
            "s2",
            105,
            "question_answered",
            json!({"source": "ask_tool"}),
        ),
        (
            "s1",
            420,
            "unit_stopped",
            json!({"unit_no": 2, "summary": null}),
        ),
        (
            "s1",
            420,
            "owner_question",
            json!({"unit_no": 2, "source": "turn_end"}),
        ),
    ];
    script.extend(restarts());
    let n = script.len();
    s.write(|tx| {
        for (session, secs, kind, data) in script {
            let draft = EventDraft::new("a", session, kind, data);
            append_in_at(tx, draft, t0 + SignedDuration::from_secs(secs))?;
        }
        Ok(())
    })
    .unwrap();
    n
}

#[test]
fn rebuild_reproduces_session_views() {
    let mut s = Store::open_in_memory().unwrap();
    let mut events = script_sessions(&mut s);
    plan_edit(
        &mut s,
        &ctx("a", false),
        &[
            add(None, NodeKind::Goal, "goal"),
            add(Some("G"), NodeKind::Milestone, "core"),
            add(Some("1"), NodeKind::Task, "schema"),
        ],
    )
    .unwrap();
    events += 3;
    let claim_req = |session: &str, task: ClaimTarget| ClaimRequest {
        session_id: session.into(),
        repo_id: "a".into(),
        task,
        mode: ClaimMode::Confirmed,
        confidence: 0.5,
        actor: Actor::default(),
    };
    claim(
        &mut s,
        &claim_req("s1", ClaimTarget::Existing("1.1".into())),
    )
    .unwrap();
    // A new task (node_added + task_released + task_claimed), then s2 claims 1.1 too.
    claim(
        &mut s,
        &claim_req(
            "s1",
            ClaimTarget::New {
                parent: "1".into(),
                title: "late work".into(),
                estimate_min: Some(10),
            },
        ),
    )
    .unwrap();
    claim(
        &mut s,
        &claim_req("s2", ClaimTarget::Existing("1.1".into())),
    )
    .unwrap();
    // The switch to the new task also makes a checkpoint due.
    events += 1 + 4 + 1;
    s.append(EventDraft::new("a", "s1", "unit_stopped", json!({})))
        .unwrap();
    s.append(EventDraft::new(
        "a",
        "s2",
        "session_ended",
        json!({"reason": "other"}),
    ))
    .unwrap();
    events += 2;
    let before = dump(&s);
    let status = |id: &str| {
        bridge_screen_core::plan::get_node(s.conn(), "a", id)
            .unwrap()
            .unwrap()
            .status
    };
    // s1 holds 1.2; 1.1 went back to todo when s2 (its last holder) ended.
    assert_eq!(status("1.2"), bridge_screen_core::plan::NodeStatus::Active);
    assert_eq!(status("1.1"), bridge_screen_core::plan::NodeStatus::Todo);
    assert!(before.iter().any(|r| r.starts_with("task_time: ")));
    assert!(before.iter().any(|r| r.starts_with("units: ")));
    assert!(before.iter().any(|r| r.starts_with("sessions: ")));
    assert_eq!(s.rebuild().unwrap().events, events);
    assert_eq!(dump(&s), before);
}

/// The scripted detour session for [`rebuild_reproduces_tangent_views`].
#[allow(clippy::too_many_lines)] // one event per line, as data
fn tangent_script() -> Vec<(&'static str, i64, &'static str, Value)> {
    vec![
        (
            "s1",
            0,
            "session_started",
            json!({"cwd": "/a", "source": "startup"}),
        ),
        (
            "s1",
            1,
            "task_claimed",
            json!({"task_id": "1", "mode": "auto", "confidence": 0.9}),
        ),
        ("s1", 2, "prompt", json!({"chars": 3})),
        (
            "s1",
            5,
            "detour_opened",
            json!({"detour_id": "D1", "reason": "broken build",
            "blocks": "1", "resume_cue": "rerun test x"}),
        ),
        (
            "s1",
            5,
            "item_suspended",
            json!({"item_id": "I1", "kind": "task", "ref_id": "1",
            "resume_cue": "rerun test x", "by_detour": "D1"}),
        ),
        ("s1", 60, "unit_stopped", json!({"unit_no": 1})),
        (
            "s1",
            60,
            "unit_labeled",
            json!({"unit_no": 1, "label": "detour", "source": "declared"}),
        ),
        (
            "s1",
            61,
            "note_sent",
            json!({"unit_no": 1, "kind": "return_to_task",
            "channel": "tool_result", "state_key": "ret:D1"}),
        ),
        ("s1", 70, "prompt", json!({"chars": 3})),
        (
            "s1",
            75,
            "detour_opened",
            json!({"detour_id": "D2", "reason": "flaky dep",
            "blocks": "1", "resume_cue": "back to D1"}),
        ),
        (
            "s1",
            75,
            "item_suspended",
            json!({"item_id": "I2", "kind": "detour", "ref_id": "D1",
            "resume_cue": "back to D1", "by_detour": "D2"}),
        ),
        (
            "s1",
            90,
            "nav_state_changed",
            json!({"from": "green", "to": "amber", "away_secs": 900}),
        ),
        (
            "s1",
            100,
            "unit_stopped",
            json!({"unit_no": 2, "score": 0.8}),
        ),
        (
            "s1",
            100,
            "drift_flagged",
            json!({"unit_no": 2, "score": 0.8}),
        ),
        (
            "s1",
            101,
            "unit_labeled",
            json!({"unit_no": 2, "label": "on_task", "source": "agent",
            "heuristic": "drift"}),
        ),
        (
            "s1",
            102,
            "detour_closed",
            json!({"detour_id": "D2", "outcome": "done"}),
        ),
        ("s1", 102, "item_resumed", json!({"item_id": "I2"})),
        (
            "s1",
            103,
            "detour_promoted",
            json!({"detour_id": "D1", "task_id": "2"}),
        ),
        (
            "owner",
            110,
            "owner_action",
            json!({"action": "drift", "answer": "stop", "target": "s1"}),
        ),
        ("s1", 120, "prompt", json!({"chars": 3})),
        (
            "s1",
            150,
            "unit_stopped",
            json!({"unit_no": 3, "score": 0.1}),
        ),
        (
            "s1",
            150,
            "unit_labeled",
            json!({"unit_no": 3, "label": "drift", "source": "owner",
            "score": 0.1, "heuristic": "on_task"}),
        ),
        ("s1", 151, "prompt", json!({"chars": 3})),
        (
            "s1",
            152,
            "detour_opened",
            json!({"detour_id": "D3", "reason": "add a licence", "blocks": "2",
            "resume_cue": "back to 2", "requested": true}),
        ),
        (
            "s1",
            155,
            "unit_stopped",
            json!({"unit_no": 4, "score": 0.9}),
        ),
        (
            "s1",
            155,
            "unit_labeled",
            json!({"unit_no": 4, "label": "requested", "source": "declared",
            "score": 0.9, "heuristic": "drift"}),
        ),
        (
            "s1",
            156,
            "branch_changed",
            json!({"branch": "feat/licence"}),
        ),
        (
            "owner",
            160,
            "config_changed",
            json!({"scope": "repo", "key": "drift_upper",
            "old": 0.7, "new": 0.65, "samples": 20}),
        ),
        ("owner", 170, "tuning_ran", json!({"samples": 40})),
    ]
}

/// A session that claims a task, opens nested detours, labels units, gets flagged, hears
/// from the owner, and has its thresholds tuned.
#[test]
fn rebuild_reproduces_tangent_views() {
    let mut s = Store::open_in_memory().unwrap();
    plan_edit(
        &mut s,
        &ctx("a", false),
        &[
            add(None, NodeKind::Goal, "goal"),
            add(Some("G"), NodeKind::Task, "schema"),
        ],
    )
    .unwrap();
    let t0: Timestamp = "2026-10-13T09:00:00Z".parse().unwrap();
    let script = tangent_script();
    s.write(|tx| {
        for (session, secs, kind, data) in script {
            let draft = EventDraft::new("a", session, kind, data);
            append_in_at(tx, draft, t0 + SignedDuration::from_secs(secs))?;
        }
        Ok(())
    })
    .unwrap();
    let before = dump(&s);
    for table in [
        "detours",
        "suspended_items",
        "drift_samples",
        "repo_tuning",
        "notes_sent",
    ] {
        let prefix = format!("{table}: ");
        assert!(before.iter().any(|r| r.starts_with(&prefix)), "{table}");
    }
    assert!(
        before
            .iter()
            .any(|r| r.starts_with("repo_tuning: ") && r.ends_with("|Integer(40)")),
        "{before:?}"
    );
    let store_branch = bridge_screen_core::session::get(s.conn(), "s1")
        .unwrap()
        .unwrap()
        .branch;
    assert_eq!(store_branch.as_deref(), Some("feat/licence"));
    // The asked-for detour keeps its flag (the last column).
    assert!(
        before
            .iter()
            .any(|r| r.starts_with("detours: Text([68, 51])") && r.ends_with("|Integer(1)")),
        "{before:?}"
    );
    s.rebuild().unwrap();
    assert_eq!(dump(&s), before);
}

/// Two sessions hand work over: a claim, a checkpoint asked for, missed and made, a note,
/// a briefing and read-back, a dirty end, a crash, leftovers found and resolved, a squash.
#[allow(clippy::too_many_lines)] // one event per line, as data
fn handoff_script() -> Vec<(&'static str, i64, &'static str, Value)> {
    vec![
        (
            "s1",
            0,
            "session_started",
            json!({"cwd": "/a/sub", "worktree": "/a", "source": "startup"}),
        ),
        (
            "s1",
            1,
            "briefed",
            json!({"status": "watcher", "readback_required": true}),
        ),
        (
            "s1",
            2,
            "readback",
            json!({"result": "mismatch", "diff": "task"}),
        ),
        (
            "s1",
            3,
            "task_claimed",
            json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        ),
        ("s1", 4, "prompt", json!({"chars": 3})),
        (
            "s1",
            9,
            "file_changed",
            json!({"tool": "Edit", "files": ["src/a.rs"]}),
        ),
        (
            "s1",
            10,
            "claimed_done",
            json!({"task_id": "1.1", "evidence": "tests pass"}),
        ),
        (
            "s1",
            10,
            "checkpoint_due",
            json!({"reason": "claimed_done"}),
        ),
        ("s1", 20, "unit_stopped", json!({"unit_no": 1})),
        (
            "s1",
            20,
            "note_sent",
            json!({"unit_no": 1, "kind": "checkpoint", "channel": "stop", "state_key": "cp:1"}),
        ),
        ("s1", 30, "checkpoint_missed", json!({"unit_no": 1})),
        (
            "s1",
            40,
            "checkpoint",
            json!({"sha": "aaa", "task_id": "1.1", "next_action": "wire the form"}),
        ),
        (
            "s1",
            41,
            "handoff_note",
            json!({"task_id": "1.1", "in_flight": "form half done", "if_then": ["if x then y"]}),
        ),
        (
            "s1",
            50,
            "session_ended",
            json!({"reason": "other", "end_kind": "dirty", "dirty_files": ["src/a.rs"]}),
        ),
        (
            "s2",
            60,
            "session_started",
            json!({"cwd": "/a/wt", "worktree": "/a/wt", "source": "startup"}),
        ),
        ("s2", 61, "prompt", json!({"chars": 3})),
        (
            "s2",
            70,
            "file_changed",
            json!({"tool": "Edit", "files": ["src/b.rs"]}),
        ),
        (
            "s2",
            4000,
            "session_crashed",
            json!({"last_event_at": "2026-10-13T09:01:10Z"}),
        ),
        (
            "s3",
            4001,
            "session_started",
            json!({"cwd": "/a", "worktree": "/a", "source": "startup"}),
        ),
        (
            "s3",
            4002,
            "leftover_found",
            json!({"leftover_id": "L1", "from_session": "s1", "worktree": "/a",
                   "path": "src/a.rs", "task_id": "1.1", "suggested": "adopt",
                   "report_only": false}),
        ),
        (
            "s3",
            4002,
            "leftover_found",
            json!({"leftover_id": "L2", "from_session": "s2", "worktree": "/a/wt",
                   "path": "src/b.rs", "task_id": null, "suggested": "undo",
                   "report_only": false}),
        ),
        (
            "s3",
            4002,
            "leftovers_analysed",
            json!({"from_session": "s1"}),
        ),
        (
            "s3",
            4003,
            "leftover_resolved",
            json!({"leftover_id": "L1", "action": "adopt", "commit": "bbb", "by": "agent"}),
        ),
        (
            "owner",
            4004,
            "leftover_resolved",
            json!({"leftover_id": "L2", "action": "undo", "stash_ref": "ccc", "by": "owner"}),
        ),
        (
            "s3",
            4005,
            "squashed",
            json!({"branch": "feat", "old_shas": ["aaa", "bbb"], "new_sha": "ddd",
                   "backup_ref": "refs/bridge-screen/backup/feat/1"}),
        ),
    ]
}

#[test]
fn rebuild_reproduces_handoff_views() {
    let mut s = Store::open_in_memory().unwrap();
    plan_edit(
        &mut s,
        &ctx("a", false),
        &[
            add(None, NodeKind::Goal, "goal"),
            add(Some("G"), NodeKind::Milestone, "core"),
            add(Some("1"), NodeKind::Task, "schema"),
        ],
    )
    .unwrap();
    let t0: Timestamp = "2026-10-13T09:00:00Z".parse().unwrap();
    s.write(|tx| {
        for (session, secs, kind, data) in handoff_script() {
            let draft = EventDraft::new("a", session, kind, data);
            append_in_at(tx, draft, t0 + SignedDuration::from_secs(secs))?;
        }
        Ok(())
    })
    .unwrap();
    let before = dump(&s);
    for table in ["checkpoints", "leftovers", "handoff_notes", "claims_done"] {
        let prefix = format!("{table}: ");
        assert!(before.iter().any(|r| r.starts_with(&prefix)), "{table}");
    }
    s.rebuild().unwrap();
    assert_eq!(dump(&s), before);
}

/// A task climbs the ladder, fails, climbs again and goes stale; it is squashed, nudged
/// and escalated; decisions are logged, merged and superseded; a test command is set.
#[allow(clippy::too_many_lines)] // one event per line, as data
fn verify_script() -> Vec<(&'static str, i64, &'static str, Value)> {
    let v = |id: &str, level: i64, sha: &str, by: &str| {
        json!({"verif_id": id, "task_id": "1.1", "level": level, "sha": sha,
               "files": ["src/schema.rs"], "evidence": "ok", "by": by})
    };
    vec![
        (
            "s1",
            0,
            "session_started",
            json!({"cwd": "/a", "worktree": "/a", "source": "startup"}),
        ),
        (
            "s1",
            1,
            "task_claimed",
            json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        ),
        (
            "s1",
            2,
            "claimed_done",
            json!({"task_id": "1.1", "evidence": "tests pass", "files": ["src/schema.rs"],
                   "symbols": ["migrate"]}),
        ),
        ("s1", 3, "verified", v("v1", 1, "aaa", "auto")),
        (
            "s1",
            4,
            "verify_nudged",
            json!({"task_id": "1.1", "level": 1}),
        ),
        (
            "s1",
            5,
            "verify_nudged",
            json!({"task_id": "1.1", "level": 1}),
        ),
        (
            "s1",
            6,
            "needs_owner",
            json!({"kind": "verify", "task_id": "1.1"}),
        ),
        ("s1", 7, "verified", v("v2", 2, "aaa", "rerun")),
        (
            "s1",
            8,
            "verification_failed",
            json!({"task_id": "1.1", "level": 4, "evidence": "broken", "by": "owner"}),
        ),
        (
            "s1",
            9,
            "task_claimed",
            json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        ),
        (
            "s1",
            10,
            "claimed_done",
            json!({"task_id": "1.1", "evidence": "fixed"}),
        ),
        ("s1", 11, "verified", v("v3", 3, "bbb", "agent")),
        (
            "s1",
            12,
            "squashed",
            json!({"branch": "feat", "old_shas": ["aaa", "bbb"], "new_sha": "ccc",
                   "backup_ref": "refs/bridge-screen/backup/feat/1"}),
        ),
        (
            "s1",
            13,
            "verification_stale",
            json!({"verif_id": "v3", "task_id": "1.1", "from_level": 3, "to_level": 2,
                   "head_sha": "ddd", "changed": ["src/schema.rs"]}),
        ),
        ("s1", 14, "precompact", json!({})),
        (
            "s1",
            15,
            "decision",
            json!({"decision_id": "D1", "task_id": "1.1", "what": "Use SQLite",
                   "why": "one file", "alternatives": ["Postgres"]}),
        ),
        (
            "s1",
            16,
            "decision",
            json!({"decision_id": "D1", "task_id": "1.1", "what": "use sqlite",
                   "why": "one file, WAL", "alternatives": ["sled"]}),
        ),
        (
            "s1",
            17,
            "decision",
            json!({"decision_id": "D2", "task_id": null, "what": "Events are append-only",
                   "why": "rebuild", "alternatives": [], "supersedes": "D1"}),
        ),
        ("s1", 18, "decision_asked", json!({})),
        (
            "owner",
            19,
            "config_changed",
            json!({"scope": "repo", "key": "test_command", "old": null,
                   "new": "cargo test"}),
        ),
        (
            "owner",
            20,
            "config_changed",
            json!({"scope": "repo", "key": "test_timeout_s", "old": null, "new": 60}),
        ),
    ]
}

#[test]
fn rebuild_reproduces_verification_and_decision_views() {
    let mut s = Store::open_in_memory().unwrap();
    plan_edit(
        &mut s,
        &ctx("a", false),
        &[
            add(None, NodeKind::Goal, "goal"),
            add(Some("G"), NodeKind::Milestone, "core"),
            add(Some("1"), NodeKind::Task, "schema"),
        ],
    )
    .unwrap();
    let t0: Timestamp = "2026-10-14T09:00:00Z".parse().unwrap();
    s.write(|tx| {
        for (session, secs, kind, data) in verify_script() {
            let draft = EventDraft::new("a", session, kind, data);
            append_in_at(tx, draft, t0 + SignedDuration::from_secs(secs))?;
        }
        Ok(())
    })
    .unwrap();
    let before = dump(&s);
    for table in ["verifications", "decisions", "repo_settings"] {
        let prefix = format!("{table}: ");
        assert!(before.iter().any(|r| r.starts_with(&prefix)), "{table}");
    }
    let status: String = s
        .conn()
        .query_row(
            "SELECT status FROM plan_nodes WHERE repo_id = 'a' AND id = '1.1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "l2");
    s.rebuild().unwrap();
    assert_eq!(dump(&s), before);
}

/// Usage across units, a detour and a subagent: a reply logged twice, a cursor that only
/// moves forward and one restarted after a shrunk file; then two forecasts.
#[test]
fn rebuild_reproduces_usage_views() {
    let mut s = Store::open_in_memory().unwrap();
    let t0: Timestamp = "2026-10-15T09:00:00Z".parse().unwrap();
    let at = |secs: i64| (t0 + SignedDuration::from_secs(secs)).to_string();
    let reply = |id: &str, secs: i64| {
        json!({"message_id": id, "model": "claude-opus-5-5", "ts": at(secs), "input": 3,
               "output": 40, "cache_read": 900, "cache_write_5m": 20, "cache_write_1h": 5})
    };
    let script = vec![
        (
            "s1",
            0,
            "session_started",
            json!({"cwd": "/a", "source": "startup", "transcript_path": "/t/s1.jsonl"}),
        ),
        (
            "s1",
            1,
            "task_claimed",
            json!({"task_id": "1.1", "mode": "auto", "confidence": 0.9}),
        ),
        ("s1", 2, "prompt", json!({"chars": 3})),
        (
            "s1",
            10,
            "detour_opened",
            json!({"detour_id": "D1", "reason": "flaky", "blocks": "1.1",
                   "resume_cue": "rerun"}),
        ),
        ("s1", 60, "unit_stopped", json!({"unit_no": 1})),
        (
            "s1",
            60,
            "usage",
            json!({"path": "/t/s1.jsonl", "offset_after": 500,
                   "replies": [reply("m1", 5), reply("m2", 30)]}),
        ),
        (
            "s1",
            61,
            "usage",
            json!({"path": "/t/s1/subagents/agent-a.jsonl", "offset_after": 90,
                   "agent_id": "a", "agent_type": "Explore", "replies": [reply("m3", 40)]}),
        ),
        (
            "s1",
            62,
            "usage",
            json!({"path": "/t/s1.jsonl", "offset_after": 300, "replies": [reply("m2", 50)]}),
        ),
        (
            "s1",
            63,
            "usage",
            json!({"path": "/t/s1/subagents/agent-a.jsonl", "offset_after": 10,
                   "restarted": true, "agent_id": "a", "replies": []}),
        ),
        (
            "s1",
            70,
            "forecast_ran",
            json!({"p50_hours": 0.0, "p90_hours": 0.0, "remaining_min": 90, "unestimated": 1,
                   "points": 1, "hours_per_day": null,
                   "reason": "Not enough finished tasks to forecast yet — 2 more are needed"}),
        ),
        (
            "owner",
            80,
            "forecast_ran",
            json!({"p50_hours": 4.5, "p90_hours": 7.25, "p50_date": "2026-10-17",
                   "p90_date": "2026-10-19", "remaining_min": 90, "unestimated": 0,
                   "points": 3, "hours_per_day": 1.5}),
        ),
    ];
    s.write(|tx| {
        for (session, secs, kind, data) in script {
            let draft = EventDraft::new("a", session, kind, data);
            append_in_at(tx, draft, t0 + SignedDuration::from_secs(secs))?;
        }
        Ok(())
    })
    .unwrap();
    let before = dump(&s);
    assert_eq!(
        before
            .iter()
            .filter(|r| r.starts_with("usage_replies: "))
            .count(),
        3
    );
    assert!(before.iter().any(|r| r.starts_with("transcript_cursors: ")));
    assert_eq!(
        before
            .iter()
            .filter(|r| r.starts_with("forecast_history: "))
            .count(),
        2
    );
    s.rebuild().unwrap();
    assert_eq!(dump(&s), before);
}

#[test]
fn rebuild_of_empty_store_is_fine() {
    let mut s = Store::open_in_memory().unwrap();
    assert_eq!(s.rebuild().unwrap().events, 0);
}
