//! Plain-text rendering of the plan tree for the CLI.

use std::fmt::Write as _;

use super::model::{Hill, PlanNode};

pub fn render_text(nodes: &[PlanNode]) -> String {
    let mut out = String::new();
    for node in nodes {
        let indent = "  ".repeat(node.depth);
        let _ = write!(
            out,
            "{indent}{}  {}  [{}]",
            node.id,
            node.title,
            node.status.as_str()
        );
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
    fn empty_tree_renders_nothing() {
        assert_eq!(render_text(&[]), "");
    }
}
