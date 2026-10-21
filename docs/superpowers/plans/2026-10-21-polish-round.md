# Polish Round — Implementation Plan

> **For agentic workers:** Execute task-by-task, test-first. Steps use checkbox (`- [ ]`) syntax. This plan gives interfaces, behaviour rules (pseudocode) and test cases — no full code. Read the cited spec sections and the existing code first and follow its patterns: events → projector → views (`Store::write`, `append_in`, `VIEW_TABLES`, schema migrations), hook handlers + the never-fail wrapper, MCP tools with `Args<T>`, `owner::act` for every owner action, the `view` read models (glance, drill), and `crates/bridge-screen/src/words.rs` / `web/src/lib/words.ts` for owner-facing wording. Add dependencies only with `cargo add` / `npm install` (prefer none).

**Goal (the problem this round fixes):** bridge-screen raises false alarms and talks to the owner in its own jargon, which costs the owner attention every day. After this round:
- work the owner asked for is never flagged as drift;
- finished milestones and the goal stop showing "to do";
- branch names stay current;
- a sign-off can be taken back;
- an upgrade tells open sessions what to do;
- the card's task form is tidy;
- every owner-facing message reads in plain words.

**Spec:** `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` — §7.1–7.3 (labels, drift, the owner's answer), §8.5 (verification ladder, the owner's verdict), §10.1–10.2 (screens), §11 (hooks).

**Branch / worktree:** `feat/polish-round` in `/Users/bedantsharma/PycharmProjects/session-plugin-polish` (from `main` at `ffb9ada`). Work only there.

## Global Constraints

- Everything from sub-projects 1–7 still applies:
  - clippy pedantic `-D warnings`;
  - hooks ≤ 50 ms p95, never fail, no new git process in any hook;
  - tests never touch the real `~/.claude` / `~/.bridge-screen` and use the scrubbed git helpers;
  - commit per task ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`;
  - never `--no-verify`; chain shell steps with `&&`.
- **Schema v9** if a task needs new columns (Tasks 1, 4, 5, 9): one migration, added once in Task 1, extended by later tasks in the same version. Rebuild must give identical views (`tests/rebuild.rs`).
- **Model keeps codes, screens show words.** Tool inputs and the API keep machine values (`requested`, `withdrawn`). Every screen shows words from `words.rs` / `words.ts` only.
- **Owner-facing wording** follows the lead-briefing rules in `CLAUDE.md`: effect first, plain words, no raw codes, say what to do.

## Review Focus

1. **Asked-for work never raises an alarm:** no amber/red, no drift question, no needs-you item, and no effect on threshold tuning. Real drift right after it still turns red. Test: `requested_work_is_not_drift` (Task 1).
2. **Roll-up is derived, never stored:** a milestone with all tasks finished reads "done"; adding a new task makes it read "in progress" again. Rebuild is unchanged. Test: `parent_status_rolls_up` (Task 3).
3. **Withdrawing a sign-off restores the earlier state exactly:** the previous level, or claimed, or to-do/active. Withdrawing twice changes nothing ("nothing to withdraw"). The forecast history is not rewritten. Test: `withdraw_restores_previous_status` (Task 5).
4. **The branch read stays inside the hook budget,** works in linked worktrees (where `.git` is a file) and with a detached HEAD, and records an event only when the branch actually changed. Test: `stop_refreshes_branch` (Task 4).
5. **No raw codes on any screen:** a shared jargon list is checked against every view-model string, `status`, `watch` and the web build. Test: `no_jargon_on_screens` (Task 7).

## File Structure (expected)

- `crates/bridge-screen-core/src/tangent/mod.rs`, `tolerance.rs`, `project.rs`; `navigator.rs`; `tuning.rs`; `recovery.rs`; `view/mod.rs`, `view/drill.rs` — the `requested` label (Task 1)
- `crates/bridge-screen-core/src/error.rs`; `crates/bridge-screen/src/mcp.rs` — the upgrade message (Task 2)
- `crates/bridge-screen-core/src/plan/query.rs` (or a new `plan/rollup.rs`), `plan/render.rs`, `view/*` — roll-up (Task 3)
- `crates/bridge-screen/src/hook/*` (Stop), `crates/bridge-screen-core/src/session/project.rs` — branch (Task 4)
- `crates/bridge-screen-core/src/verify/*`, `owner.rs`, `crates/bridge-screen/src/commands.rs` — withdraw (Task 5)
- `web/src/card/Card.svelte`, `web/src/lib/*` — card form, keyboard tooltip (Task 6)
- `crates/bridge-screen/src/words.rs`, `web/src/lib/words.ts`, `plugin/skills/bridge-screen/SKILL.md`, `docs/lead/message-audit.md` — wording (Task 7)
- `web/package-lock.json` (Task 8)
- `crates/bridge-screen-core/src/telemetry/read.rs` — replaced transcript (Task 9)

---

### Task 1: "Asked for" — work the owner requested is not drift

**Why:** three times in one session, work the owner asked for (a licence, docs) was flagged as drift and turned the session red. The only options today are on-task, detour (blocks the task) and drift. None of them fits "not this task, but the owner asked for it".

**Model:**

- Add `Label::Requested`.
  - Stored value: `requested`; the parse aliases `requested`, `asked`, `asked-for`.
  - Display in words: "Asked for".
- Add to `start_detour`: `requested_by_owner: bool` (default false).
  - Stored on the detour (schema v9: a `detours.requested` column, from the `detour_opened` event data, default false for old events).
- Accept `requested` in `label_unit`, in the CLI `bridge-screen label … requested` and in `owner::act` label.

**Behaviour (pseudocode):**

```
Stop labelling (navigator):
  if unit already labelled -> keep it
  if unit.detour_id and detour.requested -> label Requested, source Declared
  else existing rules (Detour / heuristic / ask)

label_left_unlabeled: same first rule for requested detours

tolerance::evaluate:
  away time from an open detour counts only when the detour is NOT requested
  last_unit_drift: Requested is never drift (only Label::Drift counts, as today)

tuning: skip Requested units entirely (they are neither on-task nor drift evidence)
recovery: drift flag false for Requested
glance: "drifting" false for Requested; the needs-you queue never holds an item for it
drill (session/history): show "Asked for" with its source, like the other labels
the drift question text (DRIFT_REQUEST / LABEL_REQUEST): add the option
  "requested — the owner asked for this in this session"
```

**Tests:**
- `requested_work_is_not_drift`:
  - a requested detour that runs past 2× tolerance stays green;
  - a unit labelled `requested` with a high drift score gives no red and no drift question;
  - the next unit, real drift, still turns red.
- Tuning ignores requested units.
- Rebuild equality with requested detours.
- The CLI `label … requested` works.
- The MCP `label_unit` and `start_detour` schemas list the new values.

### Task 2: An upgrade tells open sessions what to do

**Why:** after an upgrade, sessions that were already open lose their bridge-screen tools with "database schema version 8 is newer than this bridge-screen supports (7); upgrade bridge-screen". The installed copy *is* upgraded. Only the session's old background server is behind, and the fix is to start a new session.

**Behaviour:**

- Change the `SchemaTooNew` message to: "bridge-screen was updated while this session was open. Start a new Claude session to keep using it (data format {found}; this session's copy understands up to {supported})."
- In `mcp.rs`, when opening the store fails with `SchemaTooNew`, every tool returns that message as its result text, readable by the agent, instead of a protocol error.
- Hooks: the error is logged once per session, not on every hook. Note that hook processes start fresh and normally pick up the new binary.

**Tests:**
- The store test checks the new wording.
- An MCP test with a store at version 99 gets the message from `status`.

### Task 3: Milestones and the goal show their real status

**Why:** finished milestones and the goal still show "to do", because only tasks get a status. Parents never do.

**Rule (derived at read time, never written to `plan_nodes`):**

```
rollup(node):
  children = non-dropped child tasks (for the goal: all tasks, via milestones)
  if no children -> node's own status
  if any child failed -> "failed" (words: "needs attention")
  if all children done (same "done" test as done%)  -> "done"
  if any child active/claimed/done or verified at any level -> "in progress"
  else -> "to do"
```

**Behaviour:**

- One function in core (`plan::rollup` or similar).
- Use it everywhere a parent's status is shown:
  - `plan show` (render);
  - the glance/drill view models;
  - `status`, `watch`, the card;
  - the MCP `status` tool.
- A milestone that reads "done" shows no hill word (`words::hill` already hides it at 100%; check the dashboard too).

**Tests:**
- `parent_status_rolls_up`:
  - all tasks at L4 gives "done";
  - adding a task gives "in progress";
  - a failed task gives "needs attention";
  - a dropped task is ignored.
- Rebuild is unchanged.

### Task 4: Branch names stay current

**Why:** a session's branch is recorded only when the session starts. Sessions that switch branches show the old name.

**Behaviour (Stop hook, file reads only, no git process):**

```
read_head(cwd):
  walk up from cwd to find ".git" (stop at repo root already known to the session)
  if .git is a file "gitdir: <path>" -> head = <path>/HEAD   (linked worktree)
  else head = .git/HEAD
  content "ref: refs/heads/<name>" -> Some(name)
  detached (a SHA)                 -> Some("detached at <first 7>")
  unreadable                       -> None (do nothing)

at Stop: if read_head(session.cwd) != session.branch -> record `branch_changed {branch}`
projector: sessions.branch = new value
```

**Tests:**
- `stop_refreshes_branch`:
  - in a normal repo;
  - in a linked worktree;
  - with a detached HEAD;
  - an unchanged branch records no event.
- The Stop latency test still passes.

### Task 5: Taking back a sign-off

**Why:** there is no way to undo the owner's verdict. `--broken` marks a task failed, which is a different claim.

**Model:**

- New event `verification_withdrawn {task_id, verification_id}`, owner only.
- The projector sets that verification row's `state = 'withdrawn'`.

**Behaviour:**

```
bridge-screen verify <task> --withdraw [-C DIR]   (and owner::act "withdraw" for screens)
  target = the owner's newest current verdict on the task (✓ at L4, or the ✗ failure)
  none -> Stale("nothing to withdraw on <task>")
  record verification_withdrawn
  refresh_status recomputes: highest remaining current level, else claimed,
     else back to "active" if a live session holds the claim, else "todo"
     (refresh_status today leaves the status alone in that last case — fix it)
  the forecast's history is not rewritten; the next forecast run reflects it
```

- Dashboard task drill: a "Take back sign-off" button next to the owner's ✓/✗, through `owner::act`, with the stale-click check.

**Tests:**
- `withdraw_restores_previous_status`:
  - L4 over L2 goes back to L2;
  - L4 alone with a claim goes to "claimed";
  - nothing at all goes to "todo";
  - a second withdraw is stale.
- Rebuild equality.
- Owner-only: the MCP has no way to do it.

### Task 6: Desktop card and dashboard touch-ups

- **Card "Make it a task":**
  - the parent/title inputs are hidden until the owner clicks "Make it a task";
  - after the click, they show on their own full-width row with "Create task" and "Cancel";
  - placeholders are short and never clipped: "Under (e.g. 10)" and "Task title";
  - same for the dashboard if it shares the pattern.
- **Hill tooltip by keyboard:** the "exploring / executing" chip is focusable, and its tooltip shows on focus as well as hover, with `aria-describedby`.
- **Scope summary:** several additions made in the same minute show as one line ("3 tasks added"), not one line each.
- **Tests:**
  - vitest for the card form state;
  - a Playwright check that the tooltip opens with Tab;
  - a summary grouping unit test.

### Task 7: Plain words everywhere (message audit)

**Why:** bridge-screen reports in its own terms ("checkpoint missed", "readback pending", "L1", "drift red"). The owner leads but doesn't read code.

**Steps:**

1. **Audit:** list every owner-facing string in `docs/lead/message-audit.md`, as a table: where it appears / today's text / new text. Owner-facing strings are:
   - needs-you titles, details and actions;
   - glance/drill text;
   - `status`, `watch`, the card, the report;
   - CLI outputs and errors the owner sees;
   - the briefing the agent relays.

   Agent-only hook context is out of scope, except where the agent is told to repeat it to the owner.
2. **Rewrite** each one by the lead-briefing rules:
   - effect first;
   - plain words;
   - say what to do;
   - a glossary term only if it's in `docs/lead/glossary.md`.

   Examples:
   - "verify overdue" becomes "Task 4.2 was claimed done 2 days ago and nobody has checked it. Check it, or ask the agent to."
   - "L1" becomes "tests passed on the agent's machine".
   - "readback pending" becomes "the new session hasn't confirmed it understood the handoff yet".
3. **One source of truth:** move the words into `words.rs` / `words.ts` where they aren't already.
4. **Plugin skill:** update `plugin/skills/bridge-screen/SKILL.md` so agents relay bridge-screen to the owner in the lead-briefing shape:
   - effect first;
   - state;
   - the ask;
   - checked vs believed;
   - details last.

   Also tell agents when to use `requested_by_owner` / the `requested` label (Task 1).
5. **Test `no_jargon_on_screens`:** one shared list of codes (`L0`–`L4`, `drift_red`, `readback`, `checkpoint`, `on_task`, `uphill`, `downhill`, `verify_overdue`, raw ids such as session UUIDs, …). Check it against:
   - the view-model strings in fixtures;
   - `status`;
   - `watch` (as rendered text);
   - the built web bundle's visible strings, extending the existing "no raw codes" tests.

### Task 8: Build-time security warning

- `npm --prefix web audit fix` to update `braces` (GHSA-vfj7-8cjw-p6xm), which comes in via micromatch from `vite-plugin-singlefile`. No `--force`.
- If a fix needs a major bump, stop and report instead.
- `npm --prefix web run build` and the web tests pass. `npm audit` shows no high findings.

### Task 9: A replaced transcript is noticed

**Why (SP7 known gap):** a transcript replaced by a file of equal or larger size keeps its old byte cursor, so the new content is skipped.

**Behaviour:**

- Store the file's identity with the cursor: inode on Unix, plus its first-line hash (first ≤ 256 bytes).
- At read time, if the identity differs, restart from 0. Dedupe by message id already prevents double counting.
- The identity comes in the `usage` event data, so the cursor view rebuilds.

**Test:** `replaced_transcript_restarts`.

### Task 10: Docs and living docs

- `CLAUDE.md`:
  - the new label and `--withdraw` in Commands;
  - `message-audit.md` in the `docs/lead` list.
- Spec:
  - §7.1: the requested label;
  - §8.5: withdraw;
  - §10: roll-up.
- `README.md` if commands are listed there.
- `docs/lead/`:
  - `glossary.md`: "Asked for", "Take back sign-off";
  - `decision-log.md`: requested label rather than auto-detecting; roll-up derived rather than stored; withdraw is owner-only;
  - `project-map.md` if a part changed job.
  - Leave `current-state.md` for after the merge.
- Run the full suite:
  - `cargo test --workspace`;
  - clippy;
  - `npm run lint/check/test/build`;
  - e2e;
  - the hook latency test (release).

## Not in this round

- SP8 items (plan import, `init`, branch groups, `waiting_on`).
- Auto-detecting owner requests from prompt text. Prompts are never stored. The agent declares it, and the owner can relabel.
