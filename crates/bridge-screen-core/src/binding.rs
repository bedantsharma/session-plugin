//! Which session is calling the MCP server.
//!
//! Claude Code doesn't tell an MCP server which session a tool call comes from. But the
//! `PreToolUse` hook fires for MCP tools moments before the call, with the `session_id`,
//! tool name, arguments and `tool_use_id`. The hook records a binding; the server then
//! matches the call it receives (same tool, same canonical arguments) to the newest unused
//! binding from the last 30 seconds.

use std::fmt::Write as _;

use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::Result;

/// A call must arrive within this many seconds of its hook to match it.
pub const MATCH_WINDOW_SECS: i64 = 30;

/// Bindings older than this are deleted whenever a new one is recorded.
pub const KEEP_SECS: i64 = 3600;

/// `value` as JSON with object keys sorted and no whitespace, so the hook and the server
/// produce the same text for the same arguments.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// First 16 hex chars of SHA-256 over the canonical JSON of a tool's arguments.
pub fn args_hash(args: &Value) -> String {
    let digest = Sha256::digest(canonical_json(args).as_bytes());
    digest
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// Remember that `session_id` is about to call `tool_name` with arguments hashing to
/// `args_hash`. `tool_name` is the server's own tool name (no `mcp__…__` prefix).
pub fn record(
    conn: &Connection,
    tool_use_id: &str,
    session_id: &str,
    tool_name: &str,
    args_hash: &str,
    now: Timestamp,
) -> Result<()> {
    let now_ms = now.as_millisecond();
    conn.execute(
        "DELETE FROM mcp_bindings WHERE created_at < ?1",
        [now_ms - KEEP_SECS * 1000],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO mcp_bindings
             (tool_use_id, session_id, tool_name, args_hash, created_at, consumed)
         VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        params![tool_use_id, session_id, tool_name, args_hash, now_ms],
    )?;
    Ok(())
}

/// The session of the newest unused matching binding from the last 30 seconds, marking it
/// used. One statement, so two servers can't take the same binding.
pub fn take(
    conn: &Connection,
    tool_name: &str,
    args_hash: &str,
    now: Timestamp,
) -> Result<Option<String>> {
    let now_ms = now.as_millisecond();
    Ok(conn
        .query_row(
            "UPDATE mcp_bindings SET consumed = 1
             WHERE tool_use_id = (
                 SELECT tool_use_id FROM mcp_bindings
                 WHERE tool_name = ?1 AND args_hash = ?2 AND consumed = 0
                   AND created_at >= ?3 AND created_at <= ?4
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT 1)
             RETURNING session_id",
            params![
                tool_name,
                args_hash,
                now_ms - MATCH_WINDOW_SECS * 1000,
                now_ms
            ],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use jiff::{SignedDuration, Timestamp};
    use serde_json::json;

    fn at(secs: i64) -> Timestamp {
        "2026-10-13T09:00:00Z".parse::<Timestamp>().unwrap() + SignedDuration::from_secs(secs)
    }

    fn rows(s: &Store) -> i64 {
        s.conn()
            .query_row("SELECT count(*) FROM mcp_bindings", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn canonical_json_sorts_keys_and_drops_whitespace() {
        let a = json!({"b": 1, "a": {"y": [1, {"d": 2, "c": 3}], "x": "s"}});
        assert_eq!(
            canonical_json(&a),
            r#"{"a":{"x":"s","y":[1,{"c":3,"d":2}]},"b":1}"#
        );
        let b: serde_json::Value =
            serde_json::from_str(r#"{ "a" : { "y":[1,{"c":3,"d":2}], "x":"s" }, "b":1 }"#).unwrap();
        assert_eq!(args_hash(&a), args_hash(&b));
        assert_ne!(args_hash(&a), args_hash(&json!({"b": 2})));
        assert_eq!(args_hash(&a).len(), 16);
    }

    #[test]
    fn record_then_take() {
        let s = Store::open_in_memory().unwrap();
        let h = args_hash(&json!({"id": "1.2"}));
        record(s.conn(), "toolu_1", "s1", "claim_task", &h, at(0)).unwrap();
        assert_eq!(take(s.conn(), "status", &h, at(1)).unwrap(), None);
        assert_eq!(take(s.conn(), "claim_task", "other", at(1)).unwrap(), None);
        assert_eq!(
            take(s.conn(), "claim_task", &h, at(1)).unwrap().as_deref(),
            Some("s1")
        );
        // Each binding is used once.
        assert_eq!(take(s.conn(), "claim_task", &h, at(2)).unwrap(), None);
    }

    #[test]
    fn identical_calls_bind_to_their_own_sessions() {
        let s = Store::open_in_memory().unwrap();
        let h = args_hash(&json!({"id": "1.2", "mode": "auto", "confidence": 0.9}));
        record(s.conn(), "toolu_a", "A", "claim_task", &h, at(0)).unwrap();
        record(s.conn(), "toolu_b", "B", "claim_task", &h, at(0)).unwrap();
        let first = take(s.conn(), "claim_task", &h, at(0)).unwrap().unwrap();
        let second = take(s.conn(), "claim_task", &h, at(0)).unwrap().unwrap();
        let mut got = vec![first, second];
        got.sort();
        assert_eq!(got, vec!["A", "B"]);
        assert_eq!(take(s.conn(), "claim_task", &h, at(0)).unwrap(), None);

        // Newest first when they differ in time.
        record(s.conn(), "toolu_c", "C", "claim_task", &h, at(10)).unwrap();
        record(s.conn(), "toolu_d", "D", "claim_task", &h, at(12)).unwrap();
        assert_eq!(
            take(s.conn(), "claim_task", &h, at(13)).unwrap().as_deref(),
            Some("D")
        );
        assert_eq!(
            take(s.conn(), "claim_task", &h, at(13)).unwrap().as_deref(),
            Some("C")
        );
    }

    #[test]
    fn bindings_expire_after_30_seconds() {
        let s = Store::open_in_memory().unwrap();
        record(s.conn(), "toolu_1", "s1", "status", "h", at(0)).unwrap();
        assert_eq!(take(s.conn(), "status", "h", at(31)).unwrap(), None);
        record(s.conn(), "toolu_2", "s1", "status", "h", at(100)).unwrap();
        assert_eq!(
            take(s.conn(), "status", "h", at(130)).unwrap().as_deref(),
            Some("s1")
        );
    }

    #[test]
    fn old_bindings_are_cleaned_up_after_an_hour() {
        let s = Store::open_in_memory().unwrap();
        record(s.conn(), "toolu_1", "s1", "status", "h", at(0)).unwrap();
        record(s.conn(), "toolu_2", "s1", "status", "h", at(3000)).unwrap();
        assert_eq!(rows(&s), 2);
        record(s.conn(), "toolu_3", "s1", "status", "h", at(3601)).unwrap();
        assert_eq!(rows(&s), 2, "the binding from t=0 is gone");
    }

    #[test]
    fn recording_the_same_tool_use_twice_keeps_one() {
        let s = Store::open_in_memory().unwrap();
        record(s.conn(), "toolu_1", "s1", "status", "h", at(0)).unwrap();
        record(s.conn(), "toolu_1", "s1", "status", "h", at(1)).unwrap();
        assert_eq!(rows(&s), 1);
    }
}
