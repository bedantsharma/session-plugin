//! The forecast (spec §9.3): when a project will likely be done, as a range.
//!
//! - **Calibration:** for each leaf verified at L2 or above (the first time, within the
//!   last 6 months), the ratio of its actual active minutes to its estimate, kept per
//!   model (the model with the most output tokens on it).
//! - **Monte Carlo:** 1000 runs; each multiplies every remaining leaf's estimate by a
//!   ratio drawn from past ones, minus the time already spent on it, and sums. P50 and
//!   P90 of the sums are agent-active hours, turned into dates with the project's recent
//!   active hours per day.
//! - **No fake forecasts:** below [`MIN_POINTS`] finished tasks, or with no recent
//!   activity, there are no dates — only the reason. Leaves without an estimate are left
//!   out and counted.
//!
//! Deterministic: the generator is seeded from the repo and the last event used, so the
//! same inputs give the same P50/P90. History: [`record`] appends a `forecast_ran` after
//! each verification at L2 or above (never from a hook), when the forecast moved.

pub mod project;
pub mod rng;

use std::collections::{BTreeMap, HashMap};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::plan::{NodeStatus, PlanNode, load_tree};
use crate::store::{EventDraft, Store};
use crate::view::leaves;

pub use project::{FORECAST_KINDS, project};

/// Monte Carlo runs per forecast.
pub const RUNS: usize = 1000;
/// Fewer finished tasks than this and the forecast refuses to give dates.
pub const MIN_POINTS: usize = 3;
/// A model's own ratios are used once it has this many; until then, the pooled ones
/// (the spec's "wide default distribution").
pub const MODEL_POINTS: usize = 6;
/// Calibration history older than this is ignored.
pub const WINDOW_DAYS: i64 = 183;
/// Recent active hours per day are taken over this many calendar days.
pub const ACTIVITY_DAYS: i64 = 14;
/// A new forecast is kept in the history only when P50 or P90 moved at least this much.
pub const CHANGE_THRESHOLD: f64 = 0.01;

/// One finished leaf: how long it took against its estimate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    pub task_id: String,
    pub title: String,
    pub estimate_min: u32,
    pub actual_min: f64,
    /// actual ÷ estimate.
    pub ratio: f64,
    pub model: String,
    /// When it first reached L2 or above.
    pub completed_at: String,
}

/// The calibration points of a repo.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Calibration {
    pub by_model: BTreeMap<String, Vec<Point>>,
    /// Every point, oldest first.
    pub pooled: Vec<Point>,
}

/// A forecast, and the `forecast_ran` event's payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Forecast {
    /// Agent-active hours left, 50% / 90% chance (0 when refused).
    pub p50_hours: f64,
    pub p90_hours: f64,
    /// `YYYY-MM-DD`, in the owner's time zone; `None` whenever `reason` is set.
    #[serde(default)]
    pub p50_date: Option<String>,
    #[serde(default)]
    pub p90_date: Option<String>,
    /// Estimated minutes of the leaves still below L2.
    pub remaining_min: u64,
    /// Leaves still below L2 that have no estimate (left out of the forecast).
    pub unestimated: usize,
    /// Finished tasks the forecast is based on.
    pub points: usize,
    /// Recent agent-active hours per active day.
    #[serde(default)]
    pub hours_per_day: Option<f64>,
    /// Why there are no dates, in plain words.
    #[serde(default)]
    pub reason: Option<String>,
    /// The mean of the ratios used ("averages 1.8× its estimates").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_ratio: Option<f64>,
}

impl Forecast {
    /// Dates to show: both, and only when nothing stands against them.
    pub fn dates(&self) -> Option<(&str, &str)> {
        if self.reason.is_some() {
            return None;
        }
        Some((self.p50_date.as_deref()?, self.p90_date.as_deref()?))
    }
}

fn parse_ts(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

/// The first time each task reached L2 or above with a verification that still counts
/// for something (not failed).
fn completed_at(conn: &Connection, repo: &str) -> Result<HashMap<String, Timestamp>> {
    let mut stmt = conn.prepare_cached(
        "SELECT task_id, ts FROM verifications
         WHERE repo_id = ?1 AND orig_level >= 2 AND state != 'failed'",
    )?;
    let mut out: HashMap<String, Timestamp> = HashMap::new();
    for row in stmt.query_map([repo], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (task, ts) = row?;
        if let Some(t) = parse_ts(&ts) {
            out.entry(task)
                .and_modify(|e| *e = (*e).min(t))
                .or_insert(t);
        }
    }
    Ok(out)
}

/// Active seconds per task (spec §5.5: includes its detours' units).
fn active_secs(conn: &Connection, repo: &str) -> Result<HashMap<String, i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT task_id, active_secs FROM task_time WHERE repo_id = ?1")?;
    let rows = stmt.query_map([repo], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The model of a sessions that worked on `task` (the latest one that names a model).
fn session_model(conn: &Connection, repo: &str, task: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached(
            "SELECT s.model FROM units u JOIN sessions s ON s.session_id = u.session_id
             WHERE s.repo_id = ?1 AND u.task_id = ?2 AND s.model IS NOT NULL
             ORDER BY u.started_at DESC LIMIT 1",
        )?
        .query_row(params![repo, task], |r| r.get(0))
        .optional()?)
}

/// The model with the most output tokens in the repo.
fn repo_model(conn: &Connection, repo: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached(
            "SELECT model FROM usage_replies WHERE repo_id = ?1
             GROUP BY model ORDER BY SUM(output) DESC, model LIMIT 1",
        )?
        .query_row([repo], |r| r.get(0))
        .optional()?)
}

/// A leaf's model: most output tokens on it, else its session's, else `unknown`.
fn leaf_model(conn: &Connection, repo: &str, task: &str) -> Result<Option<String>> {
    if let Some(m) = crate::telemetry::main_model(conn, repo, task)? {
        return Ok(Some(m));
    }
    session_model(conn, repo, task)
}

fn is_done(status: NodeStatus) -> bool {
    matches!(status, NodeStatus::L2 | NodeStatus::L3 | NodeStatus::L4)
}

/// The calibration points of `repo` as of `now`.
pub fn calibration(conn: &Connection, repo: &str, now: Timestamp) -> Result<Calibration> {
    let tree = load_tree(conn, repo, false)?;
    let done_at = completed_at(conn, repo)?;
    let active = active_secs(conn, repo)?;
    let since = now - SignedDuration::from_hours(WINDOW_DAYS * 24);
    let mut out = Calibration::default();
    for leaf in leaves(&tree) {
        let Some(estimate) = leaf.estimate_min.filter(|e| *e > 0) else {
            continue;
        };
        if !is_done(leaf.status) {
            continue;
        }
        let Some(at) = done_at.get(&leaf.id).copied() else {
            continue;
        };
        if at < since || at > now {
            continue;
        }
        let secs = active.get(&leaf.id).copied().unwrap_or(0);
        if secs <= 0 {
            continue;
        }
        #[allow(clippy::cast_precision_loss)] // minutes of work, far below 2^52
        let actual_min = secs as f64 / 60.0;
        let model = leaf_model(conn, repo, &leaf.id)?.unwrap_or_else(|| "unknown".into());
        out.pooled.push(Point {
            task_id: leaf.id.clone(),
            title: leaf.title.clone(),
            estimate_min: estimate,
            actual_min,
            ratio: actual_min / f64::from(estimate),
            model,
            completed_at: at.to_string(),
        });
    }
    out.pooled.sort_by(|a, b| {
        (parse_ts(&a.completed_at), &a.task_id).cmp(&(parse_ts(&b.completed_at), &b.task_id))
    });
    for p in &out.pooled {
        out.by_model
            .entry(p.model.clone())
            .or_default()
            .push(p.clone());
    }
    Ok(out)
}

/// The median of `values` (which must not be empty).
pub fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// Nearest-rank percentile of sorted `values` (not empty), `q` in (0, 1].
fn percentile(sorted: &[f64], q: f64) -> f64 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )] // small counts
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

/// Recent agent-active hours per active day: active time of units that stopped in the
/// last [`ACTIVITY_DAYS`] calendar days (in `tz`), over the days that had any. `None`
/// without activity.
pub fn hours_per_day(
    conn: &Connection,
    repo: &str,
    now: Timestamp,
    tz: &TimeZone,
) -> Result<Option<f64>> {
    let today = now.to_zoned(tz.clone()).date();
    let first = today - jiff::Span::new().days(ACTIVITY_DAYS - 1);
    let mut stmt = conn.prepare_cached(
        "SELECT u.stopped_at, u.active_secs FROM units u
         JOIN sessions s ON s.session_id = u.session_id
         WHERE s.repo_id = ?1 AND u.stopped_at IS NOT NULL AND u.active_secs > 0",
    )?;
    let mut per_day: BTreeMap<Date, i64> = BTreeMap::new();
    for row in stmt.query_map([repo], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (stopped, secs) = row?;
        let Some(t) = parse_ts(&stopped) else {
            continue;
        };
        if t > now {
            continue;
        }
        let day = t.to_zoned(tz.clone()).date();
        if day >= first && day <= today {
            *per_day.entry(day).or_default() += secs;
        }
    }
    if per_day.is_empty() {
        return Ok(None);
    }
    let total: i64 = per_day.values().sum();
    #[allow(clippy::cast_precision_loss)] // seconds and day counts, small
    Ok(Some(total as f64 / per_day.len() as f64 / 3600.0))
}

/// `today + ceil(hours / per_day)` calendar days, as `YYYY-MM-DD`.
pub fn date_after(now: Timestamp, tz: &TimeZone, hours: f64, per_day: f64) -> Option<String> {
    if per_day.is_nan() || per_day <= 0.0 || !hours.is_finite() {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)] // a forecast of days, far below i64::MAX
    let days = (hours / per_day).ceil().max(0.0) as i64;
    let today = now.to_zoned(tz.clone()).date();
    let date = today
        .checked_add(jiff::Span::new().days(days.min(36_500)))
        .ok()?;
    Some(date.to_string())
}

/// The newest event position the forecast depends on (forecast history excluded, so
/// recording one doesn't change the next).
fn last_pos(conn: &Connection, repo: &str) -> Result<i64> {
    Ok(conn
        .prepare_cached(
            "SELECT COALESCE(MAX(pos), 0) FROM events WHERE repo_id = ?1 AND kind != 'forecast_ran'",
        )?
        .query_row([repo], |r| r.get(0))?)
}

/// `not enough finished tasks to forecast yet — need 2 more`.
fn thin_reason(points: usize) -> String {
    let more = MIN_POINTS - points;
    format!(
        "Not enough finished tasks to forecast yet — {more} more {} needed",
        if more == 1 { "is" } else { "are" }
    )
}

/// The forecast for `repo` at `now` (dates in `tz`), from [`RUNS`] runs.
pub fn run(conn: &Connection, repo: &str, now: Timestamp, tz: &TimeZone) -> Result<Forecast> {
    run_with(conn, repo, now, tz, RUNS)
}

/// [`run`] with a given number of runs.
pub fn run_with(
    conn: &Connection,
    repo: &str,
    now: Timestamp,
    tz: &TimeZone,
    runs: usize,
) -> Result<Forecast> {
    let cal = calibration(conn, repo, now)?;
    let tree = load_tree(conn, repo, false)?;
    let active = active_secs(conn, repo)?;
    let open: Vec<&PlanNode> = leaves(&tree)
        .into_iter()
        .filter(|n| !is_done(n.status))
        .collect();
    let remaining: Vec<(&PlanNode, u32)> = open
        .iter()
        .filter_map(|n| n.estimate_min.filter(|e| *e > 0).map(|e| (*n, e)))
        .collect();
    let mut f = Forecast {
        p50_hours: 0.0,
        p90_hours: 0.0,
        p50_date: None,
        p90_date: None,
        remaining_min: remaining.iter().map(|(_, e)| u64::from(*e)).sum(),
        unestimated: open.len() - remaining.len(),
        points: cal.pooled.len(),
        hours_per_day: hours_per_day(conn, repo, now, tz)?,
        reason: None,
        mean_ratio: None,
    };
    if cal.pooled.len() < MIN_POINTS {
        f.reason = Some(thin_reason(cal.pooled.len()));
        return Ok(f);
    }
    let pooled: Vec<f64> = cal.pooled.iter().map(|p| p.ratio).collect();
    #[allow(clippy::cast_precision_loss)] // a handful of points
    let mean = pooled.iter().sum::<f64>() / pooled.len() as f64;
    f.mean_ratio = Some(mean);
    if remaining.is_empty() {
        f.reason = Some("Nothing with an estimate is left to forecast".into());
        return Ok(f);
    }
    // Each remaining leaf: its estimate, the time already spent, and the ratios to draw.
    let fallback = repo_model(conn, repo)?;
    let by_model: HashMap<&str, Vec<f64>> = cal
        .by_model
        .iter()
        .filter(|(_, pts)| pts.len() >= MODEL_POINTS)
        .map(|(m, pts)| (m.as_str(), pts.iter().map(|p| p.ratio).collect()))
        .collect();
    let mut legs: Vec<(f64, f64, &[f64])> = Vec::new();
    for (leaf, estimate) in &remaining {
        let model = leaf_model(conn, repo, &leaf.id)?.or_else(|| fallback.clone());
        let ratios = model
            .as_deref()
            .and_then(|m| by_model.get(m))
            .map_or(pooled.as_slice(), Vec::as_slice);
        #[allow(clippy::cast_precision_loss)] // minutes of work, small
        let spent = active.get(&leaf.id).copied().unwrap_or(0).max(0) as f64 / 60.0;
        legs.push((f64::from(*estimate), spent, ratios));
    }
    let pos = last_pos(conn, repo)?.to_string();
    let mut rng = rng::Rng::new(rng::seed(&[repo.as_bytes(), pos.as_bytes()]));
    let mut totals: Vec<f64> = (0..runs.max(1))
        .map(|_| {
            legs.iter()
                .map(|(estimate, spent, ratios)| {
                    (estimate * ratios[rng.below(ratios.len())] - spent).max(0.0)
                })
                .sum()
        })
        .collect();
    totals.sort_by(f64::total_cmp);
    f.p50_hours = percentile(&totals, 0.5) / 60.0;
    f.p90_hours = percentile(&totals, 0.9).max(percentile(&totals, 0.5)) / 60.0;
    match f.hours_per_day {
        None => f.reason = Some("No recent activity to turn hours into dates".into()),
        Some(per_day) => {
            f.p50_date = date_after(now, tz, f.p50_hours, per_day);
            f.p90_date = date_after(now, tz, f.p90_hours, per_day);
        }
    }
    Ok(f)
}

/// The repo's latest recorded forecast.
pub fn latest(conn: &Connection, repo: &str) -> Result<Option<Forecast>> {
    Ok(conn
        .prepare_cached(
            "SELECT p50_hours, p90_hours, p50_date, p90_date, remaining_min, unestimated, points,
                    hours_per_day, reason
             FROM forecast_history WHERE repo_id = ?1 ORDER BY rowid DESC LIMIT 1",
        )?
        .query_row([repo], read_row)
        .optional()?
        .map(|(_, f)| f))
}

/// One history row: (ts, forecast).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryPoint {
    pub ts: String,
    #[serde(flatten)]
    pub forecast: Forecast,
}

fn read_row(r: &rusqlite::Row) -> rusqlite::Result<(String, Forecast)> {
    let n =
        |i: usize| -> rusqlite::Result<u64> { Ok(u64::try_from(r.get::<_, i64>(i)?).unwrap_or(0)) };
    Ok((
        String::new(),
        Forecast {
            p50_hours: r.get(0)?,
            p90_hours: r.get(1)?,
            p50_date: r.get(2)?,
            p90_date: r.get(3)?,
            remaining_min: n(4)?,
            unestimated: usize::try_from(n(5)?).unwrap_or(0),
            points: usize::try_from(n(6)?).unwrap_or(0),
            hours_per_day: r.get(7)?,
            reason: r.get(8)?,
            mean_ratio: None,
        },
    ))
}

/// The repo's forecast history, oldest first.
pub fn history(conn: &Connection, repo: &str) -> Result<Vec<HistoryPoint>> {
    let mut stmt = conn.prepare(
        "SELECT p50_hours, p90_hours, p50_date, p90_date, remaining_min, unestimated, points,
                hours_per_day, reason, ts
         FROM forecast_history WHERE repo_id = ?1 ORDER BY rowid",
    )?;
    let rows = stmt.query_map([repo], |r| {
        let (_, forecast) = read_row(r)?;
        Ok(HistoryPoint {
            ts: r.get(9)?,
            forecast,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `new` differs enough from `old` to be kept in the history.
pub fn moved(old: &Forecast, new: &Forecast) -> bool {
    let rel = |a: f64, b: f64| {
        if a == 0.0 {
            b != 0.0
        } else {
            ((b - a) / a).abs() >= CHANGE_THRESHOLD
        }
    };
    old.reason != new.reason
        || rel(old.p50_hours, new.p50_hours)
        || rel(old.p90_hours, new.p90_hours)
}

/// Run the forecast and append a `forecast_ran` (as `session`) when it moved since the
/// last one. Called after a verification at L2 or above, by the process that recorded it
/// (MCP `verify`, the owner's verdict) — never from a hook. Returns whether it appended.
pub fn record(
    store: &mut Store,
    repo: &str,
    session: &str,
    now: Timestamp,
    tz: &TimeZone,
) -> Result<bool> {
    let new = run(store.conn(), repo, now, tz)?;
    if latest(store.conn(), repo)?.is_some_and(|old| !moved(&old, &new)) {
        return Ok(false);
    }
    let mut data = new;
    data.mean_ratio = None;
    store.append(EventDraft::new(
        repo,
        session,
        "forecast_ran",
        serde_json::to_value(&data)?,
    ))?;
    Ok(true)
}

/// [`record`] at the current time in the owner's time zone. A forecast that can't be
/// made never undoes the verification that prompted it, so errors are dropped here.
pub fn record_now(store: &mut Store, repo: &str, session: &str) {
    let _ = record(store, repo, session, Timestamp::now(), &TimeZone::system());
}

/// The help text every screen gives the range.
pub const RANGE_HELP: &str = "50% / 90% chance, based on how long verified tasks actually took";

/// `Oct 30` (with the year when it isn't `this_year`): a forecast date for people.
pub fn short_date(date: &str, this_year: i16) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Ok(d) = date.parse::<Date>() else {
        return date.to_string();
    };
    let month = MONTHS[usize::try_from(d.month() - 1).unwrap_or(0).min(11)];
    if d.year() == this_year {
        format!("{month} {}", d.day())
    } else {
        format!("{month} {}, {}", d.day(), d.year())
    }
}

/// `likely by Oct 30 · could take until Nov 8`, or the reason there are no dates.
pub fn range_text(f: &Forecast, now: Timestamp, tz: &TimeZone) -> String {
    let year = now.to_zoned(tz.clone()).year();
    match f.dates() {
        Some((p50, p90)) => format!(
            "likely by {} · could take until {}",
            short_date(p50, year),
            short_date(p90, year)
        ),
        None => f
            .reason
            .clone()
            .unwrap_or_else(|| "No forecast yet".to_string()),
    }
}

/// `Oct 30–Nov 8` for one-line screens, or `None` without dates.
pub fn compact_range(f: &Forecast, now: Timestamp, tz: &TimeZone) -> Option<String> {
    let year = now.to_zoned(tz.clone()).year();
    let (p50, p90) = f.dates()?;
    if p50 == p90 {
        return Some(short_date(p50, year));
    }
    Some(format!(
        "{}–{}",
        short_date(p50, year),
        short_date(p90, year)
    ))
}

/// A few words for one-line screens: `likely Oct 30–Nov 8`, or why there are no dates.
pub fn short_text(f: &Forecast, now: Timestamp, tz: &TimeZone) -> String {
    if let Some(range) = compact_range(f, now, tz) {
        return format!("likely {range}");
    }
    if f.points < MIN_POINTS {
        let more = MIN_POINTS - f.points;
        format!(
            "forecast needs {more} more finished {}",
            if more == 1 { "task" } else { "tasks" }
        )
    } else if f.remaining_min == 0 {
        "nothing estimated left".to_string()
    } else {
        "no recent activity to date a forecast".to_string()
    }
}

/// `14h`, `2.5h`, `40m`: agent time for people (never rounded up).
pub fn hours_text(hours: f64) -> String {
    if hours < 1.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // display only
        let m = (hours * 60.0).floor().max(0.0) as u64;
        return format!("{m}m");
    }
    let t = format!("{:.1}", (hours * 10.0).floor() / 10.0);
    format!("{}h", t.strip_suffix(".0").unwrap_or(&t))
}

/// `based on 12 finished tasks; this project averages 1.8× its estimates`.
pub fn basis_text(f: &Forecast) -> String {
    let tasks = if f.points == 1 { "task" } else { "tasks" };
    match f.mean_ratio {
        Some(r) => format!(
            "based on {} finished {tasks}; this project averages {}× its estimates",
            f.points,
            trim2(r)
        ),
        None => format!("based on {} finished {tasks}", f.points),
    }
}

/// One decimal place, trailing `.0` dropped.
fn trim2(v: f64) -> String {
    let t = format!("{v:.1}");
    t.strip_suffix(".0").map_or(t.clone(), str::to_string)
}

#[cfg(test)]
mod tests;
