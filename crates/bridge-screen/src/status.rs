//! `bridge-screen status`: the glance (spec §10.1) in about a dozen lines of text (§10.3).
//!
//! A text renderer of [`view::Glance`] only: ordering, state words and lights come from
//! the model. ANSI colours mark the §10.4 roles when stdout is a terminal, `NO_COLOR` is
//! unset or empty and `TERM` isn't `dumb`; otherwise the output is plain text.

use std::fmt::Write as _;
use std::io::IsTerminal as _;

use bridge_screen_core::view::{FlagKind, Glance, NeedsKind, ProjectRow, SessionRow};

/// Width of the "state + task" column, and the most of it the text may use.
const TASK_COLUMN: usize = 32;
const TASK_TEXT_MAX: usize = 30;

/// How to render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// ANSI colours for the §10.4 roles.
    pub color: bool,
    /// The running dashboard's address (without its token), if any.
    pub dashboard: Option<String>,
}

/// Colour is allowed on this stream: a terminal, `NO_COLOR` unset or empty, `TERM` not
/// `dumb` (no-color.org).
pub fn color_allowed(is_tty: bool) -> bool {
    color_with(
        is_tty,
        std::env::var_os("NO_COLOR").as_deref(),
        std::env::var_os("TERM").as_deref(),
    )
}

/// [`color_allowed`] with the environment passed in.
pub fn color_with(
    is_tty: bool,
    no_color: Option<&std::ffi::OsStr>,
    term: Option<&std::ffi::OsStr>,
) -> bool {
    let no_color = no_color.is_some_and(|v| !v.is_empty());
    let dumb = term.is_some_and(|t| t == "dumb");
    is_tty && !no_color && !dumb
}

/// Colour for `status` on stdout.
pub fn stdout_color() -> bool {
    color_allowed(std::io::stdout().is_terminal())
}

/// §10.4 roles as ANSI: clay (256-colour 173), green, amber, red.
#[derive(Debug, Clone, Copy)]
enum Role {
    Clay,
    Green,
    Amber,
    Red,
    Bold,
}

fn paint(on: bool, role: Role, text: &str) -> String {
    if !on || text.is_empty() {
        return text.to_string();
    }
    let code = match role {
        Role::Clay => "38;5;173",
        Role::Green => "32",
        Role::Amber => "33",
        Role::Red => "31",
        Role::Bold => "1",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut short: String = text.chars().take(max - 1).collect();
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

/// The status text for the glance.
pub fn render(g: &Glance, opts: &Options) -> String {
    let c = opts.color;
    let mut out = String::new();
    if g.escalated {
        let _ = writeln!(
            out,
            "{}",
            paint(
                c,
                Role::Red,
                &paint(c, Role::Bold, &format!("{} need you", g.needs_you_count))
            )
        );
    }
    let shown: Vec<&ProjectRow> = g
        .projects
        .iter()
        .filter(|p| !p.sessions.is_empty())
        .collect();
    if g.live_sessions == 0 || shown.is_empty() {
        out.push_str("bridge-screen · no live sessions\n");
    } else {
        let _ = writeln!(
            out,
            "bridge-screen · {} · {}",
            plural(g.live_sessions, "live session", "live sessions"),
            plural(shown.len(), "project", "projects")
        );
        for p in shown {
            project(&mut out, g, p, c);
        }
        for grp in &g.branch_groups {
            let _ = writeln!(
                out,
                "branch group  {}  ({})",
                crate::words::branch_group(grp),
                crate::words::branch_group_repos(grp)
            );
        }
    }
    if let Some(url) = &opts.dashboard {
        let _ = writeln!(out, "dashboard: {url} (`bridge-screen ui` prints the link)");
    }
    out
}

fn project(out: &mut String, g: &Glance, p: &ProjectRow, c: bool) {
    if let Some(goal) = &p.goal {
        let mut progress = format!(
            "  · {} · claimed {}%",
            paint(c, Role::Clay, &format!("done {}%", p.done_pct)),
            p.claimed_pct
        );
        if p.unestimated > 0 {
            let _ = write!(
                progress,
                " · {} without an estimate",
                plural(p.unestimated, "task", "tasks")
            );
        }
        if let Some(f) = &p.forecast {
            let _ = write!(progress, " · {}", f.short);
        }
        if let Some(cost) = &p.cost {
            let _ = write!(progress, " · {}", cost.text);
        }
        let tuned = p
            .tuned
            .as_ref()
            .map(|t| format!("  · {t}"))
            .unwrap_or_default();
        let _ = writeln!(out, "{}  goal: {}{progress}{tuned}", p.name, goal.title);
    } else {
        let cost = p
            .cost
            .as_ref()
            .map(|c| format!(" · {}", c.text))
            .unwrap_or_default();
        let _ = writeln!(out, "{}  no plan yet{cost}", p.name);
    }
    for model in p.cost.iter().flat_map(|c| &c.unpriced_models) {
        let _ = writeln!(
            out,
            "  no price for {model}: add one under [prices] in config.toml"
        );
    }
    if p.leftovers.open > 0 {
        let _ = writeln!(
            out,
            "  unsaved work from ended sessions: {} ({} of them for information only)",
            plural(p.leftovers.open, "file", "files"),
            p.leftovers.report_only
        );
    }
    // Needs-you order: red, then questions and waits, then claimed work the owner is asked
    // to verify (spec §8.5, §10.1), then the rest.
    let (urgent, rest): (Vec<&SessionRow>, Vec<&SessionRow>) =
        p.sessions.iter().partition(|s| s.rank <= 1);
    for s in urgent {
        out.push_str(&session_line(s, c));
    }
    let mut verify: Vec<_> = g
        .needs_you
        .iter()
        .filter(|i| i.kind == NeedsKind::VerifyOverdue && i.repo_id == p.repo_id)
        .filter_map(|i| i.task.as_ref())
        .collect();
    verify.sort_by(|a, b| a.id.cmp(&b.id));
    for task in verify {
        let what = format!("{} {}", task.id, task.title);
        let _ = writeln!(
            out,
            "  {}: check {} — then `bridge-screen verify {}` (or --broken \"what's wrong\")",
            paint(c, Role::Red, "needs you"),
            what.trim_end(),
            task.id
        );
    }
    for s in rest {
        out.push_str(&session_line(s, c));
    }
}

fn session_line(s: &SessionRow, c: bool) -> String {
    let task = match &s.task {
        Some(t) => format!("{} {}", t.id, t.title),
        None => "(no task)".to_string(),
    };
    let left = cut(&format!("{:<10}{task}", s.state_label), TASK_TEXT_MAX);
    let left = format!("{left:<TASK_COLUMN$}");
    // Colour the state word only, after padding, so the columns line up.
    let role = match s.state_label.as_str() {
        "needs you" => Some(Role::Red),
        "working" | "done" => Some(Role::Green),
        _ => None,
    };
    let left = match role {
        Some(r) if c => left.replacen(&s.state_label, &paint(c, r, &s.state_label), 1),
        _ => left,
    };
    let count = usize::try_from(s.units).unwrap_or(0);
    let units = plural(count, "unit", "units");
    let mut nav = if s.nav_text == "on task" {
        s.nav_text.clone()
    } else {
        paint(c, Role::Amber, &s.nav_text)
    };
    for f in &s.flags {
        match f.kind {
            FlagKind::CheckpointMissed
            | FlagKind::CheckpointPending
            | FlagKind::ReadbackPending
            | FlagKind::SquashFailed
            | FlagKind::HandoffUnstable => {
                let _ = write!(nav, " · {}", f.kind.text());
            }
            FlagKind::VerifyPending => {
                let _ = write!(nav, " · {}: {}", f.kind.text(), f.tasks.join(", "));
            }
        }
    }
    if let Some(w) = crate::words::waiting(&s.waiting) {
        let _ = write!(nav, " · {}", paint(c, Role::Amber, &w));
    }
    if s.asked_question {
        let _ = write!(nav, " · {}", paint(c, Role::Red, "asked you a question"));
    }
    format!(
        "  S {}  {left}{units:<7} · {}m · {nav}\n",
        s.short, s.active_min
    )
}

#[cfg(test)]
mod tests {
    use super::{color_allowed, color_with, cut};

    #[test]
    fn cut_keeps_short_text_and_marks_long_text() {
        assert_eq!(cut("short", 30), "short");
        assert_eq!(cut("abcdef", 4), "abc…");
        assert_eq!(cut("ab def", 4), "ab…");
    }

    #[test]
    fn no_colour_off_a_terminal_with_no_color_or_a_dumb_terminal() {
        use std::ffi::OsStr;
        assert!(!color_allowed(false));
        let xterm = Some(OsStr::new("xterm-256color"));
        assert!(color_with(true, None, xterm));
        assert!(
            color_with(true, Some(OsStr::new("")), xterm),
            "empty NO_COLOR is unset"
        );
        assert!(!color_with(true, Some(OsStr::new("1")), xterm));
        assert!(!color_with(true, None, Some(OsStr::new("dumb"))));
        assert!(!color_with(false, None, xterm));
    }

    #[test]
    fn colour_marks_the_roles_and_plain_text_stays_plain() {
        use bridge_screen_core::config::Config;
        use bridge_screen_core::store::{EventDraft, Store};
        let mut s = Store::open_in_memory().unwrap();
        s.append(EventDraft::new(
            "r",
            "s1",
            "session_started",
            serde_json::json!({"cwd": "/w", "source": "startup"}),
        ))
        .unwrap();
        s.append(EventDraft::new(
            "r",
            "s1",
            "nav_state_changed",
            serde_json::json!({"from": "green", "to": "red", "away_secs": 1}),
        ))
        .unwrap();
        let g =
            bridge_screen_core::view::glance(s.conn(), &Config::default(), jiff::Timestamp::now())
                .unwrap();
        let plain = super::render(&g, &super::Options::default());
        assert!(!plain.contains('\x1b'));
        let colour = super::render(
            &g,
            &super::Options {
                color: true,
                dashboard: None,
            },
        );
        assert!(colour.contains("\x1b[31mneeds you\x1b[0m"), "{colour}");
        // Same text once the escapes are removed.
        let stripped = colour
            .split('\x1b')
            .enumerate()
            .map(|(i, part)| {
                if i == 0 {
                    part
                } else {
                    part.split_once('m').map_or(part, |(_, r)| r)
                }
            })
            .collect::<String>();
        assert_eq!(stripped, plain);
    }
}
