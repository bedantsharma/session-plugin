//! SQL schema. Version 1 is the first released schema; later versions append migrations.

pub const SCHEMA_VERSION: i64 = 10;

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

/// Version 3: tangents — detours, suspended items, unit labels, away time, navigator notes
/// and per-repo drift tuning (spec §7).
pub const V3: &str = r"
ALTER TABLE units ADD COLUMN label TEXT;
ALTER TABLE units ADD COLUMN label_source TEXT;
ALTER TABLE units ADD COLUMN drift_score REAL;
ALTER TABLE units ADD COLUMN detour_id TEXT;
ALTER TABLE units ADD COLUMN disagreement INTEGER NOT NULL DEFAULT 0;
ALTER TABLE units ADD COLUMN flagged INTEGER NOT NULL DEFAULT 0;

CREATE INDEX units_task ON units (task_id);

ALTER TABLE sessions ADD COLUMN current_detour TEXT;
ALTER TABLE sessions ADD COLUMN away_secs INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN away_since TEXT;
ALTER TABLE sessions ADD COLUMN nav_state TEXT NOT NULL DEFAULT 'green';
ALTER TABLE sessions ADD COLUMN ack_away_secs INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN ack_unit INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN pending_reply TEXT;

CREATE TABLE detours (
    detour_id     TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL,
    repo_id       TEXT NOT NULL,
    no            INTEGER NOT NULL,
    blocks_task   TEXT,
    reason        TEXT NOT NULL,
    resume_cue    TEXT NOT NULL,
    opened_at     TEXT NOT NULL,
    opened_unit   INTEGER,
    parent_detour TEXT,
    closed_at     TEXT,
    outcome       TEXT,
    active_secs   INTEGER NOT NULL DEFAULT 0,
    promoted_task TEXT
);

CREATE INDEX detours_session ON detours (session_id);
CREATE INDEX detours_task ON detours (repo_id, blocks_task);

CREATE TABLE suspended_items (
    item_id      TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    kind         TEXT NOT NULL,
    ref_id       TEXT NOT NULL,
    resume_cue   TEXT NOT NULL,
    by_detour    TEXT,
    suspended_at TEXT NOT NULL,
    resumed_at   TEXT
);

CREATE INDEX suspended_items_session ON suspended_items (session_id);

CREATE TABLE drift_samples (
    repo_id  TEXT NOT NULL,
    unit_ref TEXT NOT NULL,
    score    REAL NOT NULL,
    label    TEXT NOT NULL,
    source   TEXT NOT NULL,
    ts       TEXT NOT NULL
);

CREATE INDEX drift_samples_repo ON drift_samples (repo_id);

CREATE TABLE repo_tuning (
    repo_id             TEXT PRIMARY KEY,
    drift_lower         REAL,
    drift_upper         REAL,
    tolerance_pct       REAL,
    samples_at_last_run INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE notes_sent (
    session_id TEXT NOT NULL,
    unit_no    INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    state_key  TEXT NOT NULL,
    channel    TEXT NOT NULL,
    ts         TEXT NOT NULL
);

CREATE INDEX notes_sent_session ON notes_sent (session_id, unit_no);
";

/// Version 4: handoff and recovery — checkpoints, end classification, leftovers, handoff
/// notes, read-back and claimed-done records (spec §8).
pub const V4: &str = r"
ALTER TABLE sessions ADD COLUMN worktree TEXT;
ALTER TABLE sessions ADD COLUMN end_kind TEXT;
ALTER TABLE sessions ADD COLUMN last_checkpoint TEXT;
ALTER TABLE sessions ADD COLUMN checkpoint_pending TEXT;
ALTER TABLE sessions ADD COLUMN checkpoint_asked_unit INTEGER;
ALTER TABLE sessions ADD COLUMN checkpoint_missed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN readback TEXT NOT NULL DEFAULT 'not_needed';
ALTER TABLE sessions ADD COLUMN briefing_status TEXT;
ALTER TABLE sessions ADD COLUMN last_task TEXT;
ALTER TABLE sessions ADD COLUMN leftovers_analysed INTEGER NOT NULL DEFAULT 0;

CREATE TABLE checkpoints (
    sha         TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    repo_id     TEXT NOT NULL,
    task_id     TEXT,
    next_action TEXT,
    ts          TEXT NOT NULL
);

CREATE INDEX checkpoints_session ON checkpoints (session_id, ts);

CREATE TABLE leftovers (
    leftover_id  TEXT PRIMARY KEY,
    repo_id      TEXT NOT NULL,
    from_session TEXT NOT NULL,
    worktree     TEXT NOT NULL,
    path         TEXT NOT NULL,
    task_id      TEXT,
    suggested    TEXT NOT NULL,
    state        TEXT NOT NULL,
    resolution   TEXT,
    stash_ref    TEXT,
    commit_sha   TEXT,
    resolved_by  TEXT,
    ts           TEXT NOT NULL,
    resolved_at  TEXT
);

CREATE INDEX leftovers_repo_state ON leftovers (repo_id, state);

CREATE TABLE handoff_notes (
    session_id TEXT NOT NULL,
    repo_id    TEXT NOT NULL,
    task_id    TEXT,
    in_flight  TEXT NOT NULL,
    if_then    TEXT NOT NULL DEFAULT '[]',
    ts         TEXT NOT NULL
);

CREATE INDEX handoff_notes_task ON handoff_notes (repo_id, task_id);

CREATE TABLE claims_done (
    repo_id    TEXT NOT NULL,
    task_id    TEXT NOT NULL,
    session_id TEXT NOT NULL,
    evidence   TEXT NOT NULL,
    ts         TEXT NOT NULL
);

CREATE INDEX claims_done_task ON claims_done (repo_id, task_id);
";

/// Version 5: verification and decisions — the verification ladder with its commit SHAs,
/// nudges about unverified work, per-repo test settings, decisions with supersede chains,
/// and the decision-capture flag (spec §8.5–8.6).
pub const V5: &str = r"
ALTER TABLE sessions ADD COLUMN decision_capture TEXT;
ALTER TABLE claims_done ADD COLUMN files TEXT NOT NULL DEFAULT '[]';
ALTER TABLE claims_done ADD COLUMN symbols TEXT NOT NULL DEFAULT '[]';

CREATE TABLE verifications (
    verif_id          TEXT PRIMARY KEY,
    repo_id           TEXT NOT NULL,
    task_id           TEXT NOT NULL,
    level             INTEGER NOT NULL,
    orig_level        INTEGER NOT NULL,
    sha               TEXT NOT NULL,
    files             TEXT NOT NULL DEFAULT '[]',
    evidence          TEXT NOT NULL,
    verified_by       TEXT NOT NULL,
    rerun             TEXT,
    session_id        TEXT NOT NULL,
    state             TEXT NOT NULL,
    stale_checked_sha TEXT NOT NULL,
    ts                TEXT NOT NULL
);

CREATE INDEX verifications_task ON verifications (repo_id, task_id);

CREATE TABLE decisions (
    decision_id   TEXT NOT NULL,
    repo_id       TEXT NOT NULL,
    task_id       TEXT,
    session_id    TEXT NOT NULL,
    what          TEXT NOT NULL,
    what_key      TEXT NOT NULL,
    why           TEXT NOT NULL,
    alternatives  TEXT NOT NULL DEFAULT '[]',
    supersedes    TEXT,
    superseded_by TEXT,
    merged_count  INTEGER NOT NULL DEFAULT 1,
    ts            TEXT NOT NULL,
    updated_ts    TEXT NOT NULL,
    PRIMARY KEY (repo_id, decision_id)
);

CREATE INDEX decisions_key ON decisions (repo_id, task_id, what_key);

CREATE TABLE verify_nudges (
    repo_id        TEXT NOT NULL,
    task_id        TEXT NOT NULL,
    nudges         INTEGER NOT NULL DEFAULT 0,
    level_at_first INTEGER NOT NULL,
    escalated      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_id, task_id)
);

CREATE TABLE repo_settings (
    repo_id        TEXT PRIMARY KEY,
    test_command   TEXT,
    test_timeout_s INTEGER
);
";

/// Version 6: the question a session asked the owner (spec §10.1): set by
/// `owner_question`, cleared when the owner answers (or the session ends).
pub const V6: &str = r"
ALTER TABLE sessions ADD COLUMN question_unit INTEGER;
ALTER TABLE sessions ADD COLUMN question_source TEXT;
ALTER TABLE sessions ADD COLUMN question_since TEXT;
";

/// Version 7: the per-repo `publish_reports` switch (`never` or `ask`, spec §10.6).
pub const V7: &str = r"
ALTER TABLE repo_settings ADD COLUMN publish_reports TEXT;
";

/// Version 8: telemetry and forecast (spec §6.2, §9.2–9.3) — each model reply's tokens,
/// attributed to its unit, task, detour and agent; a byte cursor per transcript file;
/// and the history of forecasts. Money is never stored: it is computed when read.
pub const V8: &str = r"
CREATE TABLE usage_replies (
    message_id     TEXT PRIMARY KEY,
    repo_id        TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    agent_id       TEXT,
    agent_type     TEXT,
    model          TEXT NOT NULL,
    ts             TEXT NOT NULL,
    unit_no        INTEGER,
    task_id        TEXT,
    detour_id      TEXT,
    input          INTEGER NOT NULL DEFAULT 0,
    output         INTEGER NOT NULL DEFAULT 0,
    cache_read     INTEGER NOT NULL DEFAULT 0,
    cache_write_5m INTEGER NOT NULL DEFAULT 0,
    cache_write_1h INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX usage_replies_task ON usage_replies (repo_id, task_id);
CREATE INDEX usage_replies_session ON usage_replies (session_id);

CREATE TABLE transcript_cursors (
    path       TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id   TEXT,
    offset     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE forecast_history (
    repo_id       TEXT NOT NULL,
    ts            TEXT NOT NULL,
    p50_hours     REAL NOT NULL,
    p90_hours     REAL NOT NULL,
    p50_date      TEXT,
    p90_date      TEXT,
    remaining_min INTEGER NOT NULL,
    unestimated   INTEGER NOT NULL,
    points        INTEGER NOT NULL,
    hours_per_day REAL,
    reason        TEXT
);

CREATE INDEX forecast_history_repo ON forecast_history (repo_id, ts);
";

/// Version 9: the polish round — a detour the owner asked for (`requested`, from
/// `detour_opened`; never drift or away time); for each verification a failure wiped,
/// which failure (`failed_by`, its event id) and the state it had (`failed_from`), so the
/// owner can take a "broken" verdict back; and each transcript cursor's file identity
/// (device, inode and creation time; never its contents), so a replaced transcript
/// restarts from 0.
pub const V9: &str = r"
ALTER TABLE detours ADD COLUMN requested INTEGER NOT NULL DEFAULT 0;
ALTER TABLE verifications ADD COLUMN failed_by TEXT;
ALTER TABLE verifications ADD COLUMN failed_from TEXT;
ALTER TABLE transcript_cursors ADD COLUMN identity TEXT;
";

/// Version 10: imports and onboarding (spec §5.2–5.3, §9.1, §12.1–12.2) — the branch a task
/// was last claimed on; plan links that may point at a task in another repo
/// (`dep_repo_id`, the same repo for every older link); the plan files imported or seen
/// (`plan_sources`) and which node each of their tasks and steps became
/// (`plan_source_items`); and ad-hoc waits (`waiting_on`).
pub const V10: &str = r"
ALTER TABLE plan_nodes ADD COLUMN branch TEXT;

CREATE TABLE plan_deps_v10 (
    repo_id     TEXT NOT NULL,
    node_id     TEXT NOT NULL,
    dep_repo_id TEXT NOT NULL,
    depends_on  TEXT NOT NULL,
    PRIMARY KEY (repo_id, node_id, dep_repo_id, depends_on)
);
INSERT INTO plan_deps_v10 (repo_id, node_id, dep_repo_id, depends_on)
    SELECT repo_id, node_id, repo_id, depends_on FROM plan_deps;
DROP TABLE plan_deps;
ALTER TABLE plan_deps_v10 RENAME TO plan_deps;
CREATE INDEX plan_deps_target ON plan_deps (dep_repo_id, depends_on);

CREATE TABLE plan_sources (
    repo_id     TEXT NOT NULL,
    path        TEXT NOT NULL,
    hash        TEXT NOT NULL,
    size        INTEGER NOT NULL,
    mtime       INTEGER NOT NULL,
    node_id     TEXT,
    baseline    INTEGER NOT NULL DEFAULT 0,
    imported_at TEXT NOT NULL,
    needs_link  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_id, path)
);

CREATE TABLE plan_source_items (
    repo_id TEXT NOT NULL,
    path    TEXT NOT NULL,
    task_no INTEGER NOT NULL,
    step_no INTEGER NOT NULL DEFAULT 0,
    node_id TEXT NOT NULL,
    title   TEXT NOT NULL,
    PRIMARY KEY (repo_id, path, task_no, step_no)
);

CREATE TABLE waits (
    wait_id        TEXT PRIMARY KEY,
    repo_id        TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    target_repo_id TEXT,
    target_task    TEXT,
    target_text    TEXT NOT NULL,
    reason         TEXT NOT NULL,
    opened_at      TEXT NOT NULL,
    cleared_at     TEXT,
    cleared_why    TEXT,
    cleared_sha    TEXT
);

CREATE INDEX waits_target ON waits (target_repo_id, target_task, cleared_at);
CREATE INDEX waits_session ON waits (session_id);
";
