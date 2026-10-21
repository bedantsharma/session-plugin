//! Plain-text rendering of the plan tree for the CLI.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::BuildHasher;

use super::model::{Hill, PlanNode};

pub fn render_text(nodes: &[PlanNode]) -> String {
    render_annotated(nodes, &HashMap::new())
}

/// [`render_text`] with a marker after a node's status (`stale`, `✓owner`). A parent shows
/// the roll-up of its tasks (`done`, `in_progress`, …), never its stored `todo`.
pub fn render_annotated<S: BuildHasher>(
    nodes: &[PlanNode],
    marks: &HashMap<String, String, S>,
) -> String {
    let rolled = super::rollup::rollups(nodes);
    let mut out = String::new();
    for node in nodes {
        let indent = "  ".repeat(node.depth);
        let _ = write!(
            out,
            "{indent}{}  {}  [{}]",
            node.id,
            node.title,
            super::rollup::shown_status(node, &rolled)
        );
        if let Some(mark) = marks.get(&node.id) {
            let _ = write!(out, " {mark}");
        }
        if let Some(min) = node.estimate_min {
            let _ = write!(out, " {min}m");
        }
        match node.hill {
            Some(Hill::Uphill) => out.push_str(" ↑ uphill"),
            Some(Hill::Downhill) => out.push_str(" ↓ downhill"),
            None => {}
        }
        if !node.depends_on.is_empty() {
            let _ = write!(out, " (needs {})", node.depends_on.join(", "));
        }
        out.push('\n');
        if let Some(intent) = &node.intent {
            let _ = writeln!(out, "{indent}   ↳ {intent}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::render_text;
    use crate::plan::{Hill, NodeKind, NodeStatus, PlanNode};

    fn n(id: &str, kind: NodeKind, title: &str, depth: usize) -> PlanNode {
        PlanNode {
            id: id.into(),
            parent_id: None,
            kind,
            title: title.into(),
            intent: None,
            estimate_min: None,
            status: NodeStatus::Todo,
            hill: None,
            expected_files: vec![],
            depends_on: vec![],
            depth,
        }
    }

    #[test]
    fn renders_tree_with_details() {
        let mut goal = n("G", NodeKind::Goal, "Ship bridge-screen", 0);
        goal.intent = Some("Owner sees progress at a glance.".into());
        let mut m = n("1", NodeKind::Milestone, "Core store", 1);
        m.intent = Some("Events are the source of truth.".into());
        m.hill = Some(Hill::Uphill);
        let mut t = n("1.1", NodeKind::Task, "Schema", 2);
        t.estimate_min = Some(30);
        t.depends_on = vec!["1.2".into(), "2.1".into()];
        t.status = NodeStatus::L2;
        let expected = "\
G  Ship bridge-screen  [todo]
   ↳ Owner sees progress at a glance.
  1  Core store  [todo] ↑ uphill
     ↳ Events are the source of truth.
    1.1  Schema  [l2] 30m (needs 1.2, 2.1)
";
        assert_eq!(render_text(&[goal, m, t]), expected);
    }

    #[test]
    fn marks_follow_the_status() {
        let mut t = n("1.1", NodeKind::Task, "Schema", 0);
        t.status = NodeStatus::L4;
        let marks = std::collections::HashMap::from([("1.1".to_string(), "✓owner".to_string())]);
        assert_eq!(
            super::render_annotated(&[t], &marks),
            "1.1  Schema  [l4] ✓owner\n"
        );
    }

    #[test]
    fn parents_show_their_roll_up() {
        let goal = n("G", NodeKind::Goal, "Ship", 0);
        let mut m = n("1", NodeKind::Milestone, "Core", 1);
        m.parent_id = Some("G".into());
        let mut t = n("1.1", NodeKind::Task, "Schema", 2);
        t.parent_id = Some("1".into());
        t.status = NodeStatus::L4;
        assert_eq!(
            render_text(&[goal, m, t]),
            "G  Ship  [done]\n  1  Core  [done]\n    1.1  Schema  [l4]\n"
        );
    }

    #[test]
    fn empty_tree_renders_nothing() {
        assert_eq!(render_text(&[]), "");
    }
}
