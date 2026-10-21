//! Owner-facing words for states the model keeps as codes, shared by `status`, `watch`
//! and the MCP report so every screen matches the dashboard and the card. Display only:
//! the model, the API and the agent's tool inputs keep `uphill`/`downhill` and
//! `on_task`/`detour`/`drift`.

use bridge_screen_core::plan::{Hill, Rollup};
use bridge_screen_core::tangent::{Label, LabelSource};
use bridge_screen_core::view::MilestoneRow;

/// A milestone's hill as an icon and a word, or `None` when there is none or the
/// milestone is finished.
pub fn hill(hill: Option<Hill>, done_pct: u32) -> Option<(&'static str, &'static str)> {
    if done_pct >= 100 {
        return None;
    }
    match hill? {
        Hill::Uphill => Some(("?", "exploring")),
        Hill::Downhill => Some(("→", "executing")),
    }
}

/// A milestone's hill word, hidden once it is done by % or by its tasks.
pub fn milestone_hill(m: &MilestoneRow) -> Option<(&'static str, &'static str)> {
    let pct = if m.status == Rollup::Done {
        100
    } else {
        m.done_pct
    };
    hill(m.hill, pct)
}

/// A finished or troubled milestone (or goal) as a short mark: `✓ done`,
/// `✗ needs attention`; nothing otherwise.
pub fn rollup_mark(r: Rollup) -> Option<&'static str> {
    match r {
        Rollup::Done => Some("✓ done"),
        Rollup::NeedsAttention => Some("✗ needs attention"),
        Rollup::Todo | Rollup::InProgress => None,
    }
}

/// What the hill word means (the dashboard's tooltip).
pub fn hill_tip(hill: Hill) -> &'static str {
    match hill {
        Hill::Uphill => "Unknowns remain — this % may still change",
        Hill::Downhill => "Path is clear — this % is reliable",
    }
}

/// A unit label in words: `✓ On task · agent`, `↷ Detour · auto`, `Unlabelled`.
pub fn unit_label(label: Option<Label>, source: Option<LabelSource>) -> String {
    let suffix = match source {
        Some(LabelSource::Agent | LabelSource::Declared) => " · agent",
        Some(LabelSource::Heuristic) => " · auto",
        Some(LabelSource::Owner) => " · owner",
        None => "",
    };
    match label {
        Some(Label::OnTask) => format!("✓ On task{suffix}"),
        Some(Label::Detour) => format!("↷ Detour{suffix}"),
        Some(Label::Drift) => format!("▲ Drift{suffix}"),
        Some(Label::Requested) => format!("★ Asked for{suffix}"),
        None => "Unlabelled".to_string(),
    }
}

/// A plan node's status code (a task's own, or a parent's roll-up) in plain words.
pub fn status(code: &str) -> &'static str {
    match code {
        "todo" => "to do",
        "active" => "being worked on",
        "claimed" => "said done, not checked yet",
        "l1" => "code written, tests not run",
        "l2" => "tests pass",
        "l3" => "tried end to end",
        "l4" => "you checked it",
        "failed" => "failed a check",
        "dropped" => "dropped",
        "in_progress" => "in progress",
        "done" => "done",
        "needs_attention" => "needs attention",
        _ => "unknown",
    }
}

/// `<1 min`, `6 min`.
pub fn mins(secs: i64) -> String {
    if secs < 60 {
        "<1 min".to_string()
    } else {
        format!("{} min", (secs + 30) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hill_words_replace_uphill_and_downhill() {
        assert_eq!(hill(Some(Hill::Uphill), 40), Some(("?", "exploring")));
        assert_eq!(hill(Some(Hill::Downhill), 40), Some(("→", "executing")));
        assert_eq!(hill(Some(Hill::Downhill), 100), None);
        assert_eq!(hill(None, 10), None);
        assert_eq!(
            hill_tip(Hill::Uphill),
            "Unknowns remain — this % may still change"
        );
    }

    #[test]
    fn statuses_read_as_words() {
        assert_eq!(status("l2"), "tests pass");
        assert_eq!(status("done"), "done");
        assert_eq!(status("needs_attention"), "needs attention");
        assert_eq!(status("in_progress"), "in progress");
    }

    #[test]
    fn unit_labels_read_as_words() {
        let l = |l, s| unit_label(Some(l), Some(s));
        assert_eq!(l(Label::OnTask, LabelSource::Agent), "✓ On task · agent");
        assert_eq!(l(Label::OnTask, LabelSource::Heuristic), "✓ On task · auto");
        assert_eq!(l(Label::OnTask, LabelSource::Owner), "✓ On task · owner");
        assert_eq!(l(Label::Detour, LabelSource::Declared), "↷ Detour · agent");
        assert_eq!(l(Label::Drift, LabelSource::Heuristic), "▲ Drift · auto");
        assert_eq!(
            l(Label::Requested, LabelSource::Declared),
            "★ Asked for · agent"
        );
        assert_eq!(unit_label(None, None), "Unlabelled");
        assert_eq!(mins(20), "<1 min");
        assert_eq!(mins(360), "6 min");
    }
}
