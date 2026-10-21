# Sub-project 7: Telemetry and Forecast — Implementation Plan

> **For agentic workers:** Execute task-by-task, test-first. Steps use checkbox (`- [ ]`) syntax. This plan gives interfaces, behaviour rules (pseudocode) and test cases — no full code. Read the cited spec sections and the existing code first and follow its patterns: events → projector → views (`Store::write`, `append_in`, `append_in_at`, `VIEW_TABLES`, schema migrations), hook handlers + the never-fail wrapper, MCP tools with `Args<T>`, the `view` read models every screen renders (glance, drill), `words.rs` / `web/src/lib/words.ts` for owner-facing wording, and the dashboard's type scale and motion rules. Add dependencies only with `cargo add` / `npm install` (prefer none).

**Goal:** The owner can see what each task, session and project cost (tokens and money), and when each project will likely be done — as a range ("likely by Oct 30, could take until Nov 8"), not a single guess — with that range getting tighter as work is verified.

**Architecture:**
- **Telemetry (core module `telemetry`):** the Stop and SubagentStop hooks read only the *new* bytes of the session's transcript (and the subagent's transcript), keep one byte cursor per file, and record one `usage` event per hook call listing each model reply once (deduplicated by message id). Replies are attributed to the unit, task, detour and agent that were active at the reply's timestamp. Money is computed when read, from a price table in `config.toml`, so a price fix applies to past usage too.
- **Forecast (core module `forecast`):** calibration ratios (actual active minutes ÷ estimate) from completed tasks, per model; a seeded Monte Carlo over the remaining tasks; P50/P90 agent-active hours turned into dates using the project's recent active hours per day; a `forecast_ran` event after each newly verified task keeps the history.
- **Screens:** the glance project row, `status`, `watch`, the desktop card, and new or extended drill-downs (calibration, task, session) show cost and the forecast in plain words.

**Spec:** `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` — §5.2 (`usage`), §5.5 (time and usage go to the task and any open detour), §6.2 (`usage`, `calibration`, `transcript_cursors` views), §9.2 (telemetry), §9.3 (forecast), §10.1 (P50/P90 dates on the project row), §10.2 (task drill: tokens, cost, actors, retries; calibration drill: ratios per model, forecast convergence), §11.1 (Stop/SubagentStop record usage).

**Branch / worktree:** `feat/sp7-telemetry-forecast` in `/Users/bedantsharma/PycharmProjects/session-plugin-sp7` (from `main` after the dashboard redesign). Work only there.

## Facts about the transcript format (checked on a real transcript, 2026-10-21)

- **One JSON object per line.** A model reply is a line with `"type": "assistant"`, `requestId`, `timestamp` (ISO), `isSidechain`, and `message` with `id`, `model` and `usage`.
- **`usage` fields:** `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`, and `cache_creation.{ephemeral_5m_input_tokens, ephemeral_1h_input_tokens}`. Others exist (`output_tokens_details.thinking_tokens`, `server_tool_use`, `service_tier`, `speed`); ignore unknown fields, tolerate missing ones.
- **The same reply is written on several lines with identical `usage`** (584 assistant lines held 282 distinct `message.id`s). **Count each `message.id` once** — summing lines roughly doubles tokens and cost.
- **Subagents write their own transcript** at `<transcript dir>/<session_id>/subagents/agent-<agent_id>.jsonl`, plus a `.meta.json` with `agentType` and `model`. The SubagentStop payload carries `agent_transcript_path`; when it's missing, derive that path.
- **Transcripts can be large** (7 MB for a long session) and are appended while hooks read them; the last line may be incomplete.

## Global Constraints

- Everything from sub-projects 1–6 still applies:
  - clippy pedantic `-D warnings`; no `unsafe` / `set_var`;
  - hooks never fail a session and stay ≤ 50 ms p95; no new git anywhere;
  - tests never touch the real `~/.claude` / `~/.bridge-screen` or run the real `claude`; test git calls use the scrubbed helpers;
  - commit per task ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; never `--no-verify`; chain shell steps with `&&`.
- **Privacy.** Read only the fields listed above. Never store message text, tool inputs or outputs, or prompts — only ids, model names, token counts and timestamps.
- **Hook budget.**
  - Each hook reads at most **512 KiB** of new transcript bytes per file, and only whole lines (stop at the last `\n`). Remaining bytes are picked up by later hooks.
  - Pre-filter lines with a cheap substring check (`"usage"` and `"assistant"`) before JSON parsing.
  - The Stop and SubagentStop latency tests must still pass with a 512 KiB backlog.
- **Determinism.** Projectors read event data only (reply timestamps come from the transcript, inside the event). The Monte Carlo uses a seeded generator: seed = hash(repo id, last event position used). Implement a small PCG/xorshift in core; no `rand` crate needed. The same inputs give the same P50/P90.
- **Money.**
  - Prices live in `config.toml` under `[prices."<model id>"]`: `input`, `output`, `cache_read`, `cache_write_5m`, `cache_write_1h`, in USD per million tokens. Each price matches the model id exactly, or by the longest prefix (e.g. `claude-opus-5`).
  - Ship defaults for the current Claude models. Take the numbers from the `claude-api` skill's pricing reference (use the Skill tool), and record the date they were taken in a comment in the default config.
  - A model with no price: tokens still count, cost shows as "unknown" (never $0), and a "no price for <model>" note appears in `status` and the calibration drill.
  - Money is computed at read time, never stored in views.
- **Owner-facing wording** follows the lead-briefing rules in `CLAUDE.md`:
  - plain words, the effect first, no raw codes;
  - forecast ranges read "likely by <date> · could take until <date>", with a tooltip and help text: "50% / 90% chance, based on how long verified tasks actually took".
  - Never show a date when the data can't support one; say why instead ("not enough finished tasks to forecast yet — need 3 more").

## Review Focus

1. **Duplicate reply lines counted once:** across one read, across two reads that split a reply's lines, and after a re-read from offset 0. Test: `usage_counts_each_reply_once` (Task 2).
2. **A big backlog doesn't slow hooks:** a 7 MB transcript is consumed over successive hooks, 512 KiB at a time, each hook within budget; the totals end up identical to one full read. Test: `backlog_is_read_in_bounded_slices` (Task 2) plus the latency test (Task 7).
3. **A half-written last line:** a line without its trailing `\n` is not consumed; once completed, it's counted exactly once. If a file shrank or was replaced (size < cursor), restart from 0, with dedupe preventing double counting. Test: `partial_lines_and_shrunk_files` (Task 2).
4. **Unknown prices never show as $0:** an unknown model shows "unknown" cost plus a note; known models still sum. Test: `unknown_model_cost_is_unknown` (Task 3).
5. **No fake forecasts:**
   - with fewer than 3 completed calibration points, or no recent activity for hours/day, the forecast shows no dates and the reason;
   - unestimated remaining tasks are excluded and counted ("2 tasks have no estimate");
   - P50 ≤ P90 always;
   - identical inputs give identical output.

   Test: `forecast_refuses_thin_data_and_is_deterministic` (Task 4).
6. **Rebuild equality:** usage, cursors, calibration and forecast history rebuild identically from events. Extend `tests/rebuild.rs`.

---

## File Structure

```
crates/bridge-screen-core/src/
  store/schema.rs          # V8 migration
  telemetry/mod.rs         # parse, dedupe, attribution, totals queries, cost
  telemetry/read.rs        # bounded transcript reader (cursor, whole lines, prefilter)
  telemetry/project.rs     # USAGE_KINDS projector: usage_replies, transcript_cursors
  telemetry/tests.rs
  forecast/mod.rs          # calibration, Monte Carlo, hours/day, Forecast model
  forecast/rng.rs          # small seeded PRNG
  forecast/project.rs      # forecast_ran → forecast_history
  forecast/tests.rs
  config.rs                # [prices] table + defaults
  view/mod.rs, view/drill.rs  # forecast + cost in glance; calibration drill; task/session usage
crates/bridge-screen/src/
  hook/handlers.rs         # Stop / SubagentStop read usage
  mcp.rs                   # status() adds cost + forecast; verify → forecast_ran
  commands.rs, cli.rs      # `bridge-screen usage`, `bridge-screen forecast`
  status.rs, watch.rs, report.rs, words.rs
web/src/lib/glance/ProjectRow.svelte, lib/drill/{Calibration,Task,Session}.svelte, card/Card.svelte, lib/words.ts
```

---

### Task 1: Schema V8 and the usage model

**Views (V8; `SCHEMA_VERSION = 8`):**
- `usage_replies(message_id PK, repo_id, session_id, agent_id NULL, agent_type NULL, model, ts, unit_no NULL, task_id NULL, detour_id NULL, input, output, cache_read, cache_write_5m, cache_write_1h)`, with indexes `(repo_id, task_id)` and `(session_id)`.
- `transcript_cursors(path PK, session_id, agent_id NULL, offset INTEGER)`.
- `forecast_history(repo_id, ts, p50_hours, p90_hours, p50_date NULL, p90_date NULL, remaining_min, unestimated, points, hours_per_day NULL, reason NULL)`, with index `(repo_id, ts)`.

**Events:**
- `usage { path, offset_after, agent_id?, agent_type?, replies: [{ message_id, model, ts, input, output, cache_read, cache_write_5m, cache_write_1h, sidechain }] }`
- `forecast_ran { p50_hours, p90_hours, p50_date?, p90_date?, remaining_min, unestimated, points, hours_per_day?, reason? }`

**Projector rules:**
```
usage:
  upsert transcript_cursors(path) offset = max(existing, offset_after)
  for reply in replies:
     unit = the event session's unit whose started_at ≤ reply.ts ≤ (stopped_at or +∞), latest such
     task, detour = unit.task_id, unit.detour_id   (none if no unit)
     INSERT OR IGNORE usage_replies(message_id, …)        # dedupe across events
  (when offset_after is less than the stored offset, i.e. a restart after a shrink: set the offset to offset_after)
forecast_ran: insert into forecast_history
```
- **Totals queries** (`telemetry::totals(conn, scope) -> Tokens { input, output, cache_read, cache_write_5m, cache_write_1h, by_model: map }`), with `scope` = Task(repo, id) | Detour(id) | Session(id) | Repo(id) | Agent(session, agent_id). A task total includes replies whose unit had that task, including detours opened under it (spec §5.5: usage goes to the task and also to the open detour).
- **Cost:** `telemetry::cost(tokens, prices) -> Cost { usd: Option<f64>, unpriced_models: Vec<String> }`. `usd` is None when any token belongs to an unpriced model *and* nothing else is priced; otherwise it is the priced part plus `unpriced_models`, and screens show "≥ $X (some models unpriced)".

- [ ] Failing tests (in-memory store, timestamped events):
  - V7→V8 migration keeps the data.
  - Attribution by timestamp: a reply inside unit 3 goes to unit 3's task and detour; a reply before any unit gets no task.
  - A duplicate `message_id` in a later event is ignored.
  - Cursor rules.
  - Totals per scope; the task total includes its detours.
  - Rebuild equality extended.
- [ ] Implement; commit `core: usage model and transcript cursors (schema v8)`.

---

### Task 2: Reading transcripts from the hooks

**Reader — `telemetry::read_new(path, from_offset, max_bytes) -> ReadOut { replies, offset_after, more: bool }`:**
```
open the file (missing → empty result, no error)
size < from_offset → from_offset = 0 (file replaced or truncated)
read min(max_bytes, size - from_offset) bytes from from_offset
cut at the last '\n' (no newline in the slice → read nothing, unless the slice already hit max_bytes:
   then extend to the next '\n' up to 2× max_bytes, so one huge line can't stall the cursor forever;
   still no newline → skip to the end of that line on the next call and log)
for each line containing "\"usage\"" and "\"assistant\"": parse; keep type == assistant with message.id + usage
dedupe by message.id inside this read (keep the first)
offset_after = from_offset + bytes consumed; more = offset_after < size
```

**Hooks:**
- **Stop:**
  - path = session.transcript_path or the payload's `transcript_path`;
  - cursor = `transcript_cursors[path]` (default 0);
  - `read_new(path, cursor, 512 KiB)` → when there are replies, or the offset moved, append `usage { path, offset_after, replies }` in the same write as `unit_stopped`, with replies already present in `usage_replies` filtered out first (cheap PK lookup).
- **SubagentStop:** the same, for `agent_transcript_path`, falling back to `<dirname(transcript_path)>/<session_id>/subagents/agent-<agent_id>.jsonl`, with `agent_id` / `agent_type` on the event.
- **SessionEnd:** one last read of the main transcript (still bounded).
- Any I/O or parse error → log to `hook-errors.log`, record nothing, and never fail the hook.
- **Catch-up:** sessions that existed before this upgrade start from offset 0 and catch up 512 KiB per Stop. CLI `bridge-screen usage --catch-up [-C DIR]` reads every known transcript of the repo to the end (unbounded; owner-run, not a hook) and prints what it added.

- [ ] Failing tests (temp files, binary hooks):
  - `usage_counts_each_reply_once`: three lines for one reply → counted once; a reply split across two reads → once; a re-read from 0 after a shrink → no double count.
  - `backlog_is_read_in_bounded_slices`: a ~3 MB synthetic transcript is consumed over several Stops, each reading ≤ 512 KiB, and the totals equal one full read via `--catch-up`.
  - `partial_lines_and_shrunk_files`.
  - A subagent transcript is attributed to the parent session's task with `agent_type`.
  - The derived subagent path is used when `agent_transcript_path` is missing.
  - A missing transcript → no event, no error.
  - Non-assistant lines and lines with content but no usage are ignored.
  - Nothing beyond the listed fields is stored (assert the event JSON keys).
- [ ] Implement; commit `hooks: read transcript usage incrementally at Stop and SubagentStop`.

---

### Task 3: Prices and cost

- `Config.prices: BTreeMap<String, Price { input, output, cache_read, cache_write_5m, cache_write_1h }>` (USD per million tokens).
  - Defaults are written by `write_default_if_missing` with a comment naming the source and date.
  - When loading an existing config file without `[prices]`, the built-in defaults are used. Owner entries override defaults per key.
- Lookup: exact model id, else the longest configured prefix (`claude-opus-5-5` → `claude-opus-5`), else unpriced.
- `bridge-screen usage [-C DIR] [--session PREFIX] [--task ID] [--by model|task|session|agent]`
  - prints tokens and cost in plain words, e.g. `Login form (2.3): 1.2M tokens · $4.10 (Opus 5.5 $3.90, Haiku 5.5 $0.20)`;
  - totals per project.
- `bridge-screen config` shows the price table and marks which entries come from defaults.

- [ ] Failing tests:
  - prefix matching;
  - owner override;
  - `unknown_model_cost_is_unknown`;
  - cache write 5m vs 1h priced separately;
  - large numbers are formatted readably (1.2M tokens, $4.10);
  - CLI output for each `--by`.
- [ ] Implement; commit `core: price table and cost; cli: usage`.

---

### Task 4: Calibration and the Monte Carlo forecast

**Calibration — `forecast::calibration(conn, repo, now) -> Calibration { by_model: map<model, Vec<Point>>, pooled: Vec<Point> }`:**
```
completed leaf = task/subtask that reached ≥ L2, the first time, ≤ 6 months before now, with estimate_min > 0
actual_min = task_time active minutes for the leaf (includes its detours; spec §5.5); skip actual 0
ratio = actual_min / estimate_min ; Point { task_id, ratio, model, completed_at }
model of the leaf = the model with the most output tokens on the leaf (usage), else the session's model, else "unknown"
```

**Forecast — `forecast::run(conn, repo, now, runs = 1000) -> Forecast`:**
```
remaining = live leaves below L2 with estimate_min > 0 ; unestimated = live leaves below L2 without one
points for a leaf = calibration.by_model[leaf's likely model] if ≥ 6 points, else pooled if ≥ 3, else none
if pooled < 3 → Forecast { dates: None, reason: "Not enough finished tasks to forecast yet — N more needed" }
seed = hash(repo_id, max event pos considered)
for run in 1..runs:
   total = Σ over remaining leaves: max(0, estimate × random_choice(points) − active_min_already_spent)
record totals (minutes); p50 = median, p90 = 90th percentile (nearest-rank); p50 ≤ p90
hours_per_day = active minutes on this repo over the last 14 calendar days ÷ (number of those days with any activity), ÷ 60
   (no activity in 14 days → dates None, reason "No recent activity to turn hours into dates")
dates = now + ceil(p50_hours / hours_per_day) calendar days, the same for P90
Forecast { p50_hours, p90_hours, p50_date, p90_date, remaining_min, unestimated, points: pooled.len(), hours_per_day, reason }
```
- The spec's "wide default distribution with fewer than 6 points" is the pooled fallback; below 3 pooled points we refuse rather than invent a distribution (owner rule: never round up to green).
- **History:** after any `verified` event at ≥ L2, in the same process — MCP `verify`, CLI `bridge-screen verify`, the owner's dashboard action — never in hooks: run the forecast and append `forecast_ran` when P50 or P90 changed by ≥ 1% or the reason changed.
- **CLI:** `bridge-screen forecast [-C DIR]` prints the forecast in plain words with its basis ("based on 12 finished tasks; this project averages 1.8× its estimates"), and the remaining and unestimated counts.

- [ ] Failing tests:
  - calibration picks only ≥ L2 leaves in the window, uses active minutes, and chooses the model by output tokens;
  - `forecast_refuses_thin_data_and_is_deterministic`;
  - with a ratio of always exactly 1.0 and no time spent, P50 = P90 = Σ estimates;
  - time already spent is subtracted;
  - hours/day and date maths, including the no-activity case;
  - `forecast_ran` is appended after a verify at L2 and not after L1;
  - the 1% threshold avoids noise;
  - rebuild equality for the history.
- [ ] Implement; commit `core: calibration and Monte Carlo forecast; cli: forecast`.

---

### Task 5: Screens — glance, status, watch, card, MCP status

- **Model:**
  - `ProjectRow.forecast` (currently always `None`) becomes the `Forecast`;
  - `ProjectRow.cost` = repo total tokens and cost;
  - `SessionRow.cost` = session tokens and cost.
- **Glance project card:**
  - under the big %, a line "likely by Oct 30 · could take until Nov 8", with a tooltip and screen-reader text: "50% and 90% chance, based on how long N verified tasks actually took";
  - or the plain reason when there are no dates;
  - a muted meta line "$12.40 spent · 3.1M tokens", or "≥ $… (some models unpriced)".
- **Glance session row:** cost in the meta column ("$1.20").
- **`status`:** the project line adds `· likely Oct 30–Nov 8 · $12.40`. Plain-text tests updated deliberately.
- **`watch`:** the same text on the project line; cost on session rows.
- **Card:** the project line adds the forecast range and cost (compact).
- **MCP `status()`:** cost so far for the session and the active task, plus the project forecast line.
- **Progress report:** adds the forecast and total cost.

- [ ] Failing tests:
  - model fields;
  - the glance/card vitest snapshots of the new lines (no date shown when the reason is set);
  - status and watch text;
  - MCP status text.
- [ ] Implement; commit `screens: forecast range and cost on glance, status, watch, card`.

---

### Task 6: Drill-downs — calibration, task, session

- **Calibration drill** (new route `/api/calibration/{repo}`, Svelte `Calibration.svelte`, linked from the project drill next to History and Drift tuning):
  - per model: point count, median ratio ("tasks take 1.8× their estimate"), and a dot plot of ratios (LayerChart);
  - the forecast history as a P50/P90 band over time ("forecast convergence"), using the existing small-data fallback when fewer than 2 points;
  - the price table in use, with unpriced models flagged;
  - the existing drift-tuning section can stay on its own page.
- **Task drill:** tokens and cost (by model and by agent type), actors, **retries** (units on the task after a `verification_failed`), **steps** (subtask count with the current one marked; spec §9.2), estimate vs actual and the ratio once done.
- **Session drill:** a cost and tokens summary; each timeline row gains a tokens column ("12k").
- Follow the dashboard's type scale, chips, numbers right-aligned and tabular, and the words in `words.ts` (no raw codes).

- [ ] Failing tests: drill models; API route auth (token + Host as the other routes); vitest for the Calibration page (empty state, thin-data reason, chart fallback); the e2e screens test adds a calibration screenshot.
- [ ] Implement; commit `web: calibration drill; cost and retries in task and session drills`.

---

### Task 7: Docs, latency, living docs

- **`SKILL.md`:** nothing new for agents beyond knowing `status` shows cost and the forecast (keep within the line limit).
- **`CLAUDE.md`:** layout entries (`telemetry/`, `forecast/`) and commands (`usage`, `usage --catch-up`, `forecast`).
- **README:** cost and forecast, with what P50/P90 mean in plain words.
- **`docs/lead/` living docs:**
  - `glossary.md`: P50/P90, calibration, tokens;
  - `project-map.md`: telemetry and forecast as parts;
  - `decision-log.md`: an entry for "refuse to forecast below 3 finished tasks" and "cost computed at read time from config prices".
- **Latency:** add Stop and SubagentStop cases with a 512 KiB pending transcript to the ignored latency test; run all latency tests in release and record the numbers.
- **Checks:** run the full check (Rust fmt/clippy/tests, web lint/check/format/build/test/e2e) and `claude plugin validate ./plugin`.
- [ ] Commit `docs: telemetry and forecast; latency recheck`.
