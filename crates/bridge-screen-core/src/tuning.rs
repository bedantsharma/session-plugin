//! Auto-tuning of the drift thresholds per repo (spec §7.6).
//!
//! Every owner label, and every agent label the heuristic didn't already give, is a
//! `drift_samples` row. After 20 new samples, the next SessionStart moves the lower and
//! upper cut-offs toward the values that best separate on-task from detour/drift in the
//! repo's last 200 samples, and nudges the tolerance percentage from the owner's
//! Continue/Stop answers. Each change is a `config_changed` event; guard rails keep every
//! move small and the cut-offs ordered.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::config::Config;
use crate::error::Result;
use crate::store::{EventDraft, OWNER_SESSION, append_in};
use crate::tangent::{ConfigChanged, Label, Thresholds, thresholds};

/// A run happens once this many samples have arrived since the last change.
pub const RUN_EVERY: i64 = 20;
/// How many of the most recent samples a run looks at.
pub const WINDOW: i64 = 200;
/// The most a cut-off moves in one run.
pub const MAX_STEP: f64 = 0.05;
pub const LOWER_BOUNDS: (f64, f64) = (0.15, 0.5);
pub const UPPER_BOUNDS: (f64, f64) = (0.5, 0.9);
/// The lower cut-off always stays below `upper - GAP`.
pub const GAP: f64 = 0.05;
/// Tolerance percentage: bounds and the step per run.
pub const PCT_BOUNDS: (f64, f64) = (10.0, 60.0);
pub const PCT_STEP: f64 = 5.0;

/// One key a run (or a reset) changed.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyChange {
    pub key: &'static str,
    pub old: f64,
    pub new: f64,
}

/// What a run changed, and how many samples it saw.
#[derive(Debug, Clone, PartialEq)]
pub struct TuningChange {
    pub changes: Vec<KeyChange>,
    pub samples: i64,
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// How many drift samples the repo has, and how many it had at the last change.
fn counts(conn: &Connection, repo_id: &str) -> Result<(i64, i64)> {
    let total: i64 = conn
        .prepare_cached("SELECT count(*) FROM drift_samples WHERE repo_id = ?1")?
        .query_row([repo_id], |r| r.get(0))?;
    let last: Option<i64> = conn
        .prepare_cached("SELECT samples_at_last_run FROM repo_tuning WHERE repo_id = ?1")?
        .query_row([repo_id], |r| r.get(0))
        .optional()?;
    Ok((total, last.unwrap_or(0)))
}

/// Is a run due (at least [`RUN_EVERY`] new samples)?
pub fn due(conn: &Connection, repo_id: &str) -> Result<bool> {
    let (total, last) = counts(conn, repo_id)?;
    Ok(total - last >= RUN_EVERY)
}

/// The value in `[lo, hi]` (steps of 0.01) with the lowest cost; ties go to the value
/// closest to `current`.
fn best(lo: f64, hi: f64, current: f64, cost: impl Fn(f64) -> usize) -> f64 {
    #[allow(clippy::cast_possible_truncation)] // small, positive hundredths
    let (from, to) = ((lo * 100.0).round() as i64, (hi * 100.0).round() as i64);
    let mut pick = (usize::MAX, f64::MAX, current);
    for i in from..=to {
        #[allow(clippy::cast_precision_loss)] // i is at most 100
        let v = i as f64 / 100.0;
        let key = (cost(v), (v - current).abs());
        if key.0 < pick.0 || (key.0 == pick.0 && key.1 < pick.1 - 1e-9) {
            pick = (key.0, key.1, v);
        }
    }
    pick.2
}

/// Move `current` toward `target` by at most [`MAX_STEP`], within `bounds`.
fn step_toward(current: f64, target: f64, bounds: (f64, f64)) -> f64 {
    round2((current + (target - current).clamp(-MAX_STEP, MAX_STEP)).clamp(bounds.0, bounds.1))
}

/// The repo's last [`WINDOW`] samples as (score, label).
fn samples(conn: &Connection, repo_id: &str) -> Result<Vec<(f64, Label)>> {
    let mut stmt = conn.prepare(
        "SELECT score, label FROM drift_samples WHERE repo_id = ?1 ORDER BY rowid DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![repo_id, WINDOW], |r| {
        Ok((r.get::<_, f64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (score, label) = row?;
        if let Some(label) = Label::parse(&label) {
            out.push((score, label));
        }
    }
    Ok(out)
}

/// The owner's Continue and Stop answers since the repo's last `config_changed`.
fn owner_answers(conn: &Connection, repo_id: &str) -> Result<(usize, usize)> {
    let since: i64 = conn.query_row(
        "SELECT COALESCE(MAX(pos), 0) FROM events WHERE repo_id = ?1 AND kind = 'config_changed'",
        [repo_id],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT data FROM events WHERE repo_id = ?1 AND kind = 'owner_action' AND pos > ?2",
    )?;
    let mut rows = stmt.query(params![repo_id, since])?;
    let (mut go, mut stop) = (0, 0);
    while let Some(row) = rows.next()? {
        let data: serde_json::Value = serde_json::from_str(&row.get::<_, String>(0)?)?;
        if data["action"] != "drift" {
            continue;
        }
        match data["answer"].as_str() {
            Some("continue") => go += 1,
            Some("stop") => stop += 1,
            _ => {}
        }
    }
    Ok((go, stop))
}

/// The thresholds a run would move to from `th`, given samples and owner answers.
pub fn propose(th: &Thresholds, samples: &[(f64, Label)], answers: (usize, usize)) -> Thresholds {
    let lower_target = best(LOWER_BOUNDS.0, LOWER_BOUNDS.1, th.lower, |v| {
        samples
            .iter()
            .filter(|(s, l)| (*l == Label::OnTask) == (*s >= v))
            .count()
    });
    let upper_target = best(UPPER_BOUNDS.0, UPPER_BOUNDS.1, th.upper, |v| {
        samples
            .iter()
            .filter(|(s, l)| (*l == Label::Drift) == (*s <= v))
            .count()
    });
    let upper = step_toward(th.upper, upper_target, UPPER_BOUNDS);
    let mut lower = step_toward(th.lower, lower_target, LOWER_BOUNDS);
    if lower >= upper - GAP - 1e-9 {
        lower = round2(upper - GAP - 0.01).clamp(LOWER_BOUNDS.0, LOWER_BOUNDS.1);
    }
    let (go, stop) = answers;
    let nudge = match go.cmp(&stop) {
        std::cmp::Ordering::Greater => PCT_STEP,
        std::cmp::Ordering::Less => -PCT_STEP,
        std::cmp::Ordering::Equal => 0.0,
    };
    let tolerance_pct = (th.tolerance_pct + nudge).clamp(PCT_BOUNDS.0, PCT_BOUNDS.1);
    Thresholds {
        lower,
        upper,
        tolerance_min_minutes: th.tolerance_min_minutes,
        tolerance_pct,
    }
}

/// Record a `config_changed` for each key that differs between `from` and `to`.
fn record(
    tx: &Transaction,
    repo_id: &str,
    from: &Thresholds,
    to: &Thresholds,
    samples: i64,
) -> Result<Vec<KeyChange>> {
    let mut changes = Vec::new();
    for (key, old, new) in [
        ("drift_lower", from.lower, to.lower),
        ("drift_upper", from.upper, to.upper),
        ("tolerance_pct", from.tolerance_pct, to.tolerance_pct),
    ] {
        if (old - new).abs() < 1e-9 {
            continue;
        }
        let data = ConfigChanged {
            scope: "repo".into(),
            key: key.into(),
            old: Some(old),
            new,
            samples,
        };
        append_in(
            tx,
            EventDraft::new(
                repo_id,
                OWNER_SESSION,
                "config_changed",
                serde_json::to_value(data)?,
            ),
        )?;
        changes.push(KeyChange { key, old, new });
    }
    Ok(changes)
}

/// Tune the repo's thresholds if a run is due. `None` when it isn't; a run that moves
/// nothing records nothing.
pub fn run(tx: &Transaction, repo_id: &str, cfg: &Config) -> Result<Option<TuningChange>> {
    let (total, last) = counts(tx, repo_id)?;
    if total - last < RUN_EVERY {
        return Ok(None);
    }
    let th = thresholds(tx, repo_id, cfg)?;
    let next = propose(&th, &samples(tx, repo_id)?, owner_answers(tx, repo_id)?);
    let changes = record(tx, repo_id, &th, &next, total)?;
    Ok(Some(TuningChange {
        changes,
        samples: total,
    }))
}

/// Put the repo's thresholds back to the config defaults (`config reset-drift`).
pub fn reset(tx: &Transaction, repo_id: &str, cfg: &Config) -> Result<Vec<KeyChange>> {
    let (total, _) = counts(tx, repo_id)?;
    let th = thresholds(tx, repo_id, cfg)?;
    record(tx, repo_id, &th, &Thresholds::from_config(cfg), total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use serde_json::json;

    fn add_samples(s: &Store, items: &[(f64, &str)]) {
        for (score, label) in items {
            s.conn()
                .execute(
                    "INSERT INTO drift_samples (repo_id, unit_ref, score, label, source, ts)
                     VALUES ('r', 'x:1', ?1, ?2, 'agent', 't')",
                    params![score, label],
                )
                .unwrap();
        }
    }

    fn tuned(s: &mut Store) -> Option<TuningChange> {
        s.write(|tx| run(tx, "r", &Config::default())).unwrap()
    }

    fn th(s: &Store) -> Thresholds {
        thresholds(s.conn(), "r", &Config::default()).unwrap()
    }

    #[test]
    fn tuning_respects_guard_rails() {
        let mut s = Store::open_in_memory().unwrap();
        let mut before = th(&s);
        for _ in 0..12 {
            // Extreme skew: on-task right under the middle, drift right above it.
            add_samples(&s, &[(0.49, "on_task"); 10]);
            add_samples(&s, &[(0.51, "drift"); 10]);
            tuned(&mut s).unwrap();
            let after = th(&s);
            assert!(
                (after.lower - before.lower).abs() <= MAX_STEP + 1e-9,
                "{after:?}"
            );
            assert!(
                (after.upper - before.upper).abs() <= MAX_STEP + 1e-9,
                "{after:?}"
            );
            assert!((LOWER_BOUNDS.0..=LOWER_BOUNDS.1).contains(&after.lower));
            assert!((UPPER_BOUNDS.0..=UPPER_BOUNDS.1).contains(&after.upper));
            assert!(after.lower < after.upper - GAP, "{after:?}");
            before = after;
        }
        // The very first run moved each cut-off by exactly 0.05.
        let first: Vec<(String, f64, f64)> = crate::store::read_all(s.conn(), Some("r"))
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "config_changed")
            .take(2)
            .map(|e| {
                (
                    e.data["key"].as_str().unwrap().to_string(),
                    e.data["old"].as_f64().unwrap(),
                    e.data["new"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            first,
            vec![
                ("drift_lower".to_string(), 0.3, 0.35),
                ("drift_upper".to_string(), 0.7, 0.65)
            ]
        );
    }

    #[test]
    fn separable_samples_move_cut_offs_toward_the_gap() {
        let mut s = Store::open_in_memory().unwrap();
        let mut seen = Vec::new();
        for _ in 0..5 {
            add_samples(&s, &[(0.40, "on_task"), (0.45, "on_task")].repeat(4));
            add_samples(
                &s,
                &[(0.78, "detour"), (0.85, "drift"), (0.9, "drift")].repeat(4),
            );
            tuned(&mut s);
            let t = th(&s);
            seen.push((t.lower, t.upper));
        }
        assert_eq!(
            seen,
            vec![
                (0.35, 0.75),
                (0.4, 0.78),
                (0.45, 0.78),
                (0.46, 0.78),
                (0.46, 0.78)
            ]
        );
        // Rebuild reproduces the tuned values from the events.
        let before = th(&s);
        s.conn().execute("DELETE FROM drift_samples", []).unwrap();
        s.rebuild().unwrap();
        assert_eq!(th(&s), before);
    }

    #[test]
    fn fewer_than_twenty_new_samples_is_no_run() {
        let mut s = Store::open_in_memory().unwrap();
        add_samples(&s, &[(0.49, "on_task"); 19]);
        assert!(!due(s.conn(), "r").unwrap());
        assert_eq!(tuned(&mut s), None);
        add_samples(&s, &[(0.49, "on_task")]);
        assert!(due(s.conn(), "r").unwrap());
        assert!(tuned(&mut s).is_some());
        // Counted from the last change: 19 more is still not enough.
        add_samples(&s, &[(0.49, "on_task"); 19]);
        assert_eq!(tuned(&mut s), None);
    }

    #[test]
    fn owner_answers_move_the_tolerance_percentage() {
        let mut s = Store::open_in_memory().unwrap();
        for answer in ["continue", "continue", "stop"] {
            s.append(EventDraft::new(
                "r",
                OWNER_SESSION,
                "owner_action",
                json!({"action": "drift", "answer": answer, "target": "x"}),
            ))
            .unwrap();
        }
        add_samples(&s, &[(0.1, "on_task"); 20]);
        tuned(&mut s);
        assert!((th(&s).tolerance_pct - 25.0).abs() < 1e-9);
        // The same answers don't count twice.
        add_samples(&s, &[(0.1, "on_task"); 20]);
        tuned(&mut s);
        assert!((th(&s).tolerance_pct - 25.0).abs() < 1e-9);
    }

    #[test]
    fn reset_restores_defaults() {
        let mut s = Store::open_in_memory().unwrap();
        add_samples(&s, &[(0.49, "on_task"); 20]);
        tuned(&mut s);
        assert!((th(&s).lower - 0.35).abs() < 1e-9);
        let changes = s.write(|tx| reset(tx, "r", &Config::default())).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(th(&s), Thresholds::from_config(&Config::default()));
        assert_eq!(
            s.write(|tx| reset(tx, "r", &Config::default())).unwrap(),
            vec![]
        );
    }
}
