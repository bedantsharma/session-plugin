//! SQL schema. Version 1 is the first released schema; later versions append migrations.

pub const SCHEMA_VERSION: i64 = 1;

pub const V1: &str = r"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE repos (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    root       TEXT NOT NULL,
    settings   TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE TABLE events (
    pos        INTEGER PRIMARY KEY AUTOINCREMENT,
    id         TEXT NOT NULL UNIQUE,
    v          INTEGER NOT NULL,
    ts         TEXT NOT NULL,
    repo_id    TEXT NOT NULL,
    session_id TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    actor      TEXT NOT NULL DEFAULT '{}',
    task_id    TEXT,
    data       TEXT NOT NULL DEFAULT '{}',
    UNIQUE (session_id, seq)
);

CREATE INDEX events_repo_pos ON events (repo_id, pos);

CREATE TRIGGER events_no_update BEFORE UPDATE ON events
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;

CREATE TRIGGER events_no_delete BEFORE DELETE ON events
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;
";
