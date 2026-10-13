# Sub-project 3: Navigator and Tangents — Implementation Plan

> **For agentic workers:** Execute task-by-task, test-first. Steps use checkbox (`- [ ]`) syntax. This plan gives interfaces, behaviour rules (pseudocode) and test cases — no full code. Read the cited spec sections and the existing code (sub-projects 1–2) before each task and follow its patterns (events → projector → views, `Store::write`, `append_in`, `VIEW_TABLES`, hook handlers in `crates/bridge-screen/src/hook/`, MCP tools in `src/mcp.rs` with the `Args<T>` wrapper and session binding).

**Goal:** Every unit of work gets a label — on-task, detour or drift — from the agent's own declaration plus a cross-check at Stop. Detours are tracked with their own time, suspend the current item with a resume cue, and return the agent to its task when they close. Time away from the task is measured against a tolerance band that turns sessions amber/red. A navigator sends at most one short note per unit through the cheapest channel that works, and drift thresholds tune themselves from the owner's and agent's corrections.

**Architecture:** New core modules `tangent` (detours, suspended items, labels, away time, tolerance) and `navigator` (note selection, priority, rate limits). A pure function `drift::score` computes the heuristic. All state is events + views (schema V3). Notes reach the agent through three channels: MCP tool results (immediate), `PostToolUse` additional context (mid-turn, free), and `Stop` additional context (forces another turn — used only for a label request, at most once per unit).

**Tech Stack:** existing workspace only. No new dependencies expected; if one is needed, add it with `cargo add`.

**Spec:** `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` — §3 (commander's intent, cross-track error, Altmann & Trafton, andon), §5.2 (tangent events), §5.5 (time charged to task and detour), §7.1–7.6 (all of tangent detection and the navigator), §11.1–11.2 (hooks and MCP tools).

## Global Constraints

- Everything from sub-projects 1–2 still applies: clippy pedantic `-D warnings`, no `unsafe`/`set_var`, hooks never fail a session and stay fast (no `git` on the known-session path; recheck the latency test), prompt text never stored, tests never touch the real `~/.claude`/`~/.bridge-screen` or run state-changing `claude` commands, commit per task with the `Co-Authored-By` line.
- **Never block tangents.** Nothing in this sub-project denies a tool call or prevents a stop, except the single label-request continuation below.
- **Stop continuation budget:** a `Stop` hook may return additional context (which makes Claude take another turn) at most once per unit, and never when the Stop input says a stop hook is already active (verify the field name — `stop_hook_active` — in https://code.claude.com/docs/en/hooks; also keep our own per-unit flag so the guarantee doesn't depend on it).
- Projectors use event timestamps only. Anything computed from "now" (live away time in a hook) is used for the decision and never written into views directly — if it matters, it goes into an event's data.
- Thresholds are config with defaults: `drift_lower = 0.3`, `drift_upper = 0.7`, `tolerance_min_minutes = 15`, `tolerance_pct = 20`. Per-repo tuned values (Task 6) override them.
- Note text is 1–3 lines and ends with the one action expected.

## Review Focus

1. **Runaway continuations:** a Stop that returns context must not be able to loop — a second Stop in the same unit (or with `stop_hook_active`) returns nothing. Test: `label_request_is_sent_once_per_unit` (Task 4).
2. **Nested and out-of-order detours:** detour inside a detour, closing them in a different order, resuming a suspended item directly — the active focus and each detour's time must stay right. Test: `nested_detours_resume_any_order` (Task 3).
3. **Agent and heuristic disagree:** the agent says on-task, the score says 0.9 — the label stays the agent's, the disagreement is recorded, the session shows ⚠, and a drift sample is stored. Test: `disagreement_is_recorded_not_argued` (Task 4).
4. **Tolerance with no estimate:** a task without `estimate_min` uses the 15-minute floor; a detour with no active task (session never claimed) still counts away time. Test: `tolerance_without_estimate_or_task` (Task 5).
5. **Auto-tuning guard rails:** a skewed sample set can never push a cut-off more than 0.05 per run, outside its bounds, or make lower ≥ upper. Test: `tuning_respects_guard_rails` (Task 6).

---

## File Structure

```
crates/bridge-screen-core/src/
  store/schema.rs           # V3 migration
  tangent/mod.rs            # detours, suspended items, labels, away time, tolerance (model + queries)
  tangent/project.rs        # projector for tangent events
  drift.rs                  # pure drift-score heuristic
  navigator.rs              # note candidates, priority, rate limit, text
  tuning.rs                 # auto-tuning of thresholds
  config.rs                 # new keys + per-repo overrides
  projection.rs             # route new events, new VIEW_TABLES
crates/bridge-screen/src/
  hook/handlers.rs          # Stop / PostToolUse / UserPromptSubmit / SessionStart wiring
  mcp.rs                    # start_detour, resume, close_detour, label_unit
  cli.rs, commands.rs       # `label`, `drift`, `config reset-drift`
  status.rs                 # show labels, detours, nav state
plugin/skills/bridge-screen/SKILL.md   # detour + labelling protocol
```

---

### Task 1: Schema V3 and the tangent model

**Views (V3 migration from V2; `SCHEMA_VERSION = 3`):**
- `detours(detour_id PK, session_id, repo_id, blocks_task, reason, resume_cue, opened_at, opened_unit, closed_at, outcome, active_secs, promoted_task)`
- `suspended_items(item_id PK, session_id, kind task|detour, ref_id, resume_cue, suspended_at, resumed_at)`
- `units` gains: `label` (on_task|detour|drift|NULL), `label_source` (declared|heuristic|agent|owner), `drift_score REAL`, `detour_id`, `disagreement INTEGER`
- `sessions` gains: `current_detour`, `away_secs`, `away_since`, `nav_state` (green|amber|red), `ack_away_secs` (owner said "continue" at this much away time)
- `drift_samples(repo_id, unit_ref, score, label, source, ts)` — one per agent/owner label that differs from the heuristic, or any owner label
- `repo_tuning(repo_id PK, drift_lower, drift_upper, tolerance_pct, samples_at_last_run)`

**Events (data fields):**
- `detour_opened { detour_id, reason, blocks, resume_cue }` — `blocks` = task id, or null if the session has no task
- `detour_closed { detour_id, outcome: done|abandoned, note? }`
- `detour_promoted { detour_id, task_id }`
- `item_suspended { item_id, kind, ref_id, resume_cue }`, `item_resumed { item_id }`
- `unit_labeled { unit_no, label, source, score, why? }`
- `drift_flagged { unit_no, score }`
- `nav_state_changed { from, to, away_secs }` (emitted by hooks when the computed state changes)
- `owner_action { action, target, … }` — reuse for drift answers and label overrides
- `note_sent { unit_no, kind, channel }`
- `config_changed { scope: repo, key, old, new, samples }`

**Projector rules (pseudocode):**
```
detour_opened:  insert detour (opened_at=ts, opened_unit=open unit no)
                suspend current focus: if session.current_detour → item(kind detour) else if active_task → item(kind task)
                session.current_detour = detour_id ; open unit.detour_id = detour_id
detour_closed:  detour.closed_at, outcome ; session.current_detour = the detour this one was opened inside (if still open) else null
item_resumed:   resumed_at = ts ; set session focus to that item (task → current_detour null; detour → current_detour = it)
unit_stopped (existing) additionally:
                if unit.detour_id: detours[detour_id].active_secs += unit.active_secs
unit_labeled:   set unit.label/source/score; if source in (agent, owner) and differs from heuristic label → disagreement=1, add drift_sample
                if source == owner → always add drift_sample
                away time: label != on_task → session.away_secs += unit.active_secs (away_since = first such unit's start)
                           label == on_task → away_secs = 0, away_since = null, ack_away_secs = 0
nav_state_changed: session.nav_state = to
owner_action(drift, continue): ack_away_secs = session.away_secs
config_changed: upsert repo_tuning
```
`tangent::` queries: `open_detours(conn, session)`, `suspended(conn, session)`, `focus(conn, session) -> Focus { task: Option<String>, detour: Option<Detour> }`, `detours_for_task(conn, repo, task)`.

- [ ] Failing tests (core, timestamped events): V2→V3 migration keeps data; open → work → close a detour charges unit time to both task and detour; `nested_detours_resume_any_order` (open D1, open D2 inside, resume the task directly, close D1 — focus, suspended list and per-detour seconds all correct); labels drive away time (two detour units then an on-task unit resets it); disagreement creates a drift sample; extend the rebuild-equality test with a scripted detour session.
- [ ] Implement; commit `core: tangent model — detours, suspended items, labels, away time (schema v3)`.

---

### Task 2: Navigator engine

**Interface:**
- `navigator::NoteKind` in priority order: `Checkpoint` (reserved for sub-project 4), `ReturnToTask`, `ReadBack` (reserved, sub-project 4), `LabelRequest`, `ToleranceWarning`, `DriftRed`, `OwnerReply`, `DependencyReady` (reserved), `VerifyNudge` (reserved, sub-project 5). Reserved kinds exist in the enum but nothing produces them yet.
- `navigator::Channel { ToolResult, PostToolUse, Stop, PromptSubmit }`.
- `navigator::Candidate { kind, channel, text, state_key: String }` — `state_key` identifies the situation (e.g. `"tol:amber:D7"`); a changed key means "state changed".
- `navigator::select(conn, session, unit_no, channel, candidates) -> Option<Candidate>` — applies the rules below; the caller then emits `note_sent` and returns the text through its channel.

**Rules:**
```
keep only candidates whose channel == the calling channel
drop any whose kind was already sent in this unit (one note per unit overall: if ANY note_sent exists for this unit, return None)
drop any whose (kind, state_key) was sent within the last 3 units
pick the highest-priority remaining
```
Text helpers produce the fixed formats used in later tasks.

- [ ] Failing tests: priority order; one note per unit; same kind+state suppressed for 3 units then allowed; a changed `state_key` is allowed immediately; channel filtering.
- [ ] Implement; commit `core: navigator note selection and rate limits`.

---

### Task 3: Detour tools and resume cues

**MCP tools (add to `mcp.rs`, with schemas and field docs):**
| Tool | Input | Result text |
|---|---|---|
| `start_detour` | `{ reason, resume_cue, blocks? }` — `blocks` defaults to the active task; `resume_cue` required, non-empty, ≤ 200 chars | `Detour D<n> opened: <reason>. Suspended <id> <title> — on return: <cue>.` |
| `close_detour` | `{ outcome: "done"\|"abandoned", note? }` | `Detour closed. Return to <id> <title>. On return: <cue verbatim>.` (or `No suspended item; continue with your task.`) |
| `resume` | `{ item_id }` | `Resumed <id> <title>. On return: <cue>.` |
| `promote_detour` | `{ parent, title, estimate_min? }` | `Detour D<n> is now task <id>.` — adds the plan node (reason "promoted from detour") and claims it |

`D<n>` is a short per-session detour number for display; ids stay ULIDs internally. These results are the `ReturnToTask` channel = `ToolResult`, and each emits `note_sent`.

**Behaviour:**
```
start_detour:  require a live session (binding); emit detour_opened + item_suspended in one transaction
close_detour:  close the session's current detour; return cue of the item it suspended (the item whose ref is detour.blocks
               or the outer detour); resume that item (item_resumed) in the same transaction
resume:        item must belong to the session and be unresumed
```

- [ ] Failing tests (MCP over stdio as in sub-project 2's tests, with bindings via the hook path): open/close round-trip returns the cue verbatim; close with nothing open → tool error; resume someone else's item → tool error; promote creates and claims a task and links `promoted_task`; empty/oversized cue rejected.
- [ ] Update `SKILL.md`: before tangent work, call `start_detour` with a one-line resume cue naming the next action and the file/test; close it when done; never hand a tangent to another agent; a long detour may be promoted.
- [ ] Implement; commit `mcp: detours with resume cues`.

---

### Task 4: Drift score and labelling at Stop

**`drift::score(input) -> ScoreBreakdown { score: f64 (0..=1), file_affinity: Option<f64>, text_affinity: f64, area_penalty: f64 }`** — pure function, no DB:
```
input: unit files; unit summary text (last_assistant_message, full text, in memory only);
       task-file set = task.expected_files ∪ files of earlier units on this task ∪ files of units on sibling tasks in the same milestone;
       intent text = titles + intents of task, its milestone, the goal;
       mapped dirs = top-level dirs of every file in the whole plan's expected_files and unit history

file_affinity = share of unit files that are in the task-file set or share a directory with one   (None if unit has no files or task-file set empty)
text_affinity = |keywords(summary) ∩ keywords(intent text)| / max(1, min(|keywords(summary)|, 12)), capped at 1
area_penalty  = share of unit files whose top-level dir is not in mapped dirs   (0 if no files)

weights: file 0.5, text 0.3, area 0.2 ; if file_affinity is None, renormalise text/area to 0.6/0.4
score = Σ weight × (1 − affinity) for file/text, + weight × area_penalty for area
no files and empty summary → score 0
keywords: lowercase alphanumeric tokens ≥ 3 chars, stop-words removed, simple plural trim
```

**Stop flow (hook handler):**
```
if a detour was open during the unit → unit_labeled {detour, declared}
else:
  s = drift::score(...)  ; thresholds from repo_tuning or config
  s < lower                → unit_labeled {on_task, heuristic, s}
  lower ≤ s ≤ upper        → candidate LabelRequest (channel Stop): "bridge-screen: label your last unit — on-task, detour or drift — with label_unit (one call)."
                             if not selected/allowed → unit_labeled {on_task, heuristic, s} now
                             else unit stays unlabeled until label_unit; if the session's next prompt arrives first → label on_task, heuristic
  s > upper                → drift_flagged {s}; candidate LabelRequest as above, text adds "possible drift"
the label request is the only Stop continuation; respect stop_hook_active + per-unit flag
```
**`label_unit` MCP tool** `{ label, why }`: labels the most recent stopped-but-unlabeled unit (or the last unit if all labelled — then it's a correction) with source `agent`; if it disagrees with the heuristic label implied by the score, the projector records the disagreement and a drift sample. The tool never argues; result text `Labelled unit <n> as <label>.`

- [ ] Failing tests: score breakdown on hand-built inputs (Docker-files unit vs task files → > 0.7; unit editing the task's own files with matching words → < 0.3; no files → text/area only); declared detour wins; low score labels silently; middle score sends one continuation and `label_request_is_sent_once_per_unit` (second Stop in the same unit, and `stop_hook_active: true`, return nothing); `disagreement_is_recorded_not_argued`; unlabeled unit gets on_task when the next prompt arrives; summary text is not stored anywhere beyond the existing 300-char `unit_stopped.summary`.
- [ ] Implement; commit `core: drift score; hooks: label units at stop; mcp: label_unit`.

---

### Task 5: Tolerance band, live warnings and owner answers

**Tolerance:** `tol_secs = max(tolerance_min_minutes·60, tolerance_pct% × active task estimate·60)`; no active task or no estimate → floor only. Away time used for decisions = `session.away_secs − ack_away_secs`, plus, inside an open unit with an open detour, the live seconds of that unit so far (computed in the hook from "now", minus waits).

**State:**
```
away ≤ tol                         → green
tol < away ≤ 2·tol                 → amber
away > 2·tol, or drift_flagged with no detour declared and agent didn't label it detour → red
```
Hooks (PostToolUse, Stop, UserPromptSubmit) compute the state; when it differs from `session.nav_state` they emit `nav_state_changed`. Red also emits `needs_owner { kind: "drift" }` so `status` lists it under needs-you.

**Notes:**
- Amber → `ToleranceWarning` via **PostToolUse** (mid-turn, free): `bridge-screen: you've been on "<reason>" for <m> min (~<x>× the task estimate). Continue if it still blocks <task>; otherwise close the detour and return.` state_key `tol:amber:<detour or 'none'>`.
- Red → `DriftRed` via PostToolUse: `bridge-screen: this detour is well past tolerance; the owner has been flagged. Keep going if it's essential, or close it and return to <task>.` If the repo setting `drift_pause = true`: `… Finish your current step, commit, then stop and ask the owner before continuing.`
- Owner answers → `OwnerReply` via UserPromptSubmit or PostToolUse, whichever fires first: `bridge-screen: the owner says <continue | make this a task | stop the detour and return to <task>>.`

**CLI for the owner (dashboard buttons arrive in sub-project 6):**
- `bridge-screen drift <session-prefix> continue|make-task|stop [--title T --parent P]` → `owner_action`; `continue` sets `ack_away_secs`; `make-task` runs promotion; `stop` queues the OwnerReply note.
- `bridge-screen label <session-prefix> <unit_no> on-task|detour|drift` → `unit_labeled {source owner}`.
- Session prefix: unique prefix of a live session id; ambiguous/none → clear error.

- [ ] Failing tests: `tolerance_without_estimate_or_task`; green→amber→red transitions with timestamped events and live time; amber warning sent once per state (rate-limited) then again after the state changes; red sets needs_owner and appears first in `status`; `continue` drops back to green by acknowledging; `make-task` promotes; `stop` delivers the reply once; drift_pause wording; label override creates an owner drift sample.
- [ ] Implement; commit `core: tolerance band; hooks: live warnings; cli: owner drift and label answers`.

---

### Task 6: Auto-tuning (spec §7.6)

**`tuning::run(conn, repo) -> Option<TuningChange>`**, triggered from the SessionStart hook when `drift_samples` for the repo has grown by ≥ 20 since `samples_at_last_run`:
```
samples = last 200 for the repo, each (score, label)
for lower: pick the value in [0.15, 0.5] (step 0.01) that minimises "on_task samples with score ≥ lower" + "non-on_task samples with score < lower" — ties → closest to current
for upper: pick in [0.5, 0.9] minimising "drift samples with score ≤ upper" + "on_task/detour samples with score > upper"
move each from current toward its best by at most 0.05 ; enforce lower < upper − 0.05
tolerance_pct: owner 'continue' answers at amber/red since last run push +5 (max), 'stop' answers push −5, net clamp, bounds 10..=60
emit config_changed for each key that changed (old, new, samples) — no event if nothing moved
```
- `bridge-screen config reset-drift [-C DIR]` emits `config_changed` back to defaults for that repo.
- `status`/`config` show tuned values for the current repo.

- [ ] Failing tests: `tuning_respects_guard_rails` (extreme skew → moves exactly 0.05, stays in bounds, lower < upper); separable samples move cut-offs toward the gap over several runs; fewer than 20 new samples → no run; reset restores defaults; tuned values are what the Stop flow uses; rebuild reproduces `repo_tuning`.
- [ ] Implement; commit `core: drift threshold auto-tuning`.

---

### Task 7: Status, skill, docs and latency

- `status` per session line gains: nav light word (`on task` / `detour 25m` / `drift ⚠ 40m`), current detour reason, and `⚠` when the last unit had a disagreement. Needs-you order: red drift first. Keep ≤ ~12 lines.
- `status()` MCP tool: also lists open detours and suspended items with their resume cues.
- `SKILL.md`: add labelling (`label_unit` when asked; be honest — drift is information, not blame) and what amber/red notes mean. Keep it short.
- `CLAUDE.md`: mention the `tangent`, `drift`, `navigator`, `tuning` modules and new CLI commands.
- Re-run the latency test (`cargo test --release -p bridge-screen -- --ignored hook_latency`) — Stop and PostToolUse now do more work; p95 must stay < 50 ms. Add a Stop-path latency case with a unit of 30 files on a plan of 300 nodes.
- `claude plugin validate ./plugin` and `.` (read-only) still pass.
- [ ] Commit `docs: navigator in status, skill and CLAUDE.md; latency recheck`.
