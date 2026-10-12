# Agent Progress Tracker — Design Spec

- **Date:** 2026-10-13
- **Status:** Draft for owner review
- **Source brief:** `agent-tracker-brief.md`
- **Working name:** `tracker`

---

## 1. Purpose and success criteria

The owner builds 2-day to 2-month projects almost entirely through Claude Code, often with 1–N parallel sessions, each in its own git worktree. The tracker answers three questions at a glance, without the owner nagging agents or reading walls of text:

1. **Distance:** how close is each project to its goal?
2. **Direction:** is each session advancing its task, or is it on a tangent?
3. **Leftovers:** what did dead or abandoned sessions leave behind, and has it been dealt with?

**Success criteria**
- The owner no longer maintains a `roadmap.md` by hand.
- The glance screen answers all three questions in under 5 seconds.
- A new session resumes a crashed session's work without the owner re-explaining it.
- No tracker state is ever written inside a repo, apart from the WIP commits the owner approved.

**Scope.** The full feature set from brief §3. Implementation is split into sub-projects (§13), but none are cut.

## 2. Decisions taken during design

| Topic | Decision |
|---|---|
| Steering model | **Navigator.** The tracker records events and puts short notes into the agent's context at key moments. It never blocks work. The one exception is the checkpoint commit (§8.1). |
| Claiming the active task | The agent claims it **automatically when confident** and asks the owner only when unsure where the work stands or where to start. Every claim is logged and visible. |
| Plan source of truth | The **tracker's store**. Superpowers plan files are imported, and later changes to them are recorded as plan-edit or re-plan events. Nothing is ever written back to them. |
| Estimate unit | **Agent-minutes.** Points are a display unit only (default 1 pt = 15 agent-minutes, configurable). Calibration is kept per model. |
| Checkpoints | **Real WIP commits** on the worktree branch, squashed at the end (`tracker squash` or squash-merge). |
| Dashboard | **Can act.** It has a needs-you queue with buttons, and every action is also available in chat and through the CLI. |
| Dependencies | **Both** plan-level task links (inherited by sessions) and ad-hoc `waiting_on` declarations, which the owner can promote to plan links. |
| Grouping | Each repo is a project. Worktrees with the **same branch name in different repos** form one group. No ticket regex and no manual tags. |
| Progress threshold | `done%` counts leaves verified at **L2 (unit tests) or above**. |
| Architecture | A Claude Code plugin (hooks, MCP server, skill) plus a CLI, with no background process. |
| Stack | **Rust** backend in a single binary. **Svelte 5** SPA (Vite), embedded in the binary. **SQLite** (WAL) data layer. |
| Drift pause | **Opt-in per repo** and off by default. When on, confirmed drift makes the agent finish its step, commit, and wait for the owner. |
| Visual theme | Claude Code palette (§10.4). |

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

A single Rust binary, `tracker`, with four entry points over one core library:

| Command | Role | Invoked by |
|---|---|---|
| `tracker hook <event>` | Reads the hook JSON from stdin, records events, runs cheap checks, and prints a Navigator note when one is due | Claude Code, on every hook event |
| `tracker mcp` | MCP server over stdio that provides the agent's tools | Claude Code, once per session |
| `tracker ui` | Local dashboard on `127.0.0.1`. Single-instance: if one is already running it prints the existing URL. | The owner, by hand |
| `tracker <cmd>` | CLI: `install`, `init`, `import`, `status`, `squash`, `verify`, `resolve`, `rebuild`, `config` | The owner, or the agent through Bash |

**Plugin package**
- `hooks/hooks.json`: every hook entry runs `tracker hook <event>`.
- `.mcp.json`: runs `tracker mcp`.
- `skills/tracker/SKILL.md`: the agent protocol (claiming, detours and resume cues, read-back, decisions, verification, checkpoints).
- `commands/tracker.md`: the `/tracker` compact status.

`tracker install` puts the binary on the PATH, registers and enables the plugin, and creates `~/.tracker/`. Nothing is needed per session.

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

- Svelte 5 SPA built with Vite into static assets and embedded with `rust-embed`. Node is needed **at build time only**.
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
- **Owner:** `owner_action` (button or CLI action plus target), `config_changed`

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
~/.tracker/
  config.toml          # global settings: points conversion, thresholds, prices, UI port
  ui.lock              # dashboard single-instance guard (pid, port, token)
  tracker.db           # SQLite, WAL mode, all repos
  snapshots/<repo-id>/ # records of pre-undo stashes and rescue branches
```

- `repo_id` = hash of the absolute path from `git rev-parse --git-common-dir`. All worktrees of a repo share it.
- Per-repo settings live in the `repos` table: name, root, test command, drift-pause opt-in, crash timeout.

### 6.2 Tables

- `events`: append-only (§5.1).
- View tables: `plan_nodes`, `plan_deps`, `sessions`, `suspended_items`, `units`, `decisions`, `verifications`, `usage`, `leftovers`, `waits`, `calibration`, `transcript_cursors`. Each one is updated **in the same transaction** as the event that changes it.
- `repos`, `meta` (schema version, migrations).
- `tracker rebuild` throws the view tables away and replays `events`.

### 6.3 Concurrency

- WAL mode, `busy_timeout` of 2000 ms, short `BEGIN IMMEDIATE` write transactions.
- Each hook process opens the database, writes one transaction, and exits. The dashboard and MCP reads never block writers.
- A hook killed mid-write leaves nothing behind; the transaction rolls back.
- If the database stays locked past the timeout, the hook logs to `~/.tracker/hook-errors.log` and exits 0, so it never breaks the session.

### 6.4 Privacy

- Fully local, with no network calls.
- The dashboard binds to `127.0.0.1` only and requires a random token, which `tracker ui` puts in the URL it prints.
- Nothing is written inside any repo except the approved WIP commits.

## 7. Tangent detection and the Navigator

### 7.1 Labels

Every unit gets exactly one label: **on-task**, **detour** (not the task, but it blocks the task) or **drift** (neither advances nor blocks the task). Tangents are never blocked and never handed to another agent.

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

### 7.3 Tolerance band

- **Away time** is consecutive unit time that is not on-task in the session.
- **Inside tolerance** (green): away time < max(15 min, 20% of the active task's estimate).
- **Outside tolerance** (amber): one note, e.g. *"You've been on '<reason>' for 40 min (~2× the task estimate). Continue, or should the owner know?"*
- **Far outside** (> 2× tolerance), or drift with no declared reason (red): a needs-you item with **Continue / Make it a task / Stop**. If the repo has the drift pause turned on, the agent finishes its step, commits, and waits.
- A detour past tolerance is offered **promotion** to a real task (`detour_promoted` + `node_added`, recorded as scope added mid-stream).
- The thresholds live in `config.toml`. Owner overrides of labels are recorded so the thresholds can be tuned later (§14).

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
- If the worktree has uncommitted changes at that point, the **Stop hook blocks stopping** with: *commit as `wip(tracker): <id> <title>`* plus the trailers `Tracker-Task`, `Tracker-Next`, `Tracker-Session`. This is the only place the tracker blocks.
- If a pre-commit hook rejects the commit, the agent fixes the problem and commits again.
- A `checkpoint` event records the SHA.
- `tracker squash [--onto <base>]` collapses the `wip(tracker):` commits on a branch into one, with a generated summary message. The owner runs it before merging or opening a PR.

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
- **Read-back:** before its first edit the agent calls `handoff_readback(goal, in_flight, next_action)`. The tracker compares it with the log (same task ids, next action matching the cue). On a mismatch it sends one correction note. `PreToolUse` nudges once if an edit starts without a read-back, but **does not block**.
- **Stable sessions with nothing pending** get a 3-line briefing and no read-back.
- **After compaction** (`SessionStart` with source `compact`): a short re-briefing with the active task, its resume cue and this session's decisions.
- **Handoff note:** requested whenever a task reaches done or is suspended. It holds what's in flight and the if/then warnings.

### 8.5 Verification ladder

| Level | Meaning | How it's reached |
|---|---|---|
| L0 | Claimed | `claim_done(id, evidence)` |
| L1 | Code exists | The tracker checks the evidence paths and symbols at HEAD. Automatic. |
| L2 | Unit tests pass | The agent runs the repo's test command and reports a summary and exit code. The tracker re-runs it if it finishes within a configured timeout (default 120 s). |
| L3 | End-to-end | The agent reports what it drove (browser or CLI) and the outcome. Recorded as reported. |
| L4 | Owner | ✓ on the dashboard, `tracker verify`, or telling any session in chat |

- **Every verification is tied to a commit SHA.** If later commits touch files it depended on, it drops one level (`verification_stale`) and is re-checked at the next session start.
- **Unverified items** (L0–L1 older than one session) lead the next briefing. After two ignored nudges they appear in the needs-you queue.
- `verification_failed` sets the task back to active and makes the session status unstable.

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
- **Needs-you queue**, outlined in red: idle or permission-waiting sessions, red sessions with Continue / Make task / Stop, items awaiting owner verification (✓ Works / ✗ Broken with a note), and recovery questions (Adopt / Resume / Undo). Each click appends an `owner_action`, and the affected session picks it up at its next hook.
- **One row per project:** done% bar in clay, a lighter clay segment for claimed%, hill pill (amber uphill, green downhill), P50 and P90 dates.
- **One line per live session**, with a status light (green on task, amber detour or waiting, red needs you), the active task or detour, and the wait target.
- **Branch groups:** same-named branches across repos, with a combined %.

### 10.2 Drill-downs (click to expand)

- **Project:** plan tree with intents, estimate vs actual, verification level and stale badges, dependencies, the scope line, and re-plan history.
- **Session:** a timeline of labelled units, the time split (on-task, each detour, drift), suspended items, checkpoints, leftovers and their resolutions, the handoff note and read-back result.
- **Task:** decisions with their superseded-by chains, verification evidence, tokens and cost, actors, retries.
- **History:** the raw event stream with filters. Hidden by default.
- **Calibration:** ratios per model and the forecast's convergence over time.

### 10.3 Compact views

- `/tracker` (desktop app and CLI) and `tracker status`: about 6 lines of the glance screen, with ANSI colors in the terminal and the dashboard URL if it's running.

### 10.4 Visual theme (Claude Code palette)

- **Clay `#D97757`** is reserved for progress (done bar; claimed segment at about 35% opacity), owner-check items, and the ✱ mark.
- **Status colors:** green = on task, amber = detour, waiting or uphill, red = needs you.
- Neutral grays everywhere else. Session ids, branches and file names are in monospace.
- **Dark mode by default**, matching Claude Code's dark terminal. Light mode follows the system setting.
- A new red item pulses **once**, then stays still. The tab title shows the count (`(3) tracker`) and the favicon turns red while anything needs the owner.

## 11. Agent interface

### 11.1 Hooks

| Hook | Records | Navigator output |
|---|---|---|
| SessionStart (startup, resume, clear, fork) | `session_started`; recovery analysis; superpowers import check | Briefing (§8.4) |
| SessionStart (compact) | `session_started` | Short re-briefing |
| UserPromptSubmit | `prompt` | First prompt: claim instruction with the top 3 candidates. Later: owner actions, dependency changes |
| PreToolUse (Edit, Write, MultiEdit, Bash) | `tool_intent` (one per file per unit) | Read-back nudge, once |
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
| `status()` | Compact text state for the agent |

### 11.3 Skill

`skills/tracker/SKILL.md` teaches the protocol:
- claim the task, or ask if unsure
- write intents for goals and milestones
- open a detour with a resume cue before tangent work
- read back the handoff before editing
- checkpoint at boundaries
- log decisions
- climb the verification ladder honestly

It stays under about 150 lines.

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
- **Triggers:** `tracker import`, or automatically at SessionStart when a plan file is new or its content hash has changed.
- **Changes:** new vs. previous import, matched by task number and title, recorded as `plan_edit` events. If more than 30% of the tasks changed, it's recorded as a `replan`. The file is never written to.
- Missing estimates are filled in by the agent at its first claim.

### 12.2 Existing half-built repos

- `tracker init` runs inside a normal Claude session, guided by the skill. The agent scans the README, docs, existing plans and specs, the git log, TODO/FIXME comments, and test results. It drafts a goal (with intent), milestones and tasks, with statuses inferred from the code, and shows the owner the draft once to confirm or edit. The owner can seed it up front ("the goal is X").
- Tasks inferred as done start at **L1** and climb the ladder over the following sessions.

## 13. Build order (sub-projects)

All features ship. Each sub-project gets its own implementation plan, built in dependency order:

1. **Core store and plan.** SQLite schema, event append with views, rebuild, plan tree, `plan_edit`, CLI skeleton, `tracker install`.
2. **Hooks and sessions.** All hook handlers, units, active time, claiming, the MCP server skeleton with claim/plan tools, the skill.
3. **Navigator and tangents.** Drift score, labels, tolerance band, suspended items and resume cues, note rules.
4. **Handoff and recovery.** Checkpoints and the Stop block, end classification, leftover analysis and resolution, briefing, read-back, `tracker squash`.
5. **Verification and decisions.** Ladder, staleness, decision capture.
6. **Dashboard.** axum API, Svelte SPA, glance screen, needs-you actions, drill-downs, theme, `/tracker` compact view.
7. **Telemetry and forecast.** Transcript parsing, cost, calibration, Monte Carlo.
8. **Imports and onboarding.** Superpowers import and diff, `tracker init` for existing repos, branch groups and cross-repo dependencies.

Sub-projects 1–4 make the tracker useful on its own. The glance view from sub-project 6 can start early as a read-only page.

## 14. Testing strategy

- **Core modules:** unit tests against an in-memory SQLite, plus property tests checking that rebuilding views from events matches the incrementally maintained views.
- **Hooks:** golden tests that feed recorded hook JSON payloads to `tracker hook` and compare the events and stdout produced.
- **Recovery:** temporary git repos and worktrees with scripted crash scenarios (uncommitted edits, claimed-but-unfinished tasks, a live and a dead session side by side).
- **Concurrency:** N parallel processes appending to the database must lose no events.
- **MCP:** contract tests for each tool.
- **Dashboard:** API tests in Rust, plus component tests and one end-to-end smoke test (Playwright) for the glance screen and a needs-you action.
- **Performance:** a hook latency check (≤ 50 ms p95 on a database with 100k events).

## 15. Open questions for the owner

1. **Crash timeout:** is 30 minutes of silence right for calling a session crashed, given that long tool runs can be quiet?
2. **Squash timing:** should `tracker squash` also run automatically when the agent opens a PR, or stay manual only?
3. **Owner-verification nudges:** how many items may wait in the needs-you queue before the glance screen escalates (for example, the count pill pulses)?
4. **Threshold tuning:** may the tracker auto-tune the drift thresholds from the owner's overrides, or should it only suggest new values?
5. **Retention:** should events be kept forever, or should old projects be archivable or compactable?
6. **Open-sourcing later:** should anything (naming, config locations) be chosen now with that in mind?
