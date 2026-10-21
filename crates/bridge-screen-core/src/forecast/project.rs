//! The projector for `forecast_ran`: `forecast_history`, one row per recorded forecast.

use rusqlite::{Transaction, params};

use super::Forecast;
use crate::error::Result;
use crate::store::Event;

/// Event kinds routed to this projector.
pub const FORECAST_KINDS: &[&str] = &["forecast_ran"];

pub fn project(tx: &Transaction, event: &Event) -> Result<()> {
    let f: Forecast = serde_json::from_value(event.data.clone())?;
    let i = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
    tx.execute(
        "INSERT INTO forecast_history (repo_id, ts, p50_hours, p90_hours, p50_date, p90_date,
                                       remaining_min, unestimated, points, hours_per_day, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            event.repo_id,
            event.ts,
            f.p50_hours,
            f.p90_hours,
            f.p50_date,
            f.p90_date,
            i(f.remaining_min),
            i(f.unestimated as u64),
            i(f.points as u64),
            f.hours_per_day,
            f.reason
        ],
    )?;
    Ok(())
}
