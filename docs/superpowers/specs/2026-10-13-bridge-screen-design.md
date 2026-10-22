# bridge-screen — Design Spec

- **Date:** 2026-10-13
- **Status:** Draft for owner review (revision 2: owner answers to open questions applied)
- **Source brief:** `agent-tracker-brief.md`
- **Name:** `bridge-screen` (package, plugin, binary, command, and data folder `~/.bridge-screen`). It bridges the gap between what the agents are doing and the owner's understanding of the project.

---

## 1. Purpose and success criteria

The owner builds 2-day to 2-month projects almost entirely through Claude Code, often with 1–N parallel sessions, each in its own git worktree. bridge-screen answers three questions at a glance, without the owner nagging agents or reading walls of text:

1. **Distance:** how close is each project to its goal?
2. **Direction:** is each session advancing its task, or is it on a tangent?
3. **Leftovers:** what did dead or abandoned sessions leave behind, and has it been dealt with?

**Success criteria**
- The owner no longer maintains a `roadmap.md` by hand.
- The glance screen answers all three questions in under 5 seconds.
- A new session resumes a crashed session's work without the owner re-explaining it.
- No bridge-screen state is ever written inside a repo, apart from the WIP commits the owner approved.

**Scope.** The full feature set from brief §3. Implementation is split into sub-projects (§13), but none are cut.

## 2. Decisions taken during design

| Topic | Decision |
|---|---|
| Steering model | **Navigator.** bridge-screen records events and puts short notes into the agent's context at key moments. It never blocks work. The one exception is the checkpoint commit (§8.1). |
| Claiming the active task | The agent claims it **automatically when confident** and asks the owner only when unsure where the work stands or where to start. Every claim is logged and visible. |
| Plan source of truth | The **bridge-screen's store**. Superpowers plan files are imported, and later changes to them are recorded as plan-edit or re-plan events. Nothing is ever written back to them. |
| Estimate unit | **Agent-minutes.** Points are a display unit only (default 1 pt = 15 agent-minutes, configurable). Calibration is kept per model. |
| Checkpoints | **Real WIP commits** on the worktree branch, squashed at the end (`bridge-screen squash` or squash-merge). |
| Dashboard | **Can act.** It has a needs-you queue with buttons, and every action is also available in chat and through the CLI. |
| Dependencies | **Both** plan-level task links (inherited by sessions) and ad-hoc `waiting_on` declarations, which the owner can promote to plan links. |
| Grouping | Each repo is a project. Worktrees with the **same branch name in different repos** form one group. No ticket regex and no manual tags. |
| Progress threshold | `done%` counts leaves verified at **L2 (unit tests) or above**. |
| Architecture | A Claude Code plugin (hooks, MCP server, skill) plus a CLI, with no background process. |
| Stack | **Rust** backend in a single binary. **Svelte 5 + Tailwind CSS + shadcn-svelte + LayerChart** frontend (Vite), embedded in the binary. **ratatui** terminal UI. **SQLite** (WAL) data layer. |
| Screens | Three: terminal (`status` and live `watch`), Claude desktop app (inline MCP Apps card), and the browser dashboard (§10) |
| Publishing | Everything stays local. The single exception: the owner may explicitly ask Claude to publish a progress report as a claude.ai Artifact (§10.6) |
| Distribution | Claude Code plugin marketplace as the main route; Homebrew tap, PyPI, npm and a shell installer as alternates, all from one release pipeline (§16) |
| Drift pause | **Opt-in per repo** and off by default. When on, confirmed drift makes the agent finish its step, commit, and wait for the owner. |
| Visual theme | Claude Code palette (§10.4). |
| Name | `bridge-screen` |
| Crash timeout | 30 minutes of silence |
| Squash timing | Automatic when a PR is opened from Claude Code; manual command otherwise |
| Needs-you escalation | More than 4 items |
| Drift thresholds | Auto-tuned per repo, with guard rails and one-click revert |
| Retention | Projects inactive 15 days are archived; in active projects, detail over 16 MB and older than 2 days is compacted. Search and restore tooling. |

## 3. Principles drawn from other domains

These ideas from other fields shaped the design. Each was checked against its primary source.

| Source | Principle | Where it is applied |
|---|---|---|
| Commander's intent (USMC) | Judge deviations against a short purpose and end state, not against the task list. Intent is known two levels up. | Intent on the goal and milestones; drift judged against both (§7) |
| Aviation cross-track error | Measure distance from the route inside a tolerance band. Brief overshoots are normal. | Drift tolerance band (§7.3) |
| Altmann & Trafton, memory for goals | Resumption cues work only if encoded **at the moment of suspension**. Returns are not strictly last-in-first-out. | Resume cues on detours, checkpoints and stops; a suspended-items list, not a stack (§7.4) |
| ARIES / write-ahead logging | Log intent before changing data; checkpoints; analysis before recovery; log recovery actions so they are never repeated; avoid long transactions. | Intent records, WIP checkpoints, recovery pass, `leftover_resolved` (§8) |
| I-PASS (Starmer et al., NEJM 2014) | Structured handoff with severity, summary, actions, contingencies, and a read-back by the receiver. Errors fell 23%, but only as a *bundle* that checked the handoff was actually done. | Briefing format and enforced read-back (§8.3) |
| Evidence-Based Scheduling (Spolsky) | Calibrate with each estimator's actual-vs-estimate history and forecast a date distribution. Interruptions stay in the actuals. | Calibration and P50/P90 forecast (§9.3) |
| Shape Up hill charts | Percent complete is meaningless while unknowns remain (uphill). Watch for stuck work. | Uphill/downhill state per milestone (§5.3) |
| Toyota andon / jidoka | Few signals, each with a defined trigger and response. Unanswered signals breed fatigue. | Glance board, needs-you queue, Navigator rate limits (§7.5, §10) |
| Event sourcing (Microsoft) | An append-only log is the source of truth and views can be rebuilt. Keep it minimal for small tools. | `events` table plus view tables (§5, §6) |

## 4. Architecture

### 4.1 Components

A single Rust binary, `bridge-screen`, with four entry points over one core library:

| Command | Role | Invoked by |
|---|---|---|
| `bridge-screen hook <event>` | Reads the hook JSON from stdin, records events, runs cheap checks, and prints a Navigator note when one is due | Claude Code, on every hook event |
| `bridge-screen mcp` | MCP server over stdio that provides the agent's tools | Claude Code, once per session |
| `bridge-screen ui` | Local dashboard on `127.0.0.1`. Single-instance: if one is already running it prints the existing URL. | The owner, by hand |
| `bridge-screen <cmd>` | CLI: `install`, `init`, `import`, `status`, `squash`, `verify`, `resolve`, `rebuild`, `config` | The owner, or the agent through Bash |

**Plugin package**
- `hooks/hooks.json`: every hook entry runs `bridge-screen hook <event>`.
- `.mcp.json`: runs `bridge-screen mcp`.
- `skills/bridge-screen/SKILL.md`: the agent protocol (claiming, detours and resume cues, read-back, decisions, verification, checkpoints).
- `commands/bridge-screen.md`: the `/bridge-screen` compact status.

`bridge-screen install` puts the binary on the PATH, registers and enables the plugin, and creates `~/.bridge-screen/`. Nothing is needed per session.

**Nothing runs between sessions.** Hooks are short-lived processes. The MCP server lives only as long as its session. The dashboard runs only when the owner starts it.

### 4.2 Core modules

| Module | Responsibility | Depends on |
|---|---|---|
| `store` | SQLite access, event append plus view updates in a single transaction, migrations, rebuild. The only module that touches disk. | — |
| `plan` | Plan tree, edits and re-plans, intents, estimates, dependencies, hill state, superpowers import and diff | store |
| `session` | Session lifecycle, active task, suspended items, unit accounting, active time, end-of-session classification (clean, dirty, crashed) | store, plan |
| `classify` | Drift score at Stop | plan, session |
| `navigator` | Decides whether to send a note and which one; rate limits; note text | session, plan, classify, recovery |
| `recovery` | Leftover analysis, briefing, read-back check, adopt/resume/undo | session, git, store |
| `verify` | Verification ladder, staleness | plan, git |
| `telemetry` | Incremental transcript parsing, tokens, cost | store |
| `forecast` | Calibration ratios, Monte Carlo P50/P90 | plan, store |
| `git` | Status, diff, commit, stash, squash, SHA lookups. Runs the `git` command so the user's config and hooks apply. | — |
| `web` | axum server, JSON API, embedded SPA, auth token | all read-side modules |
| `mcp` | MCP tool definitions over the core | all |

Each module is usable and testable without the CLI, MCP or web layers.

### 4.3 Frontend

- **Svelte 5** SPA built with Vite into static assets and embedded with `rust-embed`. Node is needed **at build time only**.
- **Tailwind CSS** for styling, with the Claude Code palette (§10.4) defined once as theme tokens.
- **shadcn-svelte** components (cards, dialogs, tables, tooltips, command palette), copied into the repo so they can be fully restyled.
- **LayerChart** for the burn-up chart, forecast band and time breakdowns.
- The same Svelte components are reused for the desktop app card (§10.5), built as a separate single-file bundle.
- Talks to the binary through a JSON API. Live updates come from polling the newest event id every 500 ms (cheap and needs no extra machinery).
- Light and dark mode via `prefers-color-scheme`, dark by default (§10.4).

## 5. Data model

### 5.1 Event envelope

Every state change is one row in `events`:

| Field | Meaning |
|---|---|
| `id` | ULID (sorts by time) |
| `v` | Schema version of `data` |
| `ts` | UTC timestamp |
| `repo_id` | Repo identity (§6.1) |
| `session_id` | Claude Code `session_id`, or `owner` for dashboard and CLI actions |
| `seq` | Increases by one per session |
| `kind` | Event kind (§5.2) |
| `actor` | `{agent_id?, agent_type?, model?}`: which agent or subagent |
| `task_id` | Plan node id, when applicable |
| `data` | JSON payload for the kind |

**Rules**
- Events describe intent, not resulting state.
- Events are never updated or deleted; SQLite triggers enforce this. A mistake is fixed by a later correcting event.
- Readers ignore unknown fields and fill in defaults for missing ones. Old `v` versions are converted on read.

### 5.2 Event kinds

- **Plan:** `plan_imported`, `node_added`, `node_edited`, `node_moved`, `node_removed`, `replan` (groups a batch of edits under a reason), `dep_added`, `dep_removed`, `intent_set`, `estimate_set`, `hill_moved`
- **Session:** `session_started` (source), `task_claimed` (auto or confirmed, with confidence), `prompt`, `tool_intent`, `unit_stopped` (files, summary, drift score, label), `idle`, `needs_owner`, `session_ended`
- **Tangents:** `detour_opened` (reason, blocks, resume_cue), `label_set`, `drift_flagged`, `item_suspended`, `item_resumed`, `detour_closed`, `detour_promoted`
- **Handoff:** `checkpoint` (sha, next-action cue), `handoff_note`, `readback` (pass or fail, diff), `leftover_found`, `leftover_resolved` (adopt, resume or undo, plus stash/snapshot ref)
- **Verification:** `claimed_done`, `verified` (level, evidence, by), `verification_failed`, `verification_stale`
- **Decisions:** `decision` (what, why, alternatives, `supersedes?`)
- **Dependencies:** `waiting_on`, `wait_cleared`
- **Telemetry:** `usage` (tokens in/out/cache-read/cache-write, model, cost)
- **Owner:** `owner_action` (button or CLI action plus target), `config_changed`, `report_published`
- **Git and archive:** `squashed` (old SHAs, new SHA), `archived`, `restored`

### 5.3 Plan tree (view)

| Field | Notes |
|---|---|
| `id` | Stable dotted id (`2.3`). Never reused after removal. |
| `kind` | goal, milestone, task or subtask. Small projects may go straight from goal to task. |
| `title` | |
| `intent` | 2–3 sentences of purpose and end state. **Required** on the goal and milestones, optional below. |
| `estimate_min` | Agent-minutes. Set at import, at the first claim, or by `plan_edit`. |
| `status` | todo, active, claimed (L0), L1–L4, failed or dropped |
| `hill` | Milestones only: uphill or downhill, set by the agent with `hill_moved` |
| `depends_on[]` | Node ids |
| `expected_files[]` | From the plan's Files lists or learned from history. Feeds drift scoring and L1 checks. |
| `branch` | Branch where the node's work happens |
| `source` | `{superpowers file, task number}` or `manual` |

### 5.4 Session (view)

Active task; suspended items (each with `resume_cue`); state (working, idle, needs-owner, ended-clean, ended-dirty or crashed); worktree path and branch; active seconds; tokens and cost; last checkpoint SHA; read-back status.

### 5.5 Unit of work and active time

- A **unit** runs from `prompt` to `unit_stopped`.
- **Active time** is the unit's wall-clock time minus time spent waiting on permission prompts or the owner (from `Notification` events inside the unit). Subagent time inside the unit is included once, not added on top.
- A unit's time and usage go to the active task **and**, if a detour is open, also to that detour. Forecasting uses the task total, which includes detours (as in EBS). The drift breakdown reports detour and drift time separately.

### 5.6 Progress

- `done% = Σ estimate_min(live leaves at ≥ L2) / Σ estimate_min(all live leaves)`
- `claimed%`: leaves at L0–L1, drawn separately and never counted in done%.
- **Scope line:** total live estimate over time, burn-up style, so a drop in done% caused by added scope is visible as such.
- A milestone that is **uphill** shows its % de-emphasised with the uphill pill. Hill state is the main signal until the milestone goes downhill.
- **Stuck flag:** a milestone whose hill state and done% haven't changed in 3 sessions that worked on it.

## 6. Storage

### 6.1 Layout

```
~/.bridge-screen/
  config.toml          # global settings: points conversion, thresholds, prices, UI port
  ui.lock              # dashboard single-instance guard (pid, port, token)
  bridge-screen.db     # SQLite, WAL mode, all repos
  snapshots/<repo-id>/ # records of pre-undo stashes and rescue branches
  lead/<repo-id>/      # project pages: intent, project-map, current-state, decision-log, glossary (.md), plus .offered / .declined markers (outside the database; `rebuild` never touches it)
  archive/<repo-id>/   # compressed monthly archive files (§6.5)
```

- `repo_id` = hash of the absolute path from `git rev-parse --git-common-dir`. All worktrees of a repo share it.
- Per-repo settings live in the `repos` table: name, root, test command, drift-pause opt-in, crash timeout.

### 6.2 Tables

- `events`: append-only (§5.1).
- View tables: `plan_nodes`, `plan_deps`, `sessions`, `suspended_items`, `units`, `decisions`, `verifications`, `usage`, `leftovers`, `waits`, `calibration`, `transcript_cursors`, `drift_samples`, `rollups`, `archive_index` (plus its FTS5 table). Each one is updated **in the same transaction** as the event that changes it.
- `repos`, `meta` (schema version, migrations).
- `bridge-screen rebuild` throws the view tables away and replays `events`.

### 6.3 Concurrency

- WAL mode, `busy_timeout` of 2000 ms, short `BEGIN IMMEDIATE` write transactions.
- Each hook process opens the database, writes one transaction, and exits. The dashboard and MCP reads never block writers.
- A hook killed mid-write leaves nothing behind; the transaction rolls back.
- If the database stays locked past the timeout, the hook logs to `~/.bridge-screen/hook-errors.log` and exits 0, so it never breaks the session.

### 6.4 Privacy

- Fully local. The binary makes no network calls, with two exceptions the owner starts: the plugin's first-run binary download (§16.2) and publishing a progress report Artifact on explicit request (§10.6).
- The dashboard binds to `127.0.0.1` only and requires a random token, which `bridge-screen ui` puts in the URL it prints.
- Nothing is written inside any repo except the approved WIP commits.

### 6.5 Archive and compaction

Two rules decide what moves to the archive and is compressed:

- **Inactive projects:** a repo with no events for **15 days** is archived whole. All its events and view rows move to the archive. The main database keeps only a stub row (name, final done%, last activity, archive location). The project leaves the glance screen and appears in a collapsed "Archived" list.
- **Active projects, size-triggered:** when a project's high-volume detail (`prompt`, `tool_intent`, `unit_stopped`, per-unit `usage`) takes up more than **16 MB** in the main database, all of that detail older than **2 days** is compacted. The daily archive run measures the size per repo. Detail from the last 2 days is never compacted, because recovery and the current briefing use it. The compacted detail moves to the archive and is replaced in the main database by per-task and per-session **rollups** (time, tokens, cost, label split). Small, long-lived facts stay in the main database: plan events, decisions, verifications, checkpoints, squashes, leftover resolutions and config changes. Views are rebuilt from rollup snapshots plus the remaining events.

**Archive format**
- One SQLite file per repo per month, `~/.bridge-screen/archive/<repo-id>/<YYYY-MM>.db.zst` (zstd-compressed). It holds the moved events verbatim, the moved view rows, and a `manifest` (repo, date range, row counts, checksum, schema version).
- The main database keeps an `archive_index` table and an **FTS5 full-text index** over the archived searchable text: task titles and intents, decisions, handoff notes, unit summaries, file paths, branch names. Search never needs to decompress anything; only "show details" and "restore" do.

**Safety**
- Archiving writes the file, flushes it to disk, re-opens it and checks row counts and the checksum, and only then deletes the rows from the main database, in one transaction. If any step fails, nothing is deleted.
- Archive files are never modified after they're written. A later compaction for the same month writes a new part file (`<YYYY-MM>.2.db.zst`).

**When it runs**
- No background process. At most once a day, the first `SessionStart` or `bridge-screen ui` of the day starts `bridge-screen archive run` as a detached, low-priority job guarded by a lock file. It never delays the hook.
- Starting a new session in an archived project **restores it automatically** before the briefing, so recovery and progress work as if it had never left.

**Retrieval tooling**

| Command | What it does |
|---|---|
| `bridge-screen archive list [--repo X]` | Archived projects and months, with date ranges and sizes |
| `bridge-screen archive search "<query>" [--repo X] [--kind decision\|task\|note\|file] [--since D] [--until D]` | Full-text search over everything archived. Results show repo, date, kind, a snippet and an id. |
| `bridge-screen archive show <id>` | The full archived item with its surrounding events (decompresses only that month) |
| `bridge-screen archive restore <repo> [--month YYYY-MM]` | Moves a project, or one month of it, back into the main database |
| `bridge-screen archive verify` | Checks every archive file against its manifest |
| `bridge-screen archive help` | Prints the retrieval guide below |

- **MCP tool** `archive_search(query, filters)` lets an agent look up old decisions or how something was done before.
- **Dashboard:** an Archive page with the same search box, filters, and a Restore button per project.

**Retrieval guide** (shipped as `bridge-screen archive help` and in the README):
1. Not sure where something is? Run `bridge-screen archive search "<words>"`. It searches all repos and dates.
2. Open a result with `bridge-screen archive show <id>`.
3. Want the whole project back on the dashboard? `bridge-screen archive restore <repo>`, or just start a Claude session in that repo.
4. Archive files are ordinary SQLite once decompressed (`zstd -d <file>`), so they can also be opened with any SQLite tool.

## 7. Tangent detection and the Navigator

### 7.1 Labels

Every unit gets exactly one label: **on-task**, **detour** (not the task, but it blocks the task), **requested** ("asked for": not the task, but the owner asked for it in this session) or **drift** (none of these). Tangents are never blocked and never handed to another agent.

*Requested* (polish round, 2026-10-21): the agent declares it — `start_detour` with `requested_by_owner: true` labels the detour's units `requested` at Stop, and `label_unit`/`bridge-screen label` accept `requested` — or the owner relabels. It is never detected from prompt text (prompts are never stored). A requested unit adds no away time, never turns a session amber or red, raises no drift question or needs-you item, is never a tuning sample, and shows as "Asked for" on the screens.

### 7.2 Two layers

1. **Self-declared.** If `start_detour` was called during the unit, the unit is a detour, linked to the task it blocks.
2. **Cross-check at Stop.** A Rust heuristic computes a drift score from 0 to 1:
   - **File affinity:** overlap between the files touched and the task's `expected_files` plus its history and its milestone's history. Skipped on a task's first unit if there's no history.
   - **Text affinity:** keyword overlap between `last_assistant_message` and the titles and intents of the task, its milestone and the goal (two levels, per commander's intent).
   - **Area:** files in parts of the repo the plan never mentions (container, CI, unrelated packages).

| Score | Action |
|---|---|
| < 0.3 | Labelled on-task. No note. |
| 0.3–0.7 | The Navigator asks the agent to label the unit (`label_unit`). The answer is used. |
| > 0.7 with no declared detour | `drift_flagged` as *possible drift*. The agent is asked to label it. If the agent says on-task, the disagreement is recorded and a ⚠ is shown. |

No LLM is called from hooks. Judgement comes from the agent itself, through `label_unit`.

*Units cut short* (2026-10-21): a new user message mid-turn sends no Stop, so the next prompt closes the open unit. That prompt scores the unit like a Stop does but **from its files alone** (there is no last message; missing text never counts as drift — with no task files to compare with, the area alone scores at most 0.4, a question rather than drift). Above the upper cut-off it is `drift_flagged` and labelled drift at once (the agent or owner can relabel); below the lower one, on-task; in between, the **next Stop** asks the agent about that unit ("unit N was cut short… label it") when the stopped unit itself needs no question — the same limits apply: never blocking, one note per unit, never when `stop_hook_active`. Unasked, the heuristic labels it on-task at the following prompt.

### 7.3 Tolerance band

- **Away time** is consecutive unit time that is not on-task in the session.
- **Inside tolerance** (green): away time < max(15 min, 20% of the active task's estimate).
- **Outside tolerance** (amber): one note, e.g. *"You've been on '<reason>' for 40 min (~2× the task estimate). Continue, or should the owner know?"*
- **Far outside** (> 2× tolerance), or drift with no declared reason (red): a needs-you item with **Continue / Make it a task / Stop**. If the repo has the drift pause turned on, the agent finishes its step, commits, and waits.
- A detour past tolerance is offered **promotion** to a real task (`detour_promoted` + `node_added`, recorded as scope added mid-stream).
- The thresholds start from the values above in `config.toml` and are then **auto-tuned** (§7.6).

### 7.6 Auto-tuning the drift thresholds

- **Training signal:** every time the owner overrides a label (on the dashboard or in chat), and every time the agent's `label_unit` answer disagrees with the heuristic, the unit's drift score and the final label are stored in a `drift_samples` table.
- **When it runs:** after every 20 new samples for a repo, at the next SessionStart.
- **What it does:** it moves the 0.3 and 0.7 cut-offs to the values that best separate on-task from detour/drift in that repo's last 200 samples, and adjusts the tolerance percentage the same way from owner Continue/Stop answers.
- **Guard rails:** each run moves a cut-off by at most 0.05. Cut-offs stay within 0.15–0.5 (lower) and 0.5–0.9 (upper), and the lower always stays below the upper. Tuning is per repo.
- **Visibility:** every change is a `config_changed` event with old value, new value and sample count, shown in the calibration drill-down with a one-click revert. `bridge-screen config reset-drift` restores the defaults.

### 7.4 Suspended items and resume cues

- Opening a detour **suspends** the current item and requires a `resume_cue`: one line naming the next action and the file or test it concerns.
- Closing a detour injects: *"Return to 2.3 Login form. On return: <cue, verbatim>."*
- Several items can be suspended at once, and any of them can be resumed with `resume(item_id)`. Items still suspended at session end appear in the next briefing.
- A resume cue is also written at every checkpoint and at task suspension. A crash gives no warning, so the cue is kept up to date continuously.

### 7.5 Navigator rules

- **At most one note per unit.** Priority order: checkpoint required, then return to task, then read-back missing, then label request, then tolerance warning, then dependency ready, then verification nudge.
- The same kind of note is not repeated within 3 units unless its state has changed.
- Notes are 1–3 lines and end with the single action expected.
- Every red state on the dashboard has a defined trigger and action buttons. There are no signals that only say "something's wrong".

## 8. Handoff, recovery and verification

### 8.1 Checkpoints

- A checkpoint is required at every subtask or task boundary (`claimed_done`) and when a detour closes.
- If the worktree has uncommitted changes at that point, the **Stop hook blocks stopping** with: *commit as `wip(bridge): <id> <title>`* plus the trailers `Bridge-Task`, `Bridge-Next`, `Bridge-Session`. This is the only place bridge-screen blocks.
- If a pre-commit hook rejects the commit, the agent fixes the problem and commits again.
- A `checkpoint` event records the SHA.
- `bridge-screen squash [--onto <base>]` collapses the `wip(bridge):` commits on a branch into one, with a generated summary message.
- **Automatic squash when a PR is opened.** A `PreToolUse` hook on Bash matches `gh pr create` (and the GitHub MCP create-pull-request tool, if present). Before the command runs it:
  1. squashes the branch's `wip(bridge):` commits onto the merge base with the PR's base branch
  2. if the branch was already pushed, pushes it with `--force-with-lease`, so the PR shows the squashed history
  3. records a `squashed` event (old SHAs and new SHA), then lets the command proceed

  If the squash fails (conflict, or someone else pushed to the branch), the hook leaves the history untouched, lets the PR open anyway, and puts a needs-you item up saying the WIP commits weren't squashed. PRs opened outside Claude Code (for example on github.com) can't be detected; for those, `bridge-screen squash` stays available by hand.
- The old SHAs are kept in the `squashed` event, and verifications tied to them are re-pointed to the new SHA.

### 8.2 How a session ended

- **Clean:** `session_ended`, and no uncommitted changes or claimed-but-unfinished task.
- **Dirty:** `session_ended`, with uncommitted changes or a claimed task left unfinished.
- **Crashed:** no `session_ended` and no events for 30 minutes (configurable). Idle notifications don't count as activity.

### 8.3 Recovery at SessionStart (any worktree of the repo)

1. **Analysis.** Find dirty or crashed sessions whose leftovers haven't been resolved. Look in this worktree first, then the others.
2. **Evidence per dead session:** active task, last resume cue, last checkpoint SHA, `git status` and `git diff <checkpoint>` in its worktree, unverified claims, open detours.
3. **Attribute each uncommitted file** to a task using `tool_intent` and `unit_stopped` records. Each becomes a `leftover_found`.
4. **Suggest an action for each:**
   - **resume:** the task is still active and the files match it, so continue from the cue
   - **adopt:** the work looks complete; verify it, then commit
   - **undo:** the work was abandoned or drift, or it conflicts with later work; snapshot it, then revert
5. **The agent acts when confident and asks when not.** Undo is always `git stash push -- <files>` plus a snapshot record, then the revert. Nothing is ever discarded. Leftovers belonging to a **live** session are only reported, never touched.
6. Each resolution is logged as `leftover_resolved`, so no later session triages it again. A choice the owner makes on the dashboard overrides the agent's.

### 8.4 Briefing and read-back (I-PASS)

- **Briefing** (SessionStart `additionalContext`, ≤ 40 lines):
  - **Status:** stable, watcher or unstable. Unstable = leftovers or failed verification. Watcher = open detours or unverified items. Stable = neither.
  - **Summary:** goal and milestone intent, plus where the active or candidate task stands.
  - **Action list:** unverified items to check first, leftovers, the next action.
  - **If/then warnings**, from the previous session's `handoff_note`.
  - **Dependencies:** ready or still waiting.
- **Read-back:** before its first edit the agent calls `handoff_readback(goal, in_flight, next_action)`. bridge-screen compares it with the log (same task ids, next action matching the cue). On a mismatch it sends one correction note. `PreToolUse` nudges once if an edit starts without a read-back, but **does not block**.
- **Stable sessions with nothing pending** get a 3-line briefing and no read-back.
- **After compaction** (`SessionStart` with source `compact`): a short re-briefing with the active task, its resume cue and this session's decisions.
- **Project pages note:** one last line, if there is room under the 40 lines. Pages set up: a pointer ("read intent and current state first; keep current state true; last updated <date>"). Not set up and the owner hasn't said no: a two-line offer asking the agent to ask the owner once, in plain words, whether to set them up (at most three sessions, counted in `.offered`; dropped first when the briefing is full; left out beside the "no plan yet" or plan-import notes; the re-briefing after compaction gets the pointer only). The pages and the offer are described in `2026-10-22-project-pages-and-lead-bundle-design.md`.
- **Handoff note:** requested whenever a task reaches done or is suspended. It holds what's in flight and the if/then warnings.

### 8.5 Verification ladder

| Level | Meaning | How it's reached |
|---|---|---|
| L0 | Claimed | `claim_done(id, evidence)` |
| L1 | Code exists | bridge-screen checks the evidence paths and symbols at HEAD. Automatic. |
| L2 | Unit tests pass | The agent runs the repo's test command and reports a summary and exit code. bridge-screen re-runs it if it finishes within a configured timeout (default 120 s). |
| L3 | End-to-end | The agent reports what it drove (browser or CLI) and the outcome. Recorded as reported. |
| L4 | Owner | ✓ on the dashboard, `bridge-screen verify`, or telling any session in chat |

- **Every verification is tied to a commit SHA.** If later commits touch files it depended on, it drops one level (`verification_stale`) and is re-checked at the next session start.
- **Unverified items** (L0–L1 older than one session) lead the next briefing. After two ignored nudges they appear in the needs-you queue.
- `verification_failed` sets the task back to active and makes the session status unstable.

- **Taking back a sign-off** (polish round): the owner's newest verdict still in force — a sign-off, or a "broken" while the task is still failed — can be withdrawn (`bridge-screen verify <task> --withdraw`, or "Take back sign-off" on the task drill-down; owner only, the agent's tools can't). `verification_withdrawn` marks the sign-off `withdrawn`, or restores what the "broken" wiped; the status is recomputed: the highest remaining level, else claimed, else active (a live session holds it) or to-do. A second withdraw is refused as stale. The forecast's history is not rewritten.

### 8.6 Decisions

- **When they're captured:** at task completion (Stop note), before compaction (`PreCompact` sets a flag, and the next note asks), and at clean session end (with the handoff note).
- `supersedes` links a changed decision to the one it replaces.
- Duplicates in the same task are merged when the views are built.
- Decisions are hidden by default and shown in drill-downs.

## 9. Cross-session awareness, telemetry, forecasting

### 9.1 Dependencies

- Plan links (`depends_on`) are inherited by whichever session claims the dependent task. Ad-hoc `waiting_on` events cover surprises and are offered for promotion to plan links.
- When a dependency reaches ≥ L2, every waiting session gets: *"2.4 done on `feat/auth` @abc123. Merge or rebase to pick it up."*

### 9.2 Telemetry

- At Stop and SubagentStop the hook reads **only the new lines** of `transcript_path` (it keeps a cursor per transcript) and sums tokens per model.
- Cost comes from a price table in `config.toml` (shipped defaults, editable).
- Usage is attributed to the active task, any open detour, and the actor (`agent_id`, `agent_type`).
- **Retries** = units on a task after a `verification_failed`. **Steps** = the subtask count, with the current one marked.

### 9.3 Forecast

- **Calibration ratio** per completed leaf = actual active minutes ÷ estimate, kept per model. History older than 6 months is ignored. With fewer than 6 data points, a wide default distribution is used.
- **Monte Carlo:** 1000 runs. Each run divides every remaining leaf's estimate by a randomly drawn past ratio and sums the result. The output is P50 and P90 **agent-active hours**, which become dates using the project's recent active hours per day.
- The forecast is re-run after each verified leaf. Its history is kept, so convergence or a slipping date is visible.

## 10. Dashboard and compact views

### 10.1 Glance screen (the first screen; nothing else is shown until clicked)

- **Header:** ✱ mark, session and project counts, a red "N need you" pill (or a green "All clear").
- **Escalation at more than 4 items:** when the needs-you queue holds more than 4 items, the count pill pulses briefly each time the count rises past 4 (at most 3 cycles, under 5 seconds; a single fade under reduced motion), then stays emphasised and still until the count drops back to 4 or below. The queue shows the 4 most urgent items with a "show all N" link, and the compact `/bridge-screen` view puts the count on its first line. (Owner decision 2026-10-14: no continuous motion.)
- **Needs-you queue**, outlined in red — only items where a session is blocked on the owner: sessions that asked the owner a question or wait on a permission prompt, red sessions with Continue (optional reason) / Make task / Stop, items awaiting owner verification (✓ Works / ✗ Broken with a note), and recovery questions (Adopt / Resume / Undo). A stopped session whose task was claimed done with nothing pending shows as **done**; any other stopped session shows as **idle**; neither is a needs-you item. Each click appends an `owner_action`, and the affected session picks it up at its next hook.
- **One row per project:** done% bar in clay, a lighter clay segment for claimed%, hill pill (amber uphill, green downhill), P50 and P90 dates.
- **One line per live session**, with a status light (green on task, amber detour or waiting, red needs you), the active task or detour, and the wait target.
- **Branch groups:** same-named branches across repos, with a combined %.
- **Parents show a roll-up** (polish round): a milestone and the goal read *done* when every live task under them passed its tests (L2+, the done% test), *needs attention* when one failed, *in progress* once any work started, else *to do*. Derived when read, never stored; adding a task to a finished milestone makes it *in progress* again. A done milestone shows no hill word.

### 10.2 Drill-downs (click to expand)

- **Project:** plan tree with intents, estimate vs actual, verification level and stale badges, dependencies, the scope line, and re-plan history.
- **Session:** a timeline of labelled units, the time split (on-task, each detour, drift), suspended items, checkpoints, leftovers and their resolutions, the handoff note and read-back result.
- **Task:** decisions with their superseded-by chains, verification evidence, tokens and cost, actors, retries.
- **History:** the raw event stream with filters. Hidden by default.
- **Calibration:** ratios per model and the forecast's convergence over time.
- **Project pages:** the repo's five project pages as plain text with their last-updated dates (read-only; `GET /api/pages/{repo}`); a sentence and the command to run when they aren't set up or were declined.

### 10.3 Terminal screen

- `bridge-screen status` and the `/bridge-screen` command in the terminal CLI print about 6 lines of the glance screen with ANSI colors, plus the dashboard URL if it's running.
- `bridge-screen watch` is a live full-terminal view built with **ratatui**: the glance layout, refreshed every second, with keyboard navigation (arrow keys, Enter to expand a session or project, `a` to act on the selected needs-you item, `q` to quit). Owner actions taken here are recorded like dashboard clicks.

### 10.4 Visual theme (Claude Code palette)

- **Clay `#D97757`** is reserved for progress (done bar; claimed segment at about 35% opacity), owner-check items, and the ✱ mark.
- **Status colors:** green = on task, amber = detour, waiting or uphill, red = needs you.
- Neutral grays everywhere else. Session ids, branches and file names are in monospace.
- **Dark mode by default**, matching Claude Code's dark terminal. Light mode follows the system setting.
- A new red item pulses **once**, then stays still. The tab title shows the count (`(3) bridge-screen`) and the favicon turns red while anything needs the owner.
- The terminal screen maps the same roles to ANSI colors (clay to the closest 256-color orange).

### 10.5 Claude desktop app screen

- The MCP server provides a `show_progress(scope?)` tool built on **MCP Apps**: the tool declares a `ui://bridge-screen/progress` resource (an HTML page, MIME type `text/html;profile=mcp-app`), which the app renders inline in the chat as an interactive card.
- **The card shows:** the needs-you queue with its buttons, project progress bars, and session status lights, in the §10.4 theme. Button clicks go back to the MCP server as tool calls and are recorded as `owner_action` events.
- `/bridge-screen` in the desktop app asks Claude to call `show_progress`.
- **Fallback:** where the host doesn't support MCP Apps (the terminal CLI, or the desktop Code tab if it turns out not to), the tool returns the colored text view from §10.3 plus the dashboard URL.
- **To verify first:** MCP Apps support is confirmed for Claude's desktop chat; support inside the desktop app's Code tab must be tested in a spike at the start of sub-project 6.

### 10.6 Published progress reports (opt-in)

- Only when the owner explicitly asks ("publish a progress report"), the agent calls `progress_report(scope)`. It writes a self-contained HTML snapshot (glance view, progress, forecast, recent decisions) to a temporary file and returns its path.
- Claude then shows the owner a summary of exactly what the page contains and publishes it with its Artifact tool only after the owner confirms. Re-publishing updates the same link.
- **Per-repo switch:** `publish_reports = never` (for office repos) makes the tool refuse.
- Each publication is recorded as a `report_published` event (scope, time, artifact link). Nothing is ever published automatically.

## 11. Agent interface

### 11.1 Hooks

| Hook | Records | Navigator output |
|---|---|---|
| SessionStart (startup, resume, clear, fork) | `session_started`; recovery analysis; superpowers import check | Briefing (§8.4) |
| SessionStart (compact) | `session_started` | Short re-briefing |
| UserPromptSubmit | `prompt` | First prompt: claim instruction with the top 3 candidates. Later: owner actions, dependency changes |
| PreToolUse (Edit, Write, MultiEdit, Bash) | `tool_intent` (one per file per unit) | Read-back nudge, once |
| PreToolUse (Bash `gh pr create`, GitHub MCP create-PR) | `squashed` | Automatic squash before the PR opens (§8.1) |
| PostToolUse | files touched | none |
| Stop | `unit_stopped`, `usage`, drift score | One note per §7.5; checkpoint block per §8.1 |
| SubagentStop | subagent usage and work | none |
| PreCompact | sets the decision-capture flag | none (the request goes out with the next note) |
| Notification (idle_prompt, permission_prompt) | `needs_owner` | none |
| SessionEnd | `session_ended` | none |

Hooks must finish in **≤ 50 ms** for the common case. Git and transcript work is incremental, and recovery analysis runs only at SessionStart.

### 11.2 MCP tools

| Tool | Purpose |
|---|---|
| `claim_task(id \| description, confidence)` | Claim the active task, or propose a new node |
| `plan_edit(ops[], reason, replan?)` | Add, edit, move or drop nodes; set estimates, intents, dependencies and hill state |
| `start_detour(reason, blocks, resume_cue)` | Open a detour and suspend the current item |
| `resume(item_id)` | Resume any suspended item |
| `close_detour(outcome)` | Close the detour and trigger the return cue |
| `label_unit(label, why)` | Answer a label request |
| `log_decision(what, why, alternatives, supersedes?)` | Record a decision |
| `claim_done(id, evidence)` / `verify(id, level, evidence)` | Verification ladder |
| `handoff_readback(goal, in_flight, next_action)` | Read-back, checked against the log |
| `handoff_note(in_flight, if_then[])` | Note for the next session |
| `resolve_leftover(id, action)` | adopt, resume or undo (undo is always a snapshot) |
| `waiting_on(target, reason)` | Ad-hoc dependency |
| `lead_pages(action, owner_said?)` | Project pages: `set_up` / `decline` (both need the owner's words) or `status` |
| `lead_page_read(page)` | Read one project page |
| `lead_page_write(page, content, mode?, reason, owner_said?)` | Write a page: the intent needs the owner's words, the decision log is append-only, at most 64 KiB |
| `status()` | Compact text state for the agent |
| `archive_search(query, filters)` | Search archived projects and history (§6.5) |
| `show_progress(scope?)` | Inline progress card in the desktop app, or text elsewhere (§10.5) |
| `progress_report(scope)` | Opt-in HTML report for publishing as an Artifact, only on the owner's request (§10.6) |

### 11.3 Skill

`skills/bridge-screen/SKILL.md` teaches the protocol:
- claim the task, or ask if unsure
- write intents for goals and milestones
- open a detour with a resume cue before tangent work
- read back the handoff before editing
- checkpoint at boundaries
- log decisions
- climb the verification ladder honestly

It stays under about 150 lines. It also says when to use the bundled `lead-briefing` skill (any status, problem or checklist for the owner) and how to keep the project pages current. That skill ships beside it in `skills/lead-briefing/` (with `references/why-it-works.md` and `references/living-docs.md`), copied unchanged.

## 12. Plan sources

### 12.1 Superpowers import

- Source files: `docs/superpowers/plans/YYYY-MM-DD-<feature>.md` (format as of superpowers 6.4.1). Specs in `docs/superpowers/specs/` are linked as references.
- **Mapping:**
  - plan → milestone (or the goal for the repo's first plan)
  - `**Goal:**` → intent
  - `### Task N: <name>` → task
  - the **Files:** list → `expected_files`
  - `- [ ] **Step k: …**` → subtask
  - a checked step → `claimed_done` (never "verified")
- **Triggers:** `bridge-screen import`, or automatically at SessionStart when a plan file is new or its content hash has changed.
- **Changes:** new vs. previous import, matched by task number and title, recorded as `plan_edit` events. If more than 30% of the tasks changed, it's recorded as a `replan`. The file is never written to.
- Missing estimates are filled in by the agent at its first claim.

### 12.2 Existing half-built repos

- `bridge-screen init` runs inside a normal Claude session, guided by the skill. The agent scans the README, docs, existing plans and specs, the git log, TODO/FIXME comments, and test results. It drafts a goal (with intent), milestones and tasks, with statuses inferred from the code, and shows the owner the draft once to confirm or edit. The owner can seed it up front ("the goal is X").
- Tasks inferred as done start at **L1** and climb the ladder over the following sessions.

## 13. Build order (sub-projects)

All features ship. Each sub-project gets its own implementation plan, built in dependency order:

1. **Core store and plan.** SQLite schema, event append with views, rebuild, plan tree, `plan_edit`, CLI skeleton, `bridge-screen install`, and the release pipeline (§16) so every later sub-project ships installable builds.
2. **Hooks and sessions.** All hook handlers, units, active time, claiming, the MCP server skeleton with claim/plan tools, the skill.
3. **Navigator and tangents.** Drift score, labels, tolerance band, suspended items and resume cues, note rules, drift auto-tuning.
4. **Handoff and recovery.** Checkpoints and the Stop block, end classification, leftover analysis and resolution, briefing, read-back, `bridge-screen squash` and the automatic squash on PR creation.
5. **Verification and decisions.** Ladder, staleness, decision capture, and the dependency-ready note for plan links. *Deferred from this run (owner, 2026-10-14):* ad-hoc `waiting_on` / `wait_cleared` dependencies (§9.1, §11.2) — the tool, `waits` view, clearing at ≥ L2 with the ready note, briefing/status display and promotion to plan links. Pick it up in sub-project 8 (or earlier); see the SP5 plan's "Not in this sub-project" section.
6. **Screens.** Starts with a spike testing MCP Apps in the desktop app's Code tab. Then the axum API, Svelte dashboard (glance, needs-you actions, drill-downs, theme), the ratatui `watch` view, the desktop `show_progress` card, and the opt-in `progress_report`.
7. **Telemetry and forecast.** Transcript parsing, cost, calibration, Monte Carlo.
8. **Imports and onboarding.** Superpowers import and diff, `bridge-screen init` for existing repos, branch groups and cross-repo dependencies, and the ad-hoc `waiting_on` dependencies deferred from sub-project 5.
9. **Archive.** Compaction and rollups, archive files, full-text index, `archive` commands, `archive_search` tool, Archive page, auto-restore.

Sub-projects 1–4 make bridge-screen useful on its own. The glance view from sub-project 6 can start early as a read-only page.

## 14. Testing strategy

- **Core modules:** unit tests against an in-memory SQLite, plus property tests checking that rebuilding views from events matches the incrementally maintained views.
- **Hooks:** golden tests that feed recorded hook JSON payloads to `bridge-screen hook` and compare the events and stdout produced.
- **Recovery:** temporary git repos and worktrees with scripted crash scenarios (uncommitted edits, claimed-but-unfinished tasks, a live and a dead session side by side).
- **Concurrency:** N parallel processes appending to the database must lose no events.
- **MCP:** contract tests for each tool.
- **Dashboard:** API tests in Rust, plus component tests and one end-to-end smoke test (Playwright) for the glance screen and a needs-you action.
- **Archive:** archive then search then restore round-trips with no lost rows; a failure injected at each archiving step leaves the main database unchanged; views rebuilt from rollups plus remaining events match the views before compaction.
- **Auto-squash:** a scripted `gh pr create` in a temporary repo with a fake remote; squash success, conflict fallback, and already-pushed branches.
- **Auto-tuning:** cut-offs move toward the separating value, never by more than 0.05 per run, and stay inside their bounds.
- **Performance:** a hook latency check (≤ 50 ms p95 on a database with 100k events).

## 15. Resolved questions

All open questions from the first draft were answered by the owner on 2026-10-13:

1. **Crash timeout:** 30 minutes of silence (§8.2).
2. **Squash timing:** automatic when a PR is opened (§8.1).
3. **Needs-you escalation:** more than 4 items (§10.1).
4. **Threshold tuning:** auto-tune the drift thresholds (§7.6).
5. **Retention:** inactive projects archived after 15 days; size-triggered compaction (over 16 MB, older than 2 days) in active projects; retrieval directions and search tooling (§6.5).
6. **Open-sourcing:** nothing chosen with that in mind. Name: `bridge-screen`.

**Retention rules, confirmed:** projects inactive for 15 days are archived whole. In active projects, high-volume detail is compacted once it exceeds 16 MB per project, and only the part older than 2 days. Plan, decision and verification history stays in the main database.

## 16. Distribution and install

### 16.1 Channels

| Channel | User's command | Notes |
|---|---|---|
| Claude Code plugin marketplace (main) | `/plugin marketplace add bedantsharma/bridge-screen` then `/plugin install bridge-screen` | Brings the hooks, MCP server, skill and commands |
| Homebrew tap | `brew install bedantsharma/tap/bridge-screen` | Own tap repo (`homebrew-tap`). homebrew-core later, once eligible |
| PyPI | `pipx install bridge-screen` or `uv tool install bridge-screen` | Rust binary packaged in platform wheels with `maturin` (`bindings = "bin"`) |
| npm | `npx bridge-screen` | Per-platform packages |
| Shell installer | `curl -LsSf <release-url>/install.sh \| sh` | From GitHub Releases |
| crates.io | `cargo install bridge-screen` | Builds from source |

The name `bridge-screen` was unclaimed on PyPI, crates.io, npm and Homebrew on 2026-10-13.

### 16.2 How the binary and the plugin arrive together

- **Plugin first.** The plugin ships a small launcher script (outside a top-level `bin/` directory). On first use it downloads the prebuilt binary for the machine's OS and CPU from the matching GitHub Release into `${CLAUDE_PLUGIN_DATA}`, checks it against the published SHA-256, and runs it. Hooks and the MCP server call the launcher. Plugin updates bring the matching binary version.
- **Package manager first.** `brew`, `pipx`, `npx` or the shell installer put the binary on the PATH; then `bridge-screen install` adds the marketplace and enables the plugin through Claude Code's plugin CLI. If a PATH binary of the same version exists, the launcher uses it instead of downloading.
- **Windows:** the program is released for Windows; the launcher is not supported there (it says so, and exits quietly from a hook). A `bridge-screen` on the PATH is still used.
- The terminal command (`bridge-screen ui`, `watch`, `status`) needs the binary on the shell PATH, which the package-manager route provides. Plugin-only users start the dashboard with `/bridge-screen ui`.

### 16.3 Release pipeline

- **cargo-dist** builds release binaries for macOS (arm64, x86_64), Linux (x86_64, arm64, musl) and Windows (x86_64) from one git tag. It publishes the GitHub Release with checksums, the shell and PowerShell installers, the Homebrew tap formula and the npm packages.
- A **maturin** job in the same workflow builds and uploads the PyPI wheels.
- A **crates.io** workflow publishes `bridge-screen-core`, then `bridge-screen`, on real release tags (prerelease tags are skipped). The crates carry no dashboard; the program built from them serves a page saying how to build it.
- The Svelte frontend is built before the Rust build so the assets are embedded.
- The plugin manifest's `version` is bumped from the same tag.

