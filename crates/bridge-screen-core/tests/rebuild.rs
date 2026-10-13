use bridge_screen_core::plan::{EditContext, Hill, NodeKind, PlanOp, plan_edit};
use bridge_screen_core::store::{Actor, Store};

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

#[test]
fn rebuild_of_empty_store_is_fine() {
    let mut s = Store::open_in_memory().unwrap();
    assert_eq!(s.rebuild().unwrap().events, 0);
}
