//! The plan tree: Goal → Milestones → Tasks → Subtasks (spec §5.3).

pub mod model;
pub mod ops;
pub mod project;
pub mod query;
pub mod render;
pub mod rollup;

pub use model::{ExternalDep, Hill, NodeKind, NodeSource, NodeStatus, PlanNode};
pub use ops::{
    EditContext, EditOutcome, PlanError, PlanOp, plan_edit, plan_edit_in, plan_edit_in_sourced,
    resolve_repo,
};
pub use query::{get_node, is_in_subtree, load_tree, next_child, would_cycle, would_cycle_across};
pub use render::{render_annotated, render_text};
pub use rollup::{Rollup, is_done, rollup_of, rollups, shown_status};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventDraft, Store};
    use serde_json::json;

    fn add(store: &mut Store, id: &str, parent: Option<&str>, kind: &str, title: &str) {
        store
            .append(EventDraft::new(
                "r",
                "owner",
                "node_added",
                json!({"id": id, "parent_id": parent, "kind": kind, "title": title,
                       "intent": if kind == "goal" || kind == "milestone" { Some("why") } else { None },
                       "estimate_min": null, "expected_files": []}),
            ))
            .unwrap();
    }

    fn sample() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        add(&mut s, "G", None, "goal", "Ship it");
        add(&mut s, "1", Some("G"), "milestone", "Core");
        add(&mut s, "1.1", Some("1"), "task", "Schema");
        add(&mut s, "1.2", Some("1"), "task", "Append");
        add(&mut s, "2", Some("G"), "milestone", "Screens");
        s
    }

    #[test]
    fn kind_rules() {
        assert!(NodeKind::Goal.allows_child(NodeKind::Milestone));
        assert!(NodeKind::Goal.allows_child(NodeKind::Task));
        assert!(NodeKind::Milestone.allows_child(NodeKind::Task));
        assert!(NodeKind::Task.allows_child(NodeKind::Subtask));
        assert!(!NodeKind::Task.allows_child(NodeKind::Task));
        assert!(!NodeKind::Subtask.allows_child(NodeKind::Subtask));
        assert!(!NodeKind::Milestone.allows_child(NodeKind::Milestone));
        assert!(NodeKind::Goal.requires_intent() && NodeKind::Milestone.requires_intent());
        assert!(!NodeKind::Task.requires_intent());
        assert_eq!(NodeKind::parse("subtask"), Some(NodeKind::Subtask));
        assert_eq!(
            NodeStatus::parse(NodeStatus::L2.as_str()),
            Some(NodeStatus::L2)
        );
        assert_eq!(Hill::parse("uphill"), Some(Hill::Uphill));
    }

    #[test]
    fn tree_is_depth_first_in_position_order() {
        let s = sample();
        let tree = load_tree(s.conn(), "r", false).unwrap();
        let ids: Vec<(&str, usize)> = tree.iter().map(|n| (n.id.as_str(), n.depth)).collect();
        assert_eq!(
            ids,
            vec![("G", 0), ("1", 1), ("1.1", 2), ("1.2", 2), ("2", 1)]
        );
        assert_eq!(next_child(s.conn(), "r", "1").unwrap(), 3);
        assert_eq!(next_child(s.conn(), "r", "G").unwrap(), 3);
    }

    #[test]
    fn edits_moves_and_fields_project() {
        let mut s = sample();
        let ev = |kind: &str, data: serde_json::Value| EventDraft::new("r", "owner", kind, data);
        s.append(ev(
            "node_edited",
            json!({"id": "1.1", "title": "Schema v1", "expected_files": ["a.rs"]}),
        ))
        .unwrap();
        s.append(ev("estimate_set", json!({"id": "1.1", "estimate_min": 30})))
            .unwrap();
        s.append(ev("intent_set", json!({"id": "2", "intent": "see it"})))
            .unwrap();
        s.append(ev("hill_moved", json!({"id": "1", "hill": "downhill"})))
            .unwrap();
        s.append(ev("node_moved", json!({"id": "1.2", "new_parent_id": "2"})))
            .unwrap();
        s.append(ev("dep_added", json!({"id": "1.2", "depends_on": "1.1"})))
            .unwrap();

        let n = get_node(s.conn(), "r", "1.1").unwrap().unwrap();
        assert_eq!(n.title, "Schema v1");
        assert_eq!(n.expected_files, vec!["a.rs".to_string()]);
        assert_eq!(n.estimate_min, Some(30));
        assert_eq!(
            get_node(s.conn(), "r", "2")
                .unwrap()
                .unwrap()
                .intent
                .as_deref(),
            Some("see it")
        );
        assert_eq!(
            get_node(s.conn(), "r", "1").unwrap().unwrap().hill,
            Some(Hill::Downhill)
        );
        let moved = get_node(s.conn(), "r", "1.2").unwrap().unwrap();
        assert_eq!(moved.parent_id.as_deref(), Some("2"));
        assert_eq!(moved.depends_on, vec!["1.1".to_string()]);

        s.append(ev("dep_removed", json!({"id": "1.2", "depends_on": "1.1"})))
            .unwrap();
        let deps = get_node(s.conn(), "r", "1.2").unwrap().unwrap().depends_on;
        assert_eq!(deps, Vec::<String>::new());
    }

    #[test]
    fn removal_drops_subtree_and_its_deps() {
        let mut s = sample();
        s.append(EventDraft::new(
            "r",
            "owner",
            "dep_added",
            json!({"id": "2", "depends_on": "1.1"}),
        ))
        .unwrap();
        s.append(EventDraft::new(
            "r",
            "owner",
            "node_removed",
            json!({"id": "1"}),
        ))
        .unwrap();
        let live: Vec<String> = load_tree(s.conn(), "r", false)
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(live, vec!["G", "2"]);
        let all = load_tree(s.conn(), "r", true).unwrap();
        assert!(
            all.iter()
                .filter(|n| n.id.starts_with('1'))
                .all(|n| n.status == NodeStatus::Dropped)
        );
        let deps = get_node(s.conn(), "r", "2").unwrap().unwrap().depends_on;
        assert_eq!(deps, Vec::<String>::new());
    }

    #[test]
    fn cycle_and_subtree_checks() {
        let mut s = sample();
        s.append(EventDraft::new(
            "r",
            "owner",
            "dep_added",
            json!({"id": "1.2", "depends_on": "1.1"}),
        ))
        .unwrap();
        assert!(would_cycle(s.conn(), "r", "1.1", "1.2").unwrap());
        assert!(!would_cycle(s.conn(), "r", "2", "1.2").unwrap());
        assert!(would_cycle(s.conn(), "r", "1.1", "1.1").unwrap());
        assert!(is_in_subtree(s.conn(), "r", "1", "1.2").unwrap());
        assert!(is_in_subtree(s.conn(), "r", "1", "1").unwrap());
        assert!(!is_in_subtree(s.conn(), "r", "1", "2").unwrap());
    }

    #[test]
    fn a_task_remembers_the_branch_it_was_last_claimed_on() {
        let mut s = sample();
        for (sid, branch) in [("s1", "feat/a"), ("s2", "feat/b")] {
            s.append(EventDraft::new(
                "r",
                sid,
                "session_started",
                json!({"cwd": "/x", "branch": branch, "source": "startup"}),
            ))
            .unwrap();
        }
        let claim = |sid: &str| {
            EventDraft::new(
                "r",
                sid,
                "task_claimed",
                json!({"task_id": "1.1", "mode": "confirmed", "confidence": 1.0}),
            )
            .task("1.1")
        };
        s.append(claim("s1")).unwrap();
        let n = get_node(s.conn(), "r", "1.1").unwrap().unwrap();
        assert_eq!(n.branch.as_deref(), Some("feat/a"));
        s.append(claim("s2")).unwrap();
        let n = get_node(s.conn(), "r", "1.1").unwrap().unwrap();
        assert_eq!(n.branch.as_deref(), Some("feat/b"));
        assert_eq!(n.source, NodeSource::Manual);
    }

    #[test]
    fn node_source_round_trips() {
        for src in [
            NodeSource::Manual,
            NodeSource::Init,
            NodeSource::File {
                file: "docs/p.md".into(),
                task: 3,
                step: Some(2),
            },
            NodeSource::File {
                file: "docs/p.md".into(),
                task: 3,
                step: None,
            },
        ] {
            assert_eq!(NodeSource::from_column(&src.to_column()), src);
            let v = serde_json::to_value(&src).unwrap();
            assert_eq!(serde_json::from_value::<NodeSource>(v).unwrap(), src);
        }
        assert_eq!(
            NodeSource::File {
                file: "a.md".into(),
                task: 1,
                step: None
            }
            .to_column(),
            r#"{"file":"a.md","task":1}"#
        );
        assert_eq!(NodeSource::from_column("garbage"), NodeSource::Manual);
    }

    fn repos(s: &Store, rows: &[(&str, &str, &str)]) {
        for (id, name, root) in rows {
            s.conn()
                .execute(
                    "INSERT INTO repos (id, name, root, created_at) VALUES (?1, ?2, ?3, 't')",
                    rusqlite::params![id, name, root],
                )
                .unwrap();
        }
    }

    fn two_repos() -> Store {
        let mut s = sample();
        for (id, parent, kind, title) in [
            ("G", None, "goal", "API"),
            ("2", Some("G"), "milestone", "Auth"),
            ("2.1", Some("2"), "task", "Login endpoint"),
        ] {
            s.append(EventDraft::new(
                "b",
                "owner",
                "node_added",
                json!({"id": id, "parent_id": parent, "kind": kind, "title": title,
                       "intent": (kind != "task").then_some("why")}),
            ))
            .unwrap();
        }
        repos(&s, &[("r", "web", "/src/web"), ("b", "api", "/src/api")]);
        s
    }

    fn ctx(repo: &str) -> EditContext {
        EditContext {
            repo_id: repo.into(),
            session_id: "owner".into(),
            actor: crate::store::Actor::default(),
            reason: None,
            replan: false,
        }
    }

    fn dep(id: &str, on: &str, repo: Option<&str>) -> PlanOp {
        PlanOp::AddDep {
            id: id.into(),
            depends_on: on.into(),
            repo: repo.map(str::to_string),
        }
    }

    #[test]
    fn cross_repo_dep_by_name() {
        let mut s = two_repos();
        plan_edit(&mut s, &ctx("r"), &[dep("1.1", "2.1", Some("api"))]).unwrap();
        let n = get_node(s.conn(), "r", "1.1").unwrap().unwrap();
        assert_eq!(n.depends_on, Vec::<String>::new());
        assert_eq!(
            n.external_deps,
            vec![ExternalDep {
                repo_id: "b".into(),
                repo_name: "api".into(),
                id: "2.1".into()
            }]
        );
        // By id too; the same link twice is refused; a missing task is not found.
        let again = plan_edit(&mut s, &ctx("r"), &[dep("1.1", "2.1", Some("b"))]);
        assert!(
            again
                .unwrap_err()
                .to_string()
                .contains("already depends on api:2.1")
        );
        let missing = plan_edit(&mut s, &ctx("r"), &[dep("1.1", "9.9", Some("api"))]);
        assert!(missing.unwrap_err().to_string().contains("9.9 not found"));
        // Its own repo by name is a plain link.
        plan_edit(&mut s, &ctx("r"), &[dep("1.2", "1.1", Some("web"))]).unwrap();
        assert_eq!(
            get_node(s.conn(), "r", "1.2").unwrap().unwrap().depends_on,
            vec!["1.1".to_string()]
        );
        // Removal names the repo the same way; removing the target drops the link.
        plan_edit(
            &mut s,
            &ctx("r"),
            &[PlanOp::RemoveDep {
                id: "1.1".into(),
                depends_on: "2.1".into(),
                repo: Some("api".into()),
            }],
        )
        .unwrap();
        assert_eq!(
            get_node(s.conn(), "r", "1.1")
                .unwrap()
                .unwrap()
                .external_deps,
            vec![]
        );
        plan_edit(&mut s, &ctx("r"), &[dep("1.1", "2.1", Some("api"))]).unwrap();
        s.append(EventDraft::new(
            "b",
            "owner",
            "node_removed",
            json!({"id": "2"}),
        ))
        .unwrap();
        assert_eq!(
            get_node(s.conn(), "r", "1.1")
                .unwrap()
                .unwrap()
                .external_deps,
            vec![]
        );
    }

    #[test]
    fn ambiguous_repo_name_is_refused() {
        let mut s = two_repos();
        repos(&s, &[("c", "api", "/other/api")]);
        let err = plan_edit(&mut s, &ctx("r"), &[dep("1.1", "2.1", Some("api"))]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "op 0: api matches several repos: /other/api, /src/api"
        );
        let err = plan_edit(&mut s, &ctx("r"), &[dep("1.1", "2.1", Some("nope"))]).unwrap_err();
        assert_eq!(err.to_string(), "op 0: no repo named nope");
    }

    #[test]
    fn cross_repo_cycle_is_refused() {
        let mut s = two_repos();
        plan_edit(&mut s, &ctx("r"), &[dep("1.1", "2.1", Some("api"))]).unwrap();
        let err = plan_edit(&mut s, &ctx("b"), &[dep("2.1", "1.1", Some("web"))]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "op 0: 2.1 depending on web:1.1 would create a cycle"
        );
        assert!(would_cycle_across(s.conn(), "b", "2.1", "r", "1.1").unwrap());
        assert!(!would_cycle_across(s.conn(), "b", "2.1", "r", "1.2").unwrap());
    }

    #[test]
    fn other_repos_are_separate() {
        let s = sample();
        assert_eq!(load_tree(s.conn(), "other", true).unwrap(), Vec::new());
    }
}
