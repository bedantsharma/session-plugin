# User-reported bugs (friends' first runs) Implementation Plan

**Goal:** A friend can drop the plugin into a repo whose plans and specs live anywhere, see correct progress, start a new run when one finishes, and read the History panel without decoding JSON.

**Owner decisions (2026-10-26):**
- Finished plan + new plan file → close the old run, start a fresh one (progress 0%, numbers from 1).
- Plan files found but nothing says what is finished → ask the owner once ("Needs you").

Read spec §12.1, §10, §5.3 first. Use `gitnexus_impact` before editing any symbol; run `gitnexus_detect_changes` before committing. Add dependencies only with `cargo add` / `npm install`. Hooks stay ≤ 50 ms.

## Task 1: Find plans and specs in any folder

**Files:** `crates/bridge-screen-core/src/import/mod.rs` (`plan_files`, `PLANS_DIR`), `init.rs` (spec listing), `plugin/skills/bridge-screen/SKILL.md`, `crates/bridge-screen/src/cli.rs` help text.

Behaviour:
- `plan_files(root)` stops reading one fixed folder. It walks the checkout with a hard bound (depth 8, 3000 entries, skip `.git`, `node_modules`, `target`, `dist`, `build`, `vendor`, `.venv`, any dot-folder except `.github`-style ones holding no plans) and returns every `*.md` that looks like a plan: a folder or file name containing `plan` (case-insensitive), or `roadmap`, `todo`, `tasks`. A sibling `specs`/`spec` finder returns spec files the same way (used by `init` evidence and listed as references).
- The existing parser stays the gate: a matched file with no `### Task N:` headings is already `Skipped`, so false matches cost one read, never a wrong import. Keep the SessionStart budget (20 files, 1 MiB, stat first); order results with `docs/superpowers/plans` first, then shortest path, so the old layout behaves as before.
- Keep `PLANS_DIR` as the preferred-first hint, not the only place.

Tests (describe in words): plans under `planning/`, `notes/plans/`, `docs/design/plan-x.md`, nested three levels deep are all found; `node_modules/**/plan.md` is not; a repo with 5000 files stops at the bound; old `docs/superpowers/plans` repo gives the same outcomes and order as before; a README that contains "plan" in its text is not matched by name alone.

## Task 2: Ask when nobody says what is finished

Behaviour:
- At SessionStart, when `scan` finds plan files in a repo with no import history and no plan, and none has a ticked step, do not import silently. Record a pending question (marker like the project pages offer: no new event kinds if avoidable; otherwise one event `plan_scope_asked` / `plan_scope_answered`, projected into a view).
- It appears on the glance "Needs you" queue in plain words: "Found N plan files (names). None say what is done. Count progress from now, or tell me which are already finished?" Answers: **Fresh run** (import all as not started, baseline nothing) and **Some are done** (owner lists files; those are baselined as hand-planned, the rest imported). Offer both in the dashboard card, CLI (`bridge-screen import --fresh` / `--done <file>…`) and the agent briefing note (agent may relay the owner's answer through the existing owner-only paths, never decide itself).
- Until answered, nothing is imported and the briefing note says so once per session. Files with ticked steps keep today's behaviour (the ticks are the status).

Tests: question raised once; not raised again after an answer; fresh run imports everything at 0%; "some are done" baselines the named files; agent cannot answer for the owner; hook stays within the latency budget.

## Task 3: Runs — close a finished plan, start a fresh one

**Files:** plan projector/views (`plan/`, `view/glance*`, `view/drill.rs`, `verify/mod.rs` done%), `import/mod.rs` (`target_for`), `web/src/lib/glance`, `web/src/lib/drill/Project.svelte`, `web/src/lib/words.ts`.

Model (keep the single goal `G`; do not make goals plural):
- A **run** is the set of milestones created after the previous run boundary. New event `run_closed {at_milestone_next, closed_ids, closed_at}` records a boundary; no node is renamed, so every old id and event stays valid.
- Close happens when the goal's roll-up is Done (every task verified L2+) and either a new plan file/milestone arrives (automatic, noted in the briefing) or the owner presses "Close this run" on the project page (CLI `bridge-screen run close`). Never closes with unverified tasks; says why.
- Reads: done%, claimed%, milestone list, scope chart, forecast and burn-up use only the current run. Closed runs show under a "Completed runs" section on the project page (title, finished date, task count, 100%, link to its tasks, still readable).
- Numbers shown to the owner restart per run: a run's milestone `k` displays as `k − first_milestone_of_run + 1`; task numbers inside a milestone already start at 1. Raw ids (`3.1`) stay what agents and the CLI use; show the raw id in small muted text where an agent might need it. Anywhere that prints an id to the owner uses one shared helper.
- When a run completes with nothing new yet, the glance row shows "Run finished — 100%, N tasks" in the done colour and sits below active projects; it does not show as stalled or needs-you.

Tests: closing is refused with an unverified task; after close and a new plan, the project reads 0%, display numbers start at 1, old run readable at 100%; forecast ignores closed-run tasks except calibration history (keep calibration across runs); replaying events rebuilds the same views; projectors use event timestamps only.

## Task 4: History in plain sentences

**Files:** `web/src/lib/drill/History.svelte`, new `web/src/lib/drill/historyText.ts`, `crates/bridge-screen-core/src/view/drill.rs` (add a `summary` string per row, built in core so CLI/dashboard agree), `web/src/lib/jargon.json`, `docs/lead/message-audit.md`.

Behaviour: each row shows time, a plain sentence ("Task 4 claimed by session a1b2c3", "Task 4 passed its tests", "Plan edited: added 3 tasks"), and a "details" toggle that reveals the raw JSON (no longer the default). Unknown kinds fall back to the kind name in words. Replace the free-text kind filter with a dropdown of kinds present, and show the task title next to its id. Codes in `jargon.json` must not reach the sentence text.

Tests: a sentence per common kind; unknown kind falls back; raw data hidden until toggled; vitest and Playwright smoke for the panel.

## Task 5: Docs and checks

Update `docs/lead/` (current-state, decision-log entries for runs and the scope question, glossary: run, baseline, Needs you; project-map; message-audit rows for every new owner-facing message), CLAUDE.md layout lines, spec §10/§12.1 notes. Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and in `web/`: `npm run lint`, `npm run check`, `npm run test`, `npm run build`. Do not use `--no-verify`.
