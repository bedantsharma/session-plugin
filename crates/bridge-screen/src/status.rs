//! `bridge-screen status`: live sessions per project, in about a dozen lines (spec §10.3).
//! Plain text for now; colours arrive with the screens.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::Result;
use bridge_screen_core::claim::task_title;
use bridge_screen_core::config::Config;
use bridge_screen_core::plan::{NodeStatus, get_node};
use bridge_screen_core::session::{self, Session, SessionState};
use bridge_screen_core::store::{Store, repo_name};
use bridge_screen_core::tangent::{self, Label, NavState};

/// Width of the "state + task" column, and the most of it the text may use.
const TASK_COLUMN: usize = 32;
const TASK_TEXT_MAX: usize = 30;

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Red drift first, then other needs-you, then working, then idle.
fn state_rank(s: &Session) -> u8 {
    if s.nav_state == NavState::Red {
        return 0;
    }
    match s.state {
        SessionState::NeedsOwner => 1,
        SessionState::Working => 2,
        SessionState::Idle => 3,
        SessionState::Ended => 4,
    }
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut short: String = text.chars().take(max - 1).collect();
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

/// ` · tuned drift .35–.65, tolerance 25%` when the repo's thresholds were auto-tuned.
fn tuned_suffix(store: &Store, repo_id: &str, cfg: &Config) -> Result<String> {
    let th = tangent::thresholds(store.conn(), repo_id, cfg)?;
    if th == tangent::Thresholds::from_config(cfg) {
        return Ok(String::new());
    }
    let short = |v: f64| {
        let t = format!("{v:.2}");
        t.strip_prefix('0').map_or(t.clone(), str::to_string)
    };
    Ok(format!(
        "  · tuned drift {}–{}, tolerance {}%",
        short(th.lower),
        short(th.upper),
        th.tolerance_pct
    ))
}

/// The status text for every live session.
pub fn render(store: &Store, cfg: &Config) -> Result<String> {
    let conn = store.conn();
    let live = session::list_live(conn, None)?;
    if live.is_empty() {
        return Ok("bridge-screen · no live sessions\n".to_string());
    }
    // Group by repo display name (then id, in case two repos share a folder name).
    let mut repos: BTreeMap<(String, String), Vec<Session>> = BTreeMap::new();
    for s in live.iter().cloned() {
        let name = repo_name(conn, &s.repo_id)?.unwrap_or_else(|| s.repo_id.clone());
        repos.entry((name, s.repo_id.clone())).or_default().push(s);
    }
    let mut out = format!(
        "bridge-screen · {} · {}\n",
        plural(live.len(), "live session", "live sessions"),
        plural(repos.len(), "project", "projects")
    );
    for ((name, repo_id), mut sessions) in repos {
        match get_node(conn, &repo_id, "G")?.filter(|g| g.status != NodeStatus::Dropped) {
            Some(goal) => {
                let _ = writeln!(
                    out,
                    "{name}  goal: {}{}",
                    goal.title,
                    tuned_suffix(store, &repo_id, cfg)?
                );
            }
            None => {
                let _ = writeln!(out, "{name}  no plan yet");
            }
        }
        // `list_live` is newest first; a stable sort keeps that within each state.
        sessions.sort_by_key(state_rank);
        for s in &sessions {
            out.push_str(&session_line(store, s)?);
        }
    }
    Ok(out)
}

fn session_line(store: &Store, s: &Session) -> Result<String> {
    let short: String = s.session_id.chars().take(6).collect();
    let label = match s.state {
        _ if s.nav_state == NavState::Red => "needs you",
        SessionState::NeedsOwner => "needs you",
        SessionState::Working => "working",
        SessionState::Idle => "idle",
        SessionState::Ended => "ended",
    };
    let task = match &s.active_task {
        Some(id) => {
            let title = task_title(store.conn(), &s.repo_id, id)?.unwrap_or_default();
            format!("{id} {title}")
        }
        None => "(no task)".to_string(),
    };
    let left = cut(&format!("{label:<10}{task}"), TASK_TEXT_MAX);
    let count = usize::try_from(s.unit_count).unwrap_or(0);
    let units = plural(count, "unit", "units");
    let minutes = s.active_secs / 60;
    let nav = nav_text(store, s)?;
    Ok(format!(
        "  S {short}  {left:<TASK_COLUMN$}{units:<7} · {minutes}m · {nav}\n"
    ))
}

/// The navigator light in words — `on task`, `detour 25m`, `drift ⚠ 40m` — plus the
/// current detour and a `⚠` when the agent disagreed with the heuristic.
fn nav_text(store: &Store, s: &Session) -> Result<String> {
    let last = session::units(store.conn(), &s.session_id)?
        .into_iter()
        .rev()
        .find(|u| u.stopped_at.is_some());
    let away = s.away_secs / 60;
    let drifting = last.as_ref().is_some_and(|u| u.label == Some(Label::Drift));
    let mut text = if s.current_detour.is_none() && drifting {
        format!("drift ⚠ {away}m")
    } else if s.current_detour.is_some() || s.away_secs > 0 {
        format!("detour {away}m")
    } else {
        "on task".to_string()
    };
    if let Some(id) = &s.current_detour
        && let Some(d) = tangent::get_detour(store.conn(), id)?
    {
        let _ = write!(text, " · {} \"{}\"", d.label(), cut(&d.reason, 30));
    }
    if !drifting && last.is_some_and(|u| u.disagreement) {
        text.push_str(" ⚠");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::cut;

    #[test]
    fn cut_keeps_short_text_and_marks_long_text() {
        assert_eq!(cut("short", 30), "short");
        assert_eq!(cut("abcdef", 4), "abc…");
        assert_eq!(cut("ab def", 4), "ab…");
    }
}
