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
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::config::{Price, price_for};
use crate::error::Result;

pub use project::{USAGE_KINDS, project};
pub use read::{MAX_HOOK_BYTES, ReadOut, read_new, read_to_end};

/// Appended to the model id of a fast-mode reply (`claude-opus-5-5 (fast)`): fast mode
/// costs more than the standard rate, so it is priced only by an entry of its own.
pub const FAST: &str = " (fast)";

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

/// Where Claude Code writes a subagent's transcript, next to the session's own:
/// `<transcript dir>/<session_id>/subagents/agent-<agent_id>.jsonl`.
pub fn subagent_path(transcript: &Path, session_id: &str, agent_id: &str) -> PathBuf {
    transcript
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(session_id)
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"))
}

/// A transcript's byte cursor (0 when it was never read).
pub fn cursor(conn: &Connection, path: &str) -> Result<u64> {
    let offset: Option<i64> = conn
        .prepare_cached("SELECT offset FROM transcript_cursors WHERE path = ?1")?
        .query_row([path], |r| r.get(0))
        .optional()?;
    Ok(offset.and_then(|o| u64::try_from(o).ok()).unwrap_or(0))
}

/// What a transcript holds that isn't recorded yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The `usage` event to record, or `None` when nothing moved.
    pub usage: Option<Usage>,
    /// A line too long to read was skipped (worth a log line).
    pub skipped_long_line: bool,
}

/// Read `path` from its cursor — at most `max` bytes, or to the end with `None` (owner
/// catch-up only) — and drop replies already recorded.
pub fn pending(conn: &Connection, path: &Path, max: Option<u64>) -> Result<Pending> {
    let key = path.to_string_lossy().into_owned();
    let from = cursor(conn, &key)?;
    let out = match max {
        Some(max) => read::read_new(path, from, max)?,
        None => read::read_to_end(path, from)?,
    };
    let mut known = conn.prepare_cached("SELECT 1 FROM usage_replies WHERE message_id = ?1")?;
    let mut replies = Vec::with_capacity(out.replies.len());
    for r in out.replies {
        if !known.exists([&r.message_id])? {
            replies.push(r);
        }
    }
    let moved = out.offset_after != from || out.restarted;
    Ok(Pending {
        usage: (moved || !replies.is_empty()).then_some(Usage {
            path: key,
            offset_after: out.offset_after,
            restarted: out.restarted,
            agent_id: None,
            agent_type: None,
            replies,
        }),
        skipped_long_line: out.skipped_long_line,
    })
}

/// The subagent transcripts of a session: (agent id, agent type, path), by agent id. The
/// type comes from the `.meta.json` next to each (its `agentType`, nothing else).
pub fn subagent_files(
    transcript: &Path,
    session_id: &str,
) -> Vec<(String, Option<String>, PathBuf)> {
    let dir = subagent_path(transcript, session_id, "x")
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name
            .strip_prefix("agent-")
            .and_then(|n| n.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let meta = dir.join(format!("agent-{id}.meta.json"));
        let agent_type = std::fs::read(&meta)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["agentType"].as_str().map(str::to_string));
        out.push((id.to_string(), agent_type, entry.path()));
    }
    out.sort();
    out
}

/// What a catch-up added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CatchUp {
    /// Transcript files read.
    pub files: usize,
    /// New replies recorded.
    pub replies: usize,
    /// Their tokens.
    pub tokens: u64,
}

/// Read every known transcript of the repo's sessions (and their subagents') to the end
/// and record what is new. Owner-run (`bridge-screen usage --catch-up`), never a hook:
/// it is unbounded.
pub fn catch_up(store: &mut crate::store::Store, repo_id: &str) -> Result<CatchUp> {
    let mut stmt = store.conn().prepare(
        "SELECT session_id, transcript_path FROM sessions
         WHERE repo_id = ?1 AND transcript_path IS NOT NULL ORDER BY started_at, session_id",
    )?;
    let sessions: Vec<(String, String)> = stmt
        .query_map([repo_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    let mut out = CatchUp::default();
    for (session, transcript) in sessions {
        let main = PathBuf::from(&transcript);
        let mut files = vec![(None, None, main.clone())];
        files.extend(
            subagent_files(&main, &session)
                .into_iter()
                .map(|(id, kind, path)| (Some(id), kind, path)),
        );
        for (agent_id, agent_type, path) in files {
            if !path.exists() {
                continue;
            }
            out.files += 1;
            let Some(mut usage) = pending(store.conn(), &path, None)?.usage else {
                continue;
            };
            usage.agent_id.clone_from(&agent_id);
            usage.agent_type.clone_from(&agent_type);
            out.replies += usage.replies.len();
            out.tokens += usage
                .replies
                .iter()
                .map(|r| r.input + r.output + r.cache_read + r.cache_write_5m + r.cache_write_1h)
                .sum::<u64>();
            let actor = crate::store::Actor {
                agent_id,
                agent_type,
                model: None,
            };
            store.append(
                crate::store::EventDraft::new(
                    repo_id,
                    &session,
                    "usage",
                    serde_json::to_value(&usage)?,
                )
                .actor(actor),
            )?;
        }
    }
    Ok(out)
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

    pub fn add(&mut self, other: &Self) {
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

    /// Add another scope's tokens to these.
    pub fn merge(&mut self, other: &Self) {
        self.replies += other.replies;
        self.sum.add(&other.sum);
        for (model, c) in &other.by_model {
            self.by_model.entry(model.clone()).or_default().add(c);
        }
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

/// How [`split`] groups a repo's usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitBy {
    Model,
    Task,
    Session,
    /// By agent type (`None`: the main agent).
    Agent,
}

/// Narrows [`split`] to one session and/or one task (with its detours).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub session: Option<String>,
    pub task: Option<String>,
}

/// A repo's tokens grouped `by` (`None`: no task, the main agent, …), keys in order.
pub fn split(
    conn: &Connection,
    repo: &str,
    filter: &Filter,
    by: SplitBy,
) -> Result<Vec<(Option<String>, Tokens)>> {
    let key = match by {
        SplitBy::Model => "model",
        SplitBy::Task => "task_id",
        SplitBy::Session => "session_id",
        SplitBy::Agent => "agent_type",
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {key}, model, count(*), SUM(input), SUM(output), SUM(cache_read),
                SUM(cache_write_5m), SUM(cache_write_1h)
         FROM usage_replies
         WHERE repo_id = ?1 AND (?2 IS NULL OR session_id = ?2)
           AND (?3 IS NULL OR task_id = ?3 OR detour_id IN
                (SELECT detour_id FROM detours WHERE repo_id = ?1 AND blocks_task = ?3))
         GROUP BY {key}, model"
    ))?;
    let rows = stmt.query_map(params![repo, filter.session, filter.task], |r| {
        let n = |i: usize| -> rusqlite::Result<u64> {
            Ok(u64::try_from(r.get::<_, Option<i64>>(i)?.unwrap_or(0)).unwrap_or(0))
        };
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, String>(1)?,
            n(2)?,
            Counts {
                input: n(3)?,
                output: n(4)?,
                cache_read: n(5)?,
                cache_write_5m: n(6)?,
                cache_write_1h: n(7)?,
            },
        ))
    })?;
    let mut groups: BTreeMap<Option<String>, Tokens> = BTreeMap::new();
    for row in rows {
        let (k, model, replies, counts) = row?;
        let t = groups.entry(k).or_default();
        t.replies += replies;
        t.sum.add(&counts);
        t.by_model.insert(model, counts);
    }
    Ok(groups.into_iter().collect())
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
    if let Some(base) = model.strip_suffix(FAST) {
        return format!("{}{FAST}", model_text(base));
    }
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
