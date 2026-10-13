//! SQL schema. Version 1 is the first released schema; later versions append migrations.

pub const SCHEMA_VERSION: i64 = 2;

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

CREATE TABLE plan_nodes (
    repo_id        TEXT NOT NULL,
    id             TEXT NOT NULL,
    parent_id      TEXT,
    kind           TEXT NOT NULL,
    title          TEXT NOT NULL,
    intent         TEXT,
    estimate_min   INTEGER,
    status         TEXT NOT NULL DEFAULT 'todo',
    hill           TEXT,
    expected_files TEXT NOT NULL DEFAULT '[]',
    source         TEXT NOT NULL DEFAULT 'manual',
    position       INTEGER NOT NULL,
    next_child     INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (repo_id, id)
);

CREATE TABLE plan_deps (
    repo_id    TEXT NOT NULL,
    node_id    TEXT NOT NULL,
    depends_on TEXT NOT NULL,
    PRIMARY KEY (repo_id, node_id, depends_on)
);
";

/// Version 2: sessions, units of work, active time per task, and MCP call bindings.
pub const V2: &str = r"
CREATE TABLE sessions (
    session_id      TEXT PRIMARY KEY,
    repo_id         TEXT NOT NULL,
    cwd             TEXT NOT NULL,
    branch          TEXT,
    source          TEXT NOT NULL,
    model           TEXT,
    transcript_path TEXT,
    started_at      TEXT NOT NULL,
    last_event_at   TEXT NOT NULL,
    state           TEXT NOT NULL,
    active_task     TEXT,
    active_since    TEXT,
    unit_count      INTEGER NOT NULL DEFAULT 0,
    active_secs     INTEGER NOT NULL DEFAULT 0,
    ended_at        TEXT,
    end_reason      TEXT
);

CREATE INDEX sessions_repo_state ON sessions (repo_id, state);

CREATE TABLE units (
    session_id      TEXT NOT NULL,
    unit_no         INTEGER NOT NULL,
    prompt_id       TEXT,
    task_id         TEXT,
    started_at      TEXT NOT NULL,
    stopped_at      TEXT,
    waited_secs     INTEGER NOT NULL DEFAULT 0,
    wait_started_at TEXT,
    active_secs     INTEGER NOT NULL DEFAULT 0,
    files           TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (session_id, unit_no)
);

CREATE TABLE task_time (
    repo_id     TEXT NOT NULL,
    task_id     TEXT NOT NULL,
    active_secs INTEGER NOT NULL DEFAULT 0,
    units       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_id, task_id)
);

-- Operational, not a view: rebuild leaves it alone.
CREATE TABLE mcp_bindings (
    tool_use_id TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    tool_name   TEXT NOT NULL,
    args_hash   TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    consumed    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX mcp_bindings_match ON mcp_bindings (tool_name, args_hash, created_at);
";
