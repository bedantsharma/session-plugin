//! The SQLite store: the append-only event log plus view tables (spec §5–6).

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{Error, Result};
use crate::repo::RepoInfo;

pub use event::{Actor, EVENT_VERSION, Event, EventDraft, OWNER_SESSION, append_in, read_all};
pub use schema::SCHEMA_VERSION;

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
        conn.busy_timeout(Duration::from_millis(2000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
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

fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_meta: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    let current: i64 = if has_meta > 0 {
        tx.query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0)
    } else {
        0
    };
    if current > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    if current < 1 {
        tx.execute_batch(schema::V1)?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
            [SCHEMA_VERSION.to_string()],
        )?;
    }
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
        assert_eq!(version, "1");
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
        assert!(matches!(
            Store::open(&path),
            Err(Error::SchemaTooNew {
                found: 99,
                supported: 1
            })
        ));
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
