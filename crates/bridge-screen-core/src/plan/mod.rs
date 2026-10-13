//! The plan tree: Goal → Milestones → Tasks → Subtasks (spec §5.3).

pub mod model;
pub mod project;
pub mod query;

pub use model::{Hill, NodeKind, NodeStatus, PlanNode};
pub use query::{get_node, is_in_subtree, load_tree, next_child, would_cycle};

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
    fn other_repos_are_separate() {
        let s = sample();
        assert_eq!(load_tree(s.conn(), "other", true).unwrap(), Vec::new());
    }
}
