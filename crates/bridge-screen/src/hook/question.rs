//! Did the agent end its turn with a question to the owner? Pure string checks: no git,
//! nothing stored (spec §10.1).

/// Claude Code's built-in tool for asking the owner.
pub const ASK_TOOL: &str = "AskUserQuestion";

/// `msg` ends with a question: its last non-empty line outside fenced code blocks ends
/// with `?` (or a full-width `？`) once trailing emphasis, backticks, closing brackets and
/// quotes are stripped.
pub fn ends_with_question(msg: Option<&str>) -> bool {
    let Some(msg) = msg else {
        return false;
    };
    let mut fenced = false;
    let mut last: Option<&str> = None;
    for line in msg.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if !fenced && !t.is_empty() {
            last = Some(t);
        }
    }
    let Some(line) = last else {
        return false;
    };
    // Quote and list markers in front never decide; what's left at the end does.
    let line = line.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
    let line = line.trim_end_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '*' | '_' | '`' | ')' | ']' | '"' | '\'' | '”' | '’' | '»'
            )
    });
    line.ends_with('?') || line.ends_with('？')
}

#[cfg(test)]
mod tests {
    use super::ends_with_question;

    #[test]
    fn question_endings() {
        let yes = [
            "Should I also update the docs?",
            "Done.\n\nShall I push the branch?\n\n",
            "Which one do you want: **A or B?**",
            "Use `foo` or `bar?`",
            "Ready to merge (or wait for CI?)",
            "> Is this right?",
            "- Keep the old API?",
            "続けますか？",
        ];
        for m in yes {
            assert!(ends_with_question(Some(m)), "{m}");
        }
        let no = [
            "",
            "   \n  ",
            "Fixed it.",
            "Did it work? Yes, the tests pass.",
            "Should I go on?\nI went ahead and pushed it.",
            "Here is the code:\n```rust\nfn ok() -> bool { a? }\n// why?\n```",
            "Done.\n```\nwhat?\n```\n",
        ];
        for m in no {
            assert!(!ends_with_question(Some(m)), "{m}");
        }
        assert!(!ends_with_question(None));
    }
}
