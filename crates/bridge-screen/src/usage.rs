//! `bridge-screen usage`: tokens and cost in plain words (spec §9.2), and the owner's
//! catch-up of transcripts the hooks haven't fully read yet; `bridge-screen forecast`.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Result, bail};
use bridge_screen_core::claim::task_title;
use bridge_screen_core::config::Config;
use bridge_screen_core::paths::Home;
use bridge_screen_core::repo;
use bridge_screen_core::telemetry::{self, Filter, SplitBy, Tokens};
use rusqlite::Connection;

use crate::cli::UsageBy;
use crate::commands::open_store;

pub fn run(
    dir: &Path,
    session: Option<&str>,
    task: Option<&str>,
    by: UsageBy,
    catch_up: bool,
) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let mut text = String::new();
    if catch_up {
        let done = telemetry::catch_up(&mut store, &repo.id)?;
        if done.replies == 0 {
            let _ = writeln!(
                text,
                "Nothing new: {} transcript(s) of {} are fully read.",
                done.files, repo.name
            );
        } else {
            let _ = writeln!(
                text,
                "Read {} transcript(s) of {}: {} replies, {} tokens added.",
                done.files,
                repo.name,
                done.replies,
                telemetry::tokens_text(done.tokens)
            );
        }
    }
    let cfg = Config::load(&home.config_path())?;
    let conn = store.conn();
    let session = match session {
        Some(prefix) => Some(session_by_prefix(conn, &repo.id, prefix)?),
        None => None,
    };
    let filter = Filter {
        session: session.clone(),
        task: task.map(str::to_string),
    };
    let split_by = match by {
        UsageBy::Model => SplitBy::Model,
        UsageBy::Task => SplitBy::Task,
        UsageBy::Session => SplitBy::Session,
        UsageBy::Agent => SplitBy::Agent,
    };
    let rows = telemetry::split(conn, &repo.id, &filter, split_by)?;
    let mut all = Tokens::default();
    for (key, tokens) in &rows {
        let label = row_label(conn, &repo.id, by, key.as_deref())?;
        let _ = writeln!(
            text,
            "  {}",
            line(&label, tokens, &cfg, by != UsageBy::Model)
        );
        all.merge(tokens);
    }
    let scope = match (&filter.task, &session) {
        (Some(t), _) => task_label(conn, &repo.id, t)?,
        (None, Some(s)) => format!("session {}", short(s)),
        (None, None) => repo.name.clone(),
    };
    if rows.is_empty() {
        let _ = writeln!(text, "No usage recorded for {scope} yet.");
    } else {
        let _ = writeln!(
            text,
            "{}",
            line(&format!("Total for {scope}"), &all, &cfg, false)
        );
    }
    for model in telemetry::cost(&all, &cfg.prices).unpriced_models {
        let _ = writeln!(
            text,
            "No price for {model}: add one under [prices] in config.toml."
        );
    }
    print!("{text}");
    Ok(())
}

/// `Login form (2.3): 1.2M tokens · $4.10 (Opus 5.5 $3.90, Haiku 4.5 $0.20)`; the split
/// by model only when asked for and there is more than one.
fn line(label: &str, tokens: &Tokens, cfg: &Config, per_model: bool) -> String {
    let cost = telemetry::cost(tokens, &cfg.prices);
    let mut out = format!(
        "{label}: {} tokens · {}",
        telemetry::tokens_text(tokens.sum.total()),
        telemetry::cost_text(&cost)
    );
    if per_model && tokens.by_model.len() > 1 {
        let parts: Vec<String> = tokens
            .by_model
            .keys()
            .map(|m| match cost.by_model.get(m) {
                Some(usd) => format!("{} {}", telemetry::model_text(m), telemetry::usd_text(*usd)),
                None => format!("{} cost unknown", telemetry::model_text(m)),
            })
            .collect();
        let _ = write!(out, " ({})", parts.join(", "));
    }
    out
}

fn short(id: &str) -> String {
    id.chars().take(6).collect()
}

/// `Login form (2.3)`, or the bare id when the task is gone from the plan.
fn task_label(conn: &Connection, repo: &str, id: &str) -> Result<String> {
    Ok(match task_title(conn, repo, id)? {
        Some(title) => format!("{title} ({id})"),
        None => id.to_string(),
    })
}

fn row_label(conn: &Connection, repo: &str, by: UsageBy, key: Option<&str>) -> Result<String> {
    Ok(match (by, key) {
        (UsageBy::Task, Some(id)) => task_label(conn, repo, id)?,
        (UsageBy::Task, None) => "No task".to_string(),
        (UsageBy::Model, Some(m)) => telemetry::model_text(m),
        (UsageBy::Agent, Some(t)) => t.to_string(),
        (UsageBy::Agent, None) => "Main agent".to_string(),
        (UsageBy::Session, Some(s)) => {
            let branch: Option<String> = conn
                .query_row(
                    "SELECT branch FROM sessions WHERE session_id = ?1",
                    [s],
                    |r| r.get(0),
                )
                .unwrap_or(None);
            match branch {
                Some(b) => format!("{} ({b})", short(s)),
                None => short(s),
            }
        }
        (_, None) => "Unknown".to_string(),
    })
}

/// The one session of this repo whose id starts with `prefix` (ended ones too).
fn session_by_prefix(conn: &Connection, repo: &str, prefix: &str) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT session_id FROM sessions WHERE repo_id = ?1 AND substr(session_id, 1, ?3) = ?2",
    )?;
    let len = i64::try_from(prefix.chars().count()).unwrap_or(i64::MAX);
    let ids: Vec<String> = stmt
        .query_map(rusqlite::params![repo, prefix, len], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    match ids.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no session of this repo starts with {prefix}"),
        many => bail!("session prefix {prefix} is ambiguous: {}", many.join(", ")),
    }
}

/// `bridge-screen forecast`: the range in plain words and what it rests on.
pub fn forecast(dir: &Path) -> Result<()> {
    use bridge_screen_core::forecast as fc;
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let store = open_store(&home)?;
    store.register_repo(&repo)?;
    let now = jiff::Timestamp::now();
    let tz = jiff::tz::TimeZone::system();
    let f = fc::run(store.conn(), &repo.id, now, &tz)?;
    print!("{}", forecast_text(&repo.name, &f, now, &tz));
    Ok(())
}

/// The forecast's text, as `bridge-screen forecast` prints it.
pub fn forecast_text(
    name: &str,
    f: &bridge_screen_core::forecast::Forecast,
    now: jiff::Timestamp,
    tz: &jiff::tz::TimeZone,
) -> String {
    use bridge_screen_core::forecast as fc;
    let mut text = String::new();
    if f.dates().is_some() {
        let _ = writeln!(text, "{name}: {}", fc::range_text(f, now, tz));
        let _ = writeln!(text, "  ({})", fc::RANGE_HELP);
    } else {
        let _ = writeln!(
            text,
            "{name}: no dates yet. {}.",
            fc::range_text(f, now, tz)
        );
    }
    if f.points > 0 {
        let mut basis = fc::basis_text(f);
        if let Some(first) = basis.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        let _ = writeln!(text, "{basis}.");
    }
    #[allow(clippy::cast_precision_loss)] // minutes of estimate, small
    let remaining_h = f.remaining_min as f64 / 60.0;
    let mut left = format!("{} of estimated work left", fc::hours_text(remaining_h));
    if f.dates().is_some() || f.reason.as_deref() == Some(NO_ACTIVITY) {
        let _ = write!(
            left,
            "; likely {} of agent time, up to {}",
            fc::hours_text(f.p50_hours),
            fc::hours_text(f.p90_hours)
        );
    }
    if let Some(per_day) = f.hours_per_day {
        let _ = write!(left, " (lately {} a day)", fc::hours_text(per_day));
    }
    let _ = writeln!(text, "{left}.");
    match f.unestimated {
        0 => {}
        1 => {
            let _ = writeln!(text, "1 task has no estimate and isn't counted.");
        }
        n => {
            let _ = writeln!(text, "{n} tasks have no estimate and aren't counted.");
        }
    }
    text
}

const NO_ACTIVITY: &str = "No recent activity to turn hours into dates";
