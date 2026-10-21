//! The SQLite store: the append-only event log plus view tables (spec §5–6).

use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};

use crate::error::{Error, Result};
use crate::repo::RepoInfo;

pub use event::{
    Actor, EVENT_VERSION, Event, EventDraft, OWNER_SESSION, append_in, append_in_at, read_all,
};
pub use schema::SCHEMA_VERSION;

/// How long a connection waits for another writer before giving up (spec §6.2).
const BUSY_TIMEOUT: Duration = Duration::from_millis(2000);

pub struct Store {
    conn: Connection,
}

/// What a rebuild did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildStats {
    pub events: usize,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        enable_wal(&conn)?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Run `f` inside one `BEGIN IMMEDIATE` transaction; commit only if it succeeds.
    pub fn write<T>(&mut self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Run `f` inside one `BEGIN IMMEDIATE` transaction and roll it back: a dry run sees
    /// what `f` would write, and nothing is kept.
    pub fn dry_run<T>(&mut self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.rollback()?;
        Ok(out)
    }

    /// Append one event (and update its views) in its own transaction.
    pub fn append(&mut self, draft: EventDraft) -> Result<Event> {
        self.write(|tx| append_in(tx, draft))
    }

    /// Record or refresh a repo's name and root.
    pub fn register_repo(&self, repo: &RepoInfo) -> Result<()> {
        self.conn.execute(
            "INSERT INTO repos (id, name, root, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, root = excluded.root",
            params![
                repo.id,
                repo.name,
                repo.root.to_string_lossy(),
                jiff::Timestamp::now().to_string()
            ],
        )?;
        Ok(())
    }

    /// Empty every view table and replay all events in append order (one transaction).
    pub fn rebuild(&mut self) -> Result<RebuildStats> {
        self.write(|tx| {
            for table in crate::projection::VIEW_TABLES {
                tx.execute(&format!("DELETE FROM {table}"), [])?;
            }
            let events = read_all(tx, None)?;
            for event in &events {
                crate::projection::apply(tx, event)?;
            }
            Ok(RebuildStats {
                events: events.len(),
            })
        })
    }
}

/// The display name of a registered repo.
pub fn repo_name(conn: &Connection, repo_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT name FROM repos WHERE id = ?1", [repo_id], |r| {
            r.get(0)
        })
        .optional()?)
}

/// Switch the database to WAL. Changing the journal mode needs an exclusive lock, and
/// SQLite reports `SQLITE_BUSY` for it without consulting the busy handler, so several
/// processes opening a brand-new database at once must retry here themselves.
fn enable_wal(conn: &Connection) -> Result<()> {
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        let mode: String = conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        if mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("memory") {
            return Ok(());
        }
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == ErrorCode::DatabaseBusy && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            other => return other.map_err(Error::from),
        }
    }
}

/// The schema version recorded in `meta`, or 0 for a database without one.
fn schema_version(conn: &Connection) -> Result<i64> {
    let has_meta: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    if has_meta == 0 {
        return Ok(0);
    }
    Ok(conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

fn migrate(conn: &mut Connection) -> Result<()> {
    // Fast path for every open after the first: a plain read, no write lock, so many
    // hook processes starting at once don't queue behind each other.
    if schema_version(conn)? == SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Re-read under the write lock: another process may have migrated meanwhile.
    let current = schema_version(&tx)?;
    if current > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    if current < 1 {
        tx.execute_batch(schema::V1)?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', '1')",
            [],
        )?;
    }
    if current < 2 {
        tx.execute_batch(schema::V2)?;
    }
    if current < 3 {
        tx.execute_batch(schema::V3)?;
    }
    if current < 4 {
        tx.execute_batch(schema::V4)?;
    }
    if current < 5 {
        tx.execute_batch(schema::V5)?;
    }
    if current < 6 {
        tx.execute_batch(schema::V6)?;
    }
    if current < 7 {
        tx.execute_batch(schema::V7)?;
    }
    if current < 8 {
        tx.execute_batch(schema::V8)?;
    }
    if current < 9 {
        tx.execute_batch(schema::V9)?;
    }
    if current < 10 {
        tx.execute_batch(schema::V10)?;
    }
    tx.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
        [SCHEMA_VERSION.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

pub mod event;
pub mod schema;

#[cfg(test)]
mod tests {
    use super::{EventDraft, Store, read_all};
    use crate::error::Error;
    use crate::repo::RepoInfo;
    use serde_json::json;

    #[test]
    fn open_creates_schema_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        Store::open(&path).unwrap();
        let store = Store::open(&path).unwrap();
        let version: String = store
            .conn()
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION.to_string());
    }

    #[test]
    fn migrates_v1_database_to_v2() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::schema::V1).unwrap();
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '1')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO events (id, v, ts, repo_id, session_id, seq, kind, data)
                 VALUES ('01J', 1, '2026-10-13T09:00:00Z', 'r', 'owner', 1, 'note', '{}')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let version: String = store
            .conn()
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION.to_string());
        let events = read_all(store.conn(), None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "note");
        for table in ["sessions", "units", "task_time", "mcp_bindings"] {
            let n: i64 = store
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
    }

    #[test]
    fn migrates_v2_database_to_v3_keeping_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::schema::V1).unwrap();
            conn.execute_batch(super::schema::V2).unwrap();
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '2')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (session_id, repo_id, cwd, source, started_at, last_event_at,
                                       state, unit_count, active_secs)
                 VALUES ('s', 'r', '/x', 'startup', 't0', 't1', 'idle', 1, 42)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO units (session_id, unit_no, started_at, stopped_at, active_secs, files)
                 VALUES ('s', 1, 't0', 't1', 42, '[\"a.rs\"]')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let version: String = store
            .conn()
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION.to_string());
        let s = crate::session::get(store.conn(), "s").unwrap().unwrap();
        assert_eq!((s.active_secs, s.unit_count), (42, 1));
        assert_eq!(s.nav_state, crate::tangent::NavState::Green);
        assert_eq!((s.away_secs, s.current_detour), (0, None));
        let u = crate::session::units(store.conn(), "s").unwrap();
        assert_eq!(u[0].files, vec!["a.rs"]);
        assert_eq!((u[0].label, u[0].disagreement), (None, false));
        for table in [
            "detours",
            "suspended_items",
            "drift_samples",
            "repo_tuning",
            "notes_sent",
        ] {
            let n: i64 = store
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
    }

    #[test]
    fn migrates_v3_database_to_v4_keeping_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            for v in [super::schema::V1, super::schema::V2, super::schema::V3] {
                conn.execute_batch(v).unwrap();
            }
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '3')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (session_id, repo_id, cwd, source, started_at, last_event_at,
                                       state, unit_count, active_secs, nav_state)
                 VALUES ('s', 'r', '/x', 'startup', 't0', 't1', 'idle', 1, 42, 'amber')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let version: String = store
            .conn()
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION.to_string());
        let s = crate::session::get(store.conn(), "s").unwrap().unwrap();
        assert_eq!(
            (s.active_secs, s.nav_state),
            (42, crate::tangent::NavState::Amber)
        );
        assert_eq!(
            (s.worktree, s.end_kind, s.last_checkpoint),
            (None, None, None)
        );
        assert_eq!(s.readback, crate::handoff::ReadbackState::NotNeeded);
        assert_eq!((s.checkpoint_missed, s.leftovers_analysed), (0, false));
        for table in ["checkpoints", "leftovers", "handoff_notes", "claims_done"] {
            let n: i64 = store
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
    }

    #[test]
    fn migrates_v4_database_to_v5_keeping_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            for v in [
                super::schema::V1,
                super::schema::V2,
                super::schema::V3,
                super::schema::V4,
            ] {
                conn.execute_batch(v).unwrap();
            }
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '4')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO claims_done (repo_id, task_id, session_id, evidence, ts)
                 VALUES ('r', '1.1', 's', 'tests pass', 't0')",
                [],
            )
            .unwrap();
            // A V4-era claimed_done event, without files or symbols.
            conn.execute(
                "INSERT INTO events (id, v, ts, repo_id, session_id, seq, kind, data)
                 VALUES ('01J', 1, '2026-10-13T09:00:00Z', 'r', 's', 1, 'claimed_done',
                         '{\"task_id\":\"1.1\",\"evidence\":\"tests pass\"}')",
                [],
            )
            .unwrap();
        }
        let mut store = Store::open(&path).unwrap();
        let version: String = store
            .conn()
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION.to_string());
        let claims = crate::handoff::claims_done(store.conn(), "r").unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!((claims[0].files.len(), claims[0].symbols.len()), (0, 0));
        store.rebuild().unwrap();
        let claims = crate::handoff::claims_done(store.conn(), "r").unwrap();
        assert_eq!(claims[0].evidence, "tests pass");
        for table in [
            "verifications",
            "decisions",
            "verify_nudges",
            "repo_settings",
        ] {
            let n: i64 = store
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
    }

    #[test]
    fn migrates_v7_database_to_v8_keeping_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            for v in [
                super::schema::V1,
                super::schema::V2,
                super::schema::V3,
                super::schema::V4,
                super::schema::V5,
                super::schema::V6,
                super::schema::V7,
            ] {
                conn.execute_batch(v).unwrap();
            }
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '7')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (session_id, repo_id, cwd, source, started_at, last_event_at,
                                       state, unit_count, active_secs)
                 VALUES ('s', 'r', '/x', 'startup', 't0', 't1', 'idle', 1, 42)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO repo_settings (repo_id, publish_reports) VALUES ('r', 'never')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let version: String = store
            .conn()
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION.to_string());
        let s = crate::session::get(store.conn(), "s").unwrap().unwrap();
        assert_eq!(s.active_secs, 42);
        let set = crate::verify::settings(store.conn(), "r").unwrap();
        assert_eq!(set.publish_reports.as_deref(), Some("never"));
        for table in ["usage_replies", "transcript_cursors", "forecast_history"] {
            let n: i64 = store
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
    }

    #[test]
    fn migrates_v8_database_to_v9_keeping_detours() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            for v in [
                super::schema::V1,
                super::schema::V2,
                super::schema::V3,
                super::schema::V4,
                super::schema::V5,
                super::schema::V6,
                super::schema::V7,
                super::schema::V8,
            ] {
                conn.execute_batch(v).unwrap();
            }
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '8')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO detours (detour_id, session_id, repo_id, no, reason, resume_cue,
                                      opened_at)
                 VALUES ('D1', 's', 'r', 1, 'ci', 'back', 't0')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let d = crate::tangent::get_detour(store.conn(), "D1")
            .unwrap()
            .unwrap();
        assert_eq!((d.reason.as_str(), d.requested), ("ci", false));
    }

    #[test]
    fn migrates_v9_database_to_v10_keeping_plan_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            for v in [
                super::schema::V1,
                super::schema::V2,
                super::schema::V3,
                super::schema::V4,
                super::schema::V5,
                super::schema::V6,
                super::schema::V7,
                super::schema::V8,
                super::schema::V9,
            ] {
                conn.execute_batch(v).unwrap();
            }
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '9')",
                [],
            )
            .unwrap();
            for (id, parent, kind) in [
                ("G", None, "goal"),
                ("1", Some("G"), "milestone"),
                ("1.1", Some("1"), "task"),
                ("1.2", Some("1"), "task"),
            ] {
                conn.execute(
                    "INSERT INTO plan_nodes (repo_id, id, parent_id, kind, title, position)
                     VALUES ('r', ?1, ?2, ?3, ?1, 1)",
                    rusqlite::params![id, parent, kind],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO plan_deps (repo_id, node_id, depends_on) VALUES ('r', '1.2', '1.1')",
                [],
            )
            .unwrap();
            // A V9-era node_added event, without a source.
            conn.execute(
                "INSERT INTO events (id, v, ts, repo_id, session_id, seq, kind, data)
                 VALUES ('01J', 1, '2026-10-13T09:00:00Z', 'q', 'owner', 1, 'node_added',
                         '{\"id\":\"G\",\"parent_id\":null,\"kind\":\"goal\",\"title\":\"Ship\",\"intent\":\"why\"}')",
                [],
            )
            .unwrap();
        }
        let mut store = Store::open(&path).unwrap();
        let n = crate::plan::get_node(store.conn(), "r", "1.2")
            .unwrap()
            .unwrap();
        assert_eq!(n.depends_on, vec!["1.1".to_string()]);
        assert_eq!(n.external_deps, Vec::new());
        assert_eq!(
            (n.source, n.branch),
            (crate::plan::NodeSource::Manual, None)
        );
        let tree = crate::plan::load_tree(store.conn(), "r", true).unwrap();
        assert_eq!(tree.len(), 4);
        store.rebuild().unwrap();
        let g = crate::plan::get_node(store.conn(), "q", "G")
            .unwrap()
            .unwrap();
        assert_eq!(g.source, crate::plan::NodeSource::Manual);
        for table in ["plan_sources", "plan_source_items", "waits"] {
            let n: i64 = store
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
    }

    #[test]
    fn newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .conn()
            .execute(
                "UPDATE meta SET value = '99' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        drop(store);
        let Err(e) = Store::open(&path) else {
            panic!("a newer schema must be refused")
        };
        assert!(matches!(
            e,
            Error::SchemaTooNew {
                found: 99,
                supported: super::SCHEMA_VERSION
            }
        ));
        assert_eq!(
            e.to_string(),
            format!(
                "bridge-screen was updated while this session was open. Start a new Claude \
                 session to keep using it (data format 99; this session's copy understands up \
                 to {}).",
                super::SCHEMA_VERSION
            )
        );
    }

    #[test]
    fn append_assigns_seq_per_session() {
        let mut store = Store::open_in_memory().unwrap();
        let a1 = store
            .append(EventDraft::new("r", "s1", "note", json!({"n": 1})))
            .unwrap();
        let a2 = store
            .append(EventDraft::new("r", "s1", "note", json!({"n": 2})))
            .unwrap();
        let b1 = store
            .append(EventDraft::new("r", "s2", "note", json!({})))
            .unwrap();
        assert_eq!((a1.seq, a2.seq, b1.seq), (1, 2, 1));
        assert!(a1.pos < a2.pos && a2.pos < b1.pos);
        assert_eq!(a1.id.len(), 26);
        assert!(a1.ts.ends_with('Z'));
        let all = read_all(store.conn(), Some("r")).unwrap();
        assert_eq!(all, vec![a1, a2, b1]);
    }

    #[test]
    fn events_cannot_be_updated_or_deleted() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .append(EventDraft::new("r", "s", "note", json!({})))
            .unwrap();
        let upd = store.conn().execute("UPDATE events SET kind = 'x'", []);
        let del = store.conn().execute("DELETE FROM events", []);
        assert!(upd.unwrap_err().to_string().contains("append-only"));
        assert!(del.unwrap_err().to_string().contains("append-only"));
    }

    #[test]
    fn register_repo_upserts() {
        let store = Store::open_in_memory().unwrap();
        let mut repo = RepoInfo {
            id: "abc".into(),
            name: "one".into(),
            root: "/a".into(),
        };
        store.register_repo(&repo).unwrap();
        repo.name = "two".into();
        store.register_repo(&repo).unwrap();
        let (count, name): (i64, String) = store
            .conn()
            .query_row("SELECT count(*), max(name) FROM repos", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((count, name.as_str()), (1, "two"));
    }
}
