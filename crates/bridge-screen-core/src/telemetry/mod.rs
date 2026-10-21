//! Telemetry (spec §9.2): tokens per model reply, read from Claude Code's transcripts at
//! Stop and SubagentStop, and what they cost.
//!
//! - [`read`] reads only the new bytes of a transcript (whole lines, a bounded slice per
//!   hook) and keeps each model reply once, by its message id: Claude Code writes one
//!   reply on several lines with the same usage.
//! - [`project`] records each reply in `usage_replies`, attributed by its timestamp to the
//!   session's unit, task, detour and agent, and keeps a byte cursor per transcript.
//! - [`totals`] sums a scope's tokens per model; [`cost`] prices them from the
//!   `config.toml` table **when read**, so a price fix applies to past usage too. Money is
//!   never stored.
//!
//! Privacy: only message ids, model names, token counts and timestamps are kept — never
//! message text, tool inputs or outputs, or prompts.

pub mod project;
pub mod read;

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::config::{Price, price_for};
use crate::error::Result;

pub use project::{USAGE_KINDS, project};
pub use read::{MAX_HOOK_BYTES, ReadOut, read_new, read_to_end};

/// One model reply's tokens, as the `usage` event carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub message_id: String,
    pub model: String,
    /// When Claude Code wrote the reply (from the transcript, RFC 3339).
    pub ts: String,
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write_5m: u64,
    #[serde(default)]
    pub cache_write_1h: u64,
    /// The transcript marked it as a subagent's (sidechain) reply.
    #[serde(default)]
    pub sidechain: bool,
}

/// The `usage` event: the replies new since the transcript's cursor, and where the
/// cursor is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub path: String,
    pub offset_after: u64,
    /// The read started again from 0: the file shrank or was replaced. The cursor is set
    /// back instead of only moving forward; message ids keep replies from counting twice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restarted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub replies: Vec<Reply>,
}

/// Token counts of one kind of spend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
}

impl Counts {
    /// Every token, of every kind.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write_5m + self.cache_write_1h
    }

    fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
    }
}

/// A scope's tokens: the sum and the split by model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Tokens {
    #[serde(flatten)]
    pub sum: Counts,
    /// Distinct model replies counted.
    pub replies: u64,
    pub by_model: BTreeMap<String, Counts>,
}

impl Tokens {
    pub fn is_empty(&self) -> bool {
        self.replies == 0
    }
}

/// What a total is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// A task, including detours opened under it (spec §5.5).
    Task {
        repo: String,
        id: String,
    },
    Detour(String),
    Session(String),
    Repo(String),
    /// One subagent of a session.
    Agent {
        session: String,
        agent_id: String,
    },
}

/// The tokens of `scope`, per model.
pub fn totals(conn: &Connection, scope: &Scope) -> Result<Tokens> {
    const SUMS: &str = "SELECT model, count(*), SUM(input), SUM(output), SUM(cache_read),
                               SUM(cache_write_5m), SUM(cache_write_1h)
                        FROM usage_replies";
    let (filter, args): (&str, Vec<&str>) = match scope {
        Scope::Task { repo, id } => (
            "WHERE repo_id = ?1 AND (task_id = ?2 OR detour_id IN
                 (SELECT detour_id FROM detours WHERE repo_id = ?1 AND blocks_task = ?2))",
            vec![repo, id],
        ),
        Scope::Detour(id) => ("WHERE detour_id = ?1", vec![id]),
        Scope::Session(id) => ("WHERE session_id = ?1", vec![id]),
        Scope::Repo(id) => ("WHERE repo_id = ?1", vec![id]),
        Scope::Agent { session, agent_id } => (
            "WHERE session_id = ?1 AND agent_id = ?2",
            vec![session, agent_id],
        ),
    };
    let mut stmt = conn.prepare_cached(&format!("{SUMS} {filter} GROUP BY model"))?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
        let n = |i: usize| -> rusqlite::Result<u64> {
            Ok(u64::try_from(r.get::<_, Option<i64>>(i)?.unwrap_or(0)).unwrap_or(0))
        };
        Ok((
            r.get::<_, String>(0)?,
            n(1)?,
            Counts {
                input: n(2)?,
                output: n(3)?,
                cache_read: n(4)?,
                cache_write_5m: n(5)?,
                cache_write_1h: n(6)?,
            },
        ))
    })?;
    let mut out = Tokens::default();
    for row in rows {
        let (model, replies, counts) = row?;
        out.replies += replies;
        out.sum.add(&counts);
        out.by_model.insert(model, counts);
    }
    Ok(out)
}

/// The tokens of a task per agent type (`None`: the main agent).
pub fn task_by_agent(
    conn: &Connection,
    repo: &str,
    task: &str,
) -> Result<BTreeMap<Option<String>, Counts>> {
    let mut stmt = conn.prepare_cached(
        "SELECT agent_type, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write_5m),
                SUM(cache_write_1h)
         FROM usage_replies
         WHERE repo_id = ?1 AND (task_id = ?2 OR detour_id IN
               (SELECT detour_id FROM detours WHERE repo_id = ?1 AND blocks_task = ?2))
         GROUP BY agent_type",
    )?;
    let rows = stmt.query_map(params![repo, task], |r| {
        let n = |i: usize| -> rusqlite::Result<u64> {
            Ok(u64::try_from(r.get::<_, Option<i64>>(i)?.unwrap_or(0)).unwrap_or(0))
        };
        Ok((
            r.get::<_, Option<String>>(0)?,
            Counts {
                input: n(1)?,
                output: n(2)?,
                cache_read: n(3)?,
                cache_write_5m: n(4)?,
                cache_write_1h: n(5)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Tokens per unit of a session (units without usage are absent).
pub fn session_by_unit(conn: &Connection, session: &str) -> Result<BTreeMap<i64, u64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT unit_no, SUM(input + output + cache_read + cache_write_5m + cache_write_1h)
         FROM usage_replies WHERE session_id = ?1 AND unit_no IS NOT NULL GROUP BY unit_no",
    )?;
    let rows = stmt.query_map([session], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The model with the most output tokens on a task, if any reply is attributed to it.
pub fn main_model(conn: &Connection, repo: &str, task: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .prepare_cached(
            "SELECT model FROM usage_replies WHERE repo_id = ?1 AND task_id = ?2
             GROUP BY model ORDER BY SUM(output) DESC, model LIMIT 1",
        )?
        .query_row(params![repo, task], |r| r.get(0))
        .optional()?)
}

/// What some tokens cost, at the configured prices.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Cost {
    /// USD for the priced models. `None` when nothing is priced but some tokens were
    /// spent: the cost is unknown, never $0.
    pub usd: Option<f64>,
    /// Models with tokens and no price. When `usd` is set too, it is a lower bound.
    pub unpriced_models: Vec<String>,
    /// USD per priced model.
    pub by_model: BTreeMap<String, f64>,
}

impl Cost {
    /// `usd` covers everything that was spent.
    pub fn complete(&self) -> bool {
        self.unpriced_models.is_empty()
    }
}

/// USD for `counts` at `price` (USD per million tokens).
pub fn priced(counts: &Counts, price: &Price) -> f64 {
    #[allow(clippy::cast_precision_loss)] // token counts stay far below 2^52
    let m = |n: u64| n as f64 / 1_000_000.0;
    m(counts.input) * price.input
        + m(counts.output) * price.output
        + m(counts.cache_read) * price.cache_read
        + m(counts.cache_write_5m) * price.cache_write_5m
        + m(counts.cache_write_1h) * price.cache_write_1h
}

/// Price `tokens` with `prices` (exact model id, else the longest prefix).
pub fn cost(tokens: &Tokens, prices: &BTreeMap<String, Price>) -> Cost {
    let mut out = Cost::default();
    for (model, counts) in &tokens.by_model {
        if counts.total() == 0 {
            continue;
        }
        match price_for(prices, model) {
            Some(price) => {
                out.by_model.insert(model.clone(), priced(counts, price));
            }
            None => out.unpriced_models.push(model.clone()),
        }
    }
    if !out.by_model.is_empty() || out.unpriced_models.is_empty() {
        out.usd = Some(out.by_model.values().sum());
    }
    out
}

/// `1.2M`, `340k`, `950`: a token count for people.
pub fn tokens_text(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // display only
    let f = n as f64;
    if n >= 1_000_000_000 {
        format!("{}B", trim_one(f / 1e9))
    } else if n >= 1_000_000 {
        format!("{}M", trim_one(f / 1e6))
    } else if n >= 10_000 {
        format!("{}k", (f / 1e3).floor())
    } else if n >= 1_000 {
        format!("{}k", trim_one(f / 1e3))
    } else {
        n.to_string()
    }
}

/// One decimal, never rounded up (1.29 → 1.2), without a trailing `.0`.
fn trim_one(v: f64) -> String {
    let t = format!("{:.1}", (v * 10.0).floor() / 10.0);
    t.strip_suffix(".0").map_or(t.clone(), str::to_string)
}

/// `$4.10`, `$1,240`, `<$0.01`: money for people.
pub fn usd_text(usd: f64) -> String {
    if usd <= 0.0 {
        return "$0.00".to_string();
    }
    if usd < 0.01 {
        return "<$0.01".to_string();
    }
    if usd >= 1000.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // display only
        let whole = usd.round() as u64;
        let digits = whole.to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        return format!("${out}");
    }
    format!("${usd:.2}")
}

/// A cost in words: `$4.10`, `≥ $4.10 (some models unpriced)`, or `cost unknown`.
pub fn cost_text(cost: &Cost) -> String {
    match cost.usd {
        Some(usd) if cost.complete() => usd_text(usd),
        Some(usd) => format!("≥ {} (some models unpriced)", usd_text(usd)),
        None => "cost unknown".to_string(),
    }
}

/// `Opus 5.5` for `claude-opus-5-5`: a model id for people (unknown shapes as they are).
pub fn model_text(model: &str) -> String {
    let Some(rest) = model.strip_prefix("claude-") else {
        return model.to_string();
    };
    let mut parts = rest.split('-');
    let Some(family) = parts.next() else {
        return model.to_string();
    };
    let version: Vec<&str> = parts
        .take_while(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()))
        .collect();
    if version.is_empty() {
        return model.to_string();
    }
    let mut name = family.to_string();
    if let Some(first) = name.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    format!("{name} {}", version.join("."))
}

#[cfg(test)]
mod tests;
