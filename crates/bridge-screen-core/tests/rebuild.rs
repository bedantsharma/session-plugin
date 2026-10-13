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
        ("plan_deps", "repo_id, node_id, depends_on"),
        ("sessions", "session_id"),
        ("units", "session_id, unit_no"),
        ("task_time", "repo_id, task_id"),
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
    events += 1 + 3 + 1;
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

#[test]
fn rebuild_of_empty_store_is_fine() {
    let mut s = Store::open_in_memory().unwrap();
    assert_eq!(s.rebuild().unwrap().events, 0);
}
