# Sub-project 2: Hooks and Sessions — Implementation Plan

> **For agentic workers:** Execute task-by-task, test-first. Steps use checkbox (`- [ ]`) syntax. This plan gives interfaces, behaviour rules and test cases; it deliberately contains no full code. Read the cited spec sections and the existing code before each task.

**Goal:** bridge-screen plugs into Claude Code: every hook records events, sessions and units of work are tracked with agent-active time, agents claim their active task (automatically when confident), an MCP server gives agents `claim_task` / `plan_edit` / `status`, and a plugin package plus `bridge-screen install` make it a one-command setup.

**Architecture:** The binary gains `hook <event>` (reads hook JSON on stdin, writes events, prints optional hook JSON on stdout, never fails the session), `mcp` (stdio MCP server built on `rmcp`), and `status`. A new core module `session` owns the session/unit/active-time views and their projector. Claiming lives in a new core module `claim`. The MCP server learns which session is calling by matching each call against a binding the `PreToolUse` hook recorded moments earlier. A `plugin/` folder holds the Claude Code plugin; the repo root holds the marketplace manifest.

**Tech Stack:** existing workspace (Rust 1.99, rusqlite, serde, clap) + `rmcp` 3.5.1 (server, stdio transport), `tokio` 1.53.2 (only for the MCP server), `schemars` 1.2.3 (tool input schemas). Add every dependency with `cargo add` — check `rmcp`'s feature names with `cargo add rmcp@3.5.1 --dry-run` / its docs.rs page before choosing features.

**Spec:** `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` — §4.1 (components, plugin package), §5.1–5.2 (events), §5.4–5.5 (session view, units, active time), §7.5 (note rules, for the claim reminder), §11.1–11.3 (hooks, MCP tools, skill), §16.2 (launcher).

## Global Constraints

- Everything from sub-project 1's Global Constraints still applies (toolchain, clippy pedantic `-D warnings`, no `unsafe`/`set_var`, data under `BRIDGE_SCREEN_HOME`, append-only events, hooks never bypassed).
- **A hook must never break a Claude session.** Any error inside `bridge-screen hook …` is appended to `<home>/hook-errors.log` (timestamp, event, message) and the process exits 0 with empty stdout. Panics are caught the same way.
- Hooks must be fast: the common path (session already known) must not spawn `git`. Target ≤ 50 ms p95 in release builds (spec §11.1).
- Hook stdout is either empty or a single JSON object in Claude Code's hook output format (`hookSpecificOutput.hookEventName` + `additionalContext`). Never print anything else.
- Hook input: parse leniently (unknown fields ignored, missing optional fields allowed). Fields used: `session_id`, `transcript_path`, `cwd`, `hook_event_name`, `prompt_id`, `agent_id`, `agent_type`, `source` (SessionStart), `model`, `prompt` (UserPromptSubmit), `tool_name`, `tool_input`, `tool_use_id`, `last_assistant_message` (Stop/SubagentStop), `notification_type` or the matcher value for Notification (verify the field name in https://code.claude.com/docs/en/hooks), `reason` (SessionEnd).
- Directories outside a git repo: hooks record nothing and print nothing.
- Plugin name `bridge-screen`, MCP server name `bridge-screen`, so tools are `mcp__plugin_bridge-screen_bridge-screen__<tool>`.
- Tests must never touch the real `~/.claude` or `~/.bridge-screen`, and must never run the real `claude` CLI (use a fake `claude` script on `PATH`).
- Commit per task; messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Hook failure isolation:** a corrupt/locked database, unparseable stdin, or a panic must still exit 0 with empty stdout and a log line. Test: `hook_never_fails_the_session` (Task 1).
2. **Two sessions in one repo at once:** events, units and active tasks must stay per session; a second session's prompt must not close the first's unit. Test: `parallel_sessions_stay_separate` (Task 3).
3. **MCP call attributed to the wrong session:** two sessions calling `claim_task` with identical arguments within a second must each get their own binding. Test: `identical_calls_bind_to_their_own_sessions` (Task 5).
4. **Active time:** a unit that waited 10 minutes on a permission prompt must count ~0 minutes of that wait; an idle gap between units must count nothing. Test: `waits_and_idle_gaps_are_not_active_time` (Task 2).
5. **Rebuild:** everything this sub-project adds to views must be reproduced exactly by `bridge-screen rebuild`. Test: extend `rebuild_reproduces_incremental_views` with a scripted session (Task 3).

---

## File Structure

```
crates/bridge-screen-core/src/
  store/schema.rs        # + V2 migration (session tables)
  store/mod.rs           # migrate 1 → 2
  projection.rs          # route session/claim events; add view tables
  session/mod.rs         # session + unit model, queries
  session/project.rs     # projector for session events
  claim.rs               # claim_task logic, candidate ranking
  binding.rs             # MCP call bindings (record, match)
  plan/ops.rs            # reject unknown fields in PlanOp
crates/bridge-screen/src/
  cli.rs                 # + hook, mcp, status, install flags
  hook/mod.rs            # stdin parse, dispatch, never-fail wrapper, output
  hook/handlers.rs       # one handler per event
  hook/context.rs        # text of injected context (session start, claim reminder)
  mcp.rs                 # rmcp server + tools
  status.rs              # compact text status
  install.rs             # plugin registration via the claude CLI
crates/bridge-screen/tests/
  hooks.rs, mcp.rs, status.rs, install.rs
  fixtures/hooks/*.json  # recorded-style hook payloads
plugin/
  .claude-plugin/plugin.json
  hooks/hooks.json
  .mcp.json
  skills/bridge-screen/SKILL.md
  commands/bridge-screen.md
  scripts/bridge-screen          # launcher (POSIX sh)
.claude-plugin/marketplace.json  # repo root: marketplace listing ./plugin
```

---

### Task 1: Hook plumbing — parse, dispatch, respond, never fail

**Files:** `crates/bridge-screen/src/cli.rs`, `src/hook/mod.rs`, `src/main.rs`, `tests/hooks.rs`, `tests/fixtures/hooks/`

**Interfaces:**
- CLI: `bridge-screen hook <EVENT>` where EVENT ∈ `session-start | user-prompt-submit | pre-tool-use | post-tool-use | stop | subagent-stop | pre-compact | notification | session-end` (kebab-case clap enum).
- `hook::HookInput` — serde struct with the fields listed in Global Constraints, all optional except `session_id` and `cwd`; `tool_input` stays `serde_json::Value`.
- `hook::HookOutput { event: HookEvent, additional_context: Option<String> }` → serialised as `{"hookSpecificOutput":{"hookEventName":"<PascalCase>","additionalContext":"…"}}`, or nothing when `additional_context` is `None`.
- `hook::run(event, stdin_text, home) -> Option<HookOutput>` — pure entry used by tests; `main` wraps it.

**Behaviour:**
```
main hook path:
  read all stdin
  result = catch_unwind(|| run(event, stdin, Home::resolve()))
  on Ok(Some(out)) → print out as JSON
  on Ok(None)      → print nothing
  on Err / panic   → append "<ts> <event> <message>" to <home>/hook-errors.log (ignore logging errors)
  exit 0 always
```
Handlers are stubs in this task (return `Ok(None)`); Task 3 fills them.

- [ ] Create fixtures: one JSON payload per event, shaped like the hook docs (include `session_id`, `cwd` placeholder, `transcript_path`, event-specific fields). Tests substitute `cwd` with a temp repo path.
- [ ] Write failing tests in `tests/hooks.rs`:
  - `each_event_is_accepted` — every fixture → exit 0, empty stdout.
  - `hook_never_fails_the_session` — (a) stdin `not json`, (b) `BRIDGE_SCREEN_HOME` pointing at a file instead of a folder, (c) DB file filled with garbage bytes → exit 0, empty stdout, and for (a)/(c) a line in `hook-errors.log` naming the event.
  - `output_shape` — unit test: `HookOutput` for `UserPromptSubmit` with context "x" serialises exactly to the documented shape.
- [ ] Implement; run `cargo test --workspace` and clippy; commit `hooks: input parsing, output format, never-fail wrapper`.

---

### Task 2: Session and unit model with active time

**Files:** `store/schema.rs`, `store/mod.rs`, `session/mod.rs`, `session/project.rs`, `projection.rs`, `lib.rs`

**Interfaces (core):**
- Schema **V2** (migration from 1 → 2 in the same `migrate` flow; `SCHEMA_VERSION = 2`):
  - `sessions(session_id PK, repo_id, cwd, branch, source, model, transcript_path, started_at, last_event_at, state, active_task, active_since, unit_count, active_secs, ended_at, end_reason)` — `state` ∈ `working | idle | needs_owner | ended`.
  - `units(session_id, unit_no, prompt_id, task_id, started_at, stopped_at, waited_secs, wait_started_at, active_secs, files JSON, PRIMARY KEY(session_id, unit_no))`.
  - `task_time(repo_id, task_id, active_secs, units, PRIMARY KEY(repo_id, task_id))`.
  - `mcp_bindings(tool_use_id PK, session_id, tool_name, args_hash, created_at, consumed)` — not a view (operational table, not rebuilt).
- Event kinds and `data` (all carry the envelope's `session_id`, `repo_id`, `ts`):
  - `session_started { cwd, branch, source, model, transcript_path }`
  - `prompt { prompt_id, chars }` — never store prompt text.
  - `tool_intent { tool, files[] }` (edit tools, before the call)
  - `file_changed { tool, files[] }` (edit tools, after a successful call)
  - `needs_owner { kind }` — kind ∈ `permission_prompt | idle_prompt | other`
  - `unit_stopped { unit_no, summary }` — `summary` = first 300 chars of `last_assistant_message`.
  - `subagent_stopped { agent_type, summary }`
  - `precompact {}`
  - `session_ended { reason }`
  - `task_claimed`, `task_released` — defined in Task 4; register their kinds here so the projector routes them.
- `session::project(tx, event)`; `session::get(conn, session_id) -> Option<Session>`; `session::current_unit(conn, session_id) -> Option<Unit>`; `session::list_live(conn, repo_id: Option<&str>) -> Vec<Session>` (state ≠ ended, newest first). `Session` / `Unit` structs mirror the tables.
- Add `sessions`, `units`, `task_time` to `VIEW_TABLES`.

**Projector rules (pseudocode):**
```
any event for a session:
  if unit open and unit.wait_started_at set and event.kind != needs_owner:
      unit.waited_secs += ts - wait_started_at; clear wait_started_at
  session.last_event_at = ts

session_started: upsert session (state idle, keep counters if it already exists — resume)
prompt:          if a unit is open, close it as if stopped at ts (missing Stop)
                 open unit_no = unit_count+1, task_id = session.active_task; state = working
needs_owner:     state = needs_owner
                 if kind == permission_prompt and unit open: wait_started_at = ts
file_changed:    add files to open unit's files (set semantics, keep order of first touch)
unit_stopped:    close unit: stopped_at = ts, active_secs = max(0, (ts - started_at) - waited_secs)
                 session.active_secs += active_secs; unit_count++ ; state = idle
                 if unit.task_id: task_time[task] += active_secs, units += 1
session_ended:   close open unit at ts; state = ended; ended_at; end_reason
```
Time arithmetic uses the event timestamps only (never "now"), so replay is deterministic.

- [ ] Failing tests (core, in-memory store, events appended with hand-set timestamps — add a test-only way to append with a given `ts`, e.g. `append_in_at(tx, draft, ts)`; keep `append_in` using now):
  - `migrates_v1_database_to_v2` — create a V1 DB (run V1 SQL + meta=1), open → version 2, old events intact.
  - `unit_lifecycle_and_active_time` — prompt t0, file_changed, stop t0+120s → unit 1 active 120, files listed, session idle, active_secs 120.
  - `waits_and_idle_gaps_are_not_active_time` — prompt t0, needs_owner(permission) t0+10, tool event t0+610, stop t0+620 → active 20; then 1 h gap, prompt, stop +30 s → session active 50.
  - `missing_stop_is_closed_by_next_prompt` and `session_end_closes_open_unit`.
  - `task_time_accumulates_per_task` — units with task_id set accumulate into `task_time`.
- [ ] Implement; extend `tests/rebuild.rs` with a scripted session and assert rebuild equality; commit `core: sessions, units and agent-active time`.

---

### Task 3: Hook handlers record events

**Files:** `hook/handlers.rs`, `hook/mod.rs`, `tests/hooks.rs`

**Behaviour per event (pseudocode):**
```
resolve session:
  if session_id known in sessions → reuse its repo_id (no git call)
  else → repo::identify(cwd); if NotARepo → return None; register repo
         (SessionStart creates the session; other events for an unknown session
          create it implicitly with source "unknown" — sessions started before install)

SessionStart        → session_started {cwd, branch (git rev-parse --abbrev-ref HEAD), source, model, transcript_path}
                      context: Task 4 supplies text; until then None
UserPromptSubmit    → prompt {prompt_id, chars}; context from Task 4
PreToolUse          → if tool ∈ {Edit, Write, MultiEdit, NotebookEdit}: tool_intent {files from tool_input}
                         (skip if every file already has a tool_intent in the open unit)
                      if tool starts with "mcp__plugin_bridge-screen_bridge-screen__": record binding (Task 5)
PostToolUse         → edit tools: file_changed {files}
Stop                → unit_stopped {unit_no, summary}
SubagentStop        → subagent_stopped {agent_type, summary}
PreCompact          → precompact {}
Notification        → needs_owner {kind}
SessionEnd          → session_ended {reason}
actor on every event: {agent_id, agent_type, model} from the payload
```
Paths in `files` are stored relative to the repo root when inside it, absolute otherwise.

- [ ] Failing tests (run the real binary via `CARGO_BIN_EXE_bridge-screen` with fixtures, temp repo, temp home, then inspect the DB with `Store::open`):
  - `session_flow_records_events` — start → prompt → pre/post Edit → stop → end; assert event kinds in order, unit 1 files = [the edited file], session ended.
  - `parallel_sessions_stay_separate` — interleave two session ids in the same repo; each has its own unit numbering and files.
  - `non_git_directory_is_ignored` — no events, empty stdout.
  - `known_session_does_not_run_git` — after SessionStart, put a fake failing `git` first on `PATH`; later hooks still record events.
  - `subagent_events_carry_actor` — PostToolUse with `agent_id`/`agent_type` → event actor set.
- [ ] Implement; commit `hooks: record session, unit and file events`.

---

### Task 4: Claiming the active task

**Files:** `crates/bridge-screen-core/src/claim.rs`, `session/project.rs`, `plan/project.rs` (status transitions), `hook/context.rs`, `hook/handlers.rs`

**Interfaces (core):**
- `claim::Candidate { task_id, title, score: f32, status }`; `claim::candidates(conn, repo_id, text, limit) -> Vec<Candidate>` — live tasks/subtasks with status `todo | active | failed`, ranked by keyword overlap of `text` with title + intent + expected file names + parent milestone title (lowercase word tokens, stop-words removed). Ties → plan order.
- `claim::ClaimRequest { session_id, repo_id, task: ClaimTarget, mode: Auto | Confirmed, confidence: f32 (0..=1), actor }`, `ClaimTarget = Existing(id) | New { parent, title, estimate_min }`.
- `claim::claim(store, req) -> Result<ClaimOutcome { task_id, created: bool, released: Option<String> }>` — one transaction: optional `plan_edit` add_node (New), then `task_claimed`.
- Events: `task_claimed { task_id, mode, confidence, previous }`, `task_released { task_id, reason }`.

**Rules:**
```
claim(existing id): task must be live and a task or subtask (not goal/milestone)
  previous = session.active_task
  if previous and previous != id → also emit task_released {previous, "switched"}
projector:
  task_claimed  → session.active_task = id, active_since = ts; open unit's task_id = id if unset;
                  plan node status: todo|failed → active (claimed/L1–L4 unchanged)
  task_released → if no other live session has it active and status == active → status todo
session_ended  → release its active task the same way (reason "session ended")
```

**Injected context (hook/context.rs), short and fixed-format:**
- SessionStart: `bridge-screen: tracking this session in <repo>. Goal: <goal title>.` + if the session resumed with an active task: `Active task: <id> <title>.` + `Claim your task with claim_task before working (see the bridge-screen skill).` when none.
- UserPromptSubmit, only while the session has no active task, at most once every 3 prompts:
  ```
  bridge-screen: no active task yet. Likely matches for this request:
    1.2 Login form (score .62)  ·  1.3 Session store (.31)  ·  2.1 Schema (.10)
  If one clearly fits, call claim_task with mode "auto". If unsure where the work stands, ask the owner. If none fits, claim a new task under the right parent.
  ```
  Candidates are computed from the prompt text (used in memory only, never stored).
- No plan in the repo → SessionStart says `No plan yet — ask the owner whether to create one (bridge-screen plan edit / plan_edit tool).` and no claim reminders.

- [ ] Failing tests: candidate ranking (exact order on a small plan); claim existing/new/switch/release; status transitions incl. two sessions on one task; rejects goal/milestone/dropped; reminder cadence (prompts 1 and 4 get it, 2–3 don't, none after a claim); SessionStart text with and without a plan; rebuild equality still holds.
- [ ] Implement; commit `core: claim the active task; hooks: session-start and claim reminders`.

---

### Task 5: MCP server with session binding

**Files:** `crates/bridge-screen-core/src/binding.rs`, `crates/bridge-screen/src/mcp.rs`, `cli.rs`, `hook/handlers.rs`, `plan/ops.rs`, `tests/mcp.rs`

**Session binding (why: Claude Code doesn't tell MCP servers which session is calling, but `PreToolUse` fires for MCP tools with `session_id`, `tool_name`, `tool_input`, `tool_use_id`):**
```
PreToolUse for our tools  → binding.record(tool_use_id, session_id, tool_name, hash(canonical_json(tool_input)))
MCP tool call arrives     → binding.take(tool_name, hash(canonical_json(args))):
                              newest unconsumed match created within the last 30 s → mark consumed, return session_id
                            no match → fallback: newest live session whose cwd is inside CLAUDE_PROJECT_DIR (env of the MCP process)
                            still none → tool error "bridge-screen couldn't tell which session this is; is the plugin's hook enabled?"
bindings older than 1 h are deleted on each record()
canonical_json = keys sorted, no whitespace
```

**Tools (rmcp, stdio; `bridge-screen mcp`):**
| Tool | Input | Result text |
|---|---|---|
| `claim_task` | `{ id? , new_task? {parent, title, estimate_min?}, mode: "auto"\|"confirmed", confidence: number }` — exactly one of `id` / `new_task` | `Claimed 1.2 Login form.` (+ `Released 1.1.` if switched) |
| `plan_edit` | `{ ops: PlanOp[], reason?, replan? }` | `Applied N operation(s). Added: …` or the plan error message |
| `status` | `{}` | compact text: active task, unit count, active minutes, plan goal and next 3 todo tasks |

Input schemas come from `schemars` derives so agents see field docs. Errors are returned as tool errors with the core error's message, never as protocol failures.

- [ ] Make `PlanOp` reject unknown fields (settles the follow-up from PR #1; the MCP schema makes typos visible). Verify with a test that `{"op":"set_estimate","id":"1","estimate":30}` fails to parse; check serde's behaviour for internally tagged enums and pick the approach that actually rejects it.
- [ ] Failing tests:
  - core `binding`: record/take round-trip; `identical_calls_bind_to_their_own_sessions` (two sessions, same tool+args, bindings consumed newest-first, each session gets one); expiry after 30 s; cleanup after 1 h.
  - `tests/mcp.rs`: spawn `bridge-screen mcp` with temp home; JSON-RPC over stdio: `initialize`, `tools/list` (three tools with schemas), then `tools/call claim_task` after inserting a matching binding via the `hook pre-tool-use` binary path → task claimed for the right session; a call with no binding and no fallback returns a tool error.
  - `plan_edit` through MCP adds nodes; invalid op → tool error text starts with `op 0:`.
- [ ] Implement; commit `mcp: stdio server with claim_task, plan_edit, status and session binding`.

---

### Task 6: `bridge-screen status` and `/bridge-screen`

**Files:** `crates/bridge-screen/src/status.rs`, `cli.rs`, `tests/status.rs`, `plugin/commands/bridge-screen.md`

**Output (plain text, ≤ ~12 lines, no colour yet — colours arrive with the screens sub-project):**
```
bridge-screen · 2 live sessions · 1 project
<repo>  goal: <goal title>
  S <short id>  working   1.2 Login form        3 units · 18m
  S <short id>  needs you (no task)             1 unit  · 2m
```
`short id` = first 6 chars of the session id. Order: needs-you first, then working, then idle. No live sessions → `bridge-screen · no live sessions`.

- [ ] Failing tests with a scripted DB (sessions in two states, one with a task) → exact text.
- [ ] `plugin/commands/bridge-screen.md`: tells Claude to run `bridge-screen status` and show the output verbatim (verify the command-file front-matter keys, e.g. `allowed-tools`, in the plugin docs).
- [ ] Commit `cli: status command and /bridge-screen`.

---

### Task 7: Plugin package, launcher, and install

**Files:** `plugin/**`, `.claude-plugin/marketplace.json`, `crates/bridge-screen/src/install.rs`, `cli.rs`, `tests/install.rs`

**Plugin contents:**
- `plugin.json`: name `bridge-screen`, version = workspace version, description, author, repository, homepage.
- `hooks/hooks.json`: one entry per event in Task 1, exec form, command `${CLAUDE_PLUGIN_ROOT}/scripts/bridge-screen` with args `["hook", "<event>"]`. PreToolUse/PostToolUse: no matcher restriction (the binary filters). Verify exec-form field names in the hooks docs.
- `.mcp.json`: server `bridge-screen` → command `${CLAUDE_PLUGIN_ROOT}/scripts/bridge-screen`, args `["mcp"]`.
- `skills/bridge-screen/SKILL.md` (≤ 80 lines for now): what bridge-screen is; claim your task first (auto when the prompt clearly matches a candidate, otherwise ask the owner where things stand); create a new task under the right parent when nothing fits, with an estimate in agent-minutes; use `plan_edit` for plan changes and give a reason; goals and milestones need a 2–3 sentence intent (purpose + end state); `status` to check state. Later sub-projects extend it.
- Root `.claude-plugin/marketplace.json`: marketplace `bridge-screen` listing plugin `bridge-screen` at `./plugin`.

**Launcher `scripts/bridge-screen` (POSIX sh, spec §16.2):**
```
want = version from plugin.json
if `bridge-screen` on PATH and its --version == want → exec it with "$@"
bin = $CLAUDE_PLUGIN_DATA/bin/bridge-screen-<want>
if missing:
   detect os/arch → target triple (darwin arm64/x86_64, linux x86_64/aarch64)
   base = ${BRIDGE_SCREEN_RELEASE_BASE:-https://github.com/bedantsharma/session-plugin/releases/download/v<want>}
   download archive + .sha256 (curl -fsSL), verify sha256, extract to bin (atomic rename)
   on any failure: if called as "hook …" → exit 0 silently (log to $CLAUDE_PLUGIN_DATA/launcher.log)
                   else → print a clear error with install alternatives, exit 1
exec "$bin" "$@"
```
Match the archive/checksum names to what `dist plan` lists.

**`bridge-screen install` (extends the existing command):**
```
existing behaviour (home, db, config) unchanged
then, unless --no-plugin:
  source = --dev ? <repo checkout path containing .claude-plugin/marketplace.json> : "bedantsharma/session-plugin"
  run: claude plugin marketplace add <source>
  run: claude plugin install bridge-screen@bridge-screen
  (verify exact subcommands/flags with `claude plugin --help` and the plugin CLI docs; already-added/installed must count as success)
  claude not on PATH → print the two commands for the user to run in Claude Code, exit 0
```

- [ ] Failing tests (`tests/install.rs`): a fake `claude` script on `PATH` that appends its args to a file → `install --dev` calls marketplace add with the checkout path, then install; `--no-plugin` calls nothing; no `claude` on PATH → prints manual instructions, exit 0; fake `claude` failing on "already installed" → still success.
- [ ] Launcher test (shell-driven from a Rust test, macOS/Linux only): serve a fake release from a temp dir via `BRIDGE_SCREEN_RELEASE_BASE=file://…` with a correct and a wrong checksum → correct one runs `--version`; wrong one fails safely (hook mode exits 0, other mode exits 1).
- [ ] Run `claude plugin validate ./plugin` locally if the `claude` CLI is available (read-only check); fix anything it reports. Do **not** run `bridge-screen install` against the real Claude config.
- [ ] Commit `plugin: package, launcher and install registration`.

---

### Task 8: Latency check and docs

**Files:** `crates/bridge-screen/tests/hooks.rs` (perf test), `CLAUDE.md`, `README.md` (create, short)

- [ ] Perf test, `#[ignore]` so it only runs on demand: release binary, DB pre-filled with 100k events across 50 sessions; run 200 `hook post-tool-use` calls for a known session; assert p95 < 50 ms. Document how to run it (`cargo test --release -p bridge-screen -- --ignored hook_latency`). Run it once and report the numbers.
- [ ] `README.md`: one paragraph on what bridge-screen is, install options (from spec §16.1, marked "after the first release"), and "Try it from a checkout": `cargo install --path crates/bridge-screen` then `bridge-screen install --dev`.
- [ ] `CLAUDE.md`: add the plugin layout and the `hook`/`mcp`/`status` commands to Layout/Commands.
- [ ] Commit `docs: README, plugin layout, latency check`.
