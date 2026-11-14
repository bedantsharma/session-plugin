# Tracks, hiding, removing, no-test tasks and Back — Implementation Plan

> **For the implementer:** work the tasks in order, test-first. This plan gives files, behaviour (pseudocode) and test names, never full code. Read the cited spec parts and the existing code first and follow its patterns: `plan_edit_in_sourced` and `validate` (`plan/ops.rs`), the projectors (`plan/project.rs`, `projection.rs`), `Runs` (`plan/runs.rs`), `owner::act` (`owner.rs`), `glance` (`view/mod.rs`), `useModel` + `api.ts` (`web/src/lib`). Add dependencies only with `cargo add` / `npm install` (none expected).

**Goal:** One folder can hold several tracks with their own bars; work that cannot be tested can still be finished; the owner can hide cards, bars and projects and remove abandoned work for good; Back works in the dashboard; plan import stops creating step noise.

**Spec:** `docs/superpowers/specs/2026-11-14-tracks-hide-review-design.md` (read it first, then spec §5.2, §8.5, §10, §12.1 of `2026-10-13-bridge-screen-design.md`).

**Branch:** `feat/tracks-hide-review` (checked out).

## Global Constraints

- Clippy pedantic `-D warnings`; `cargo fmt --all`; web `lint`, `check`, `test`, `build` pass. Never `--no-verify`.
- Event-sourced: new behaviour = new events (or new optional fields) + projectors using event timestamps and positions only. `bridge-screen rebuild` must reproduce every view; new view tables go into `VIEW_TABLES`.
- Old data keeps working: missing `check` = tests, missing `track` = 1, unprefixed ids = track 1, old `node_removed` = not by the owner. Old ids never change.
- Hooks ≤ 50 ms p95 and never fail; no new git process; hides are read only by the glance.
- Nothing is written inside a repo. Tests use temp homes and a fake `claude`.
- Owner actions go through `owner::act` with the stale-click check. Agents cannot hide, remove, close tracks or answer for the owner; the card's one-time code never offers Hide, Remove or Close.
- Owner-facing wording: effect first, plain words; every new message goes into `docs/lead/message-audit.md` and passes `web/src/lib/jargon.json`.
- Before editing a symbol run `gitnexus_impact` (if the tool is unavailable, read callers with grep and say so in the commit); run `gitnexus_detect_changes` before each commit.
- Commit per task with the attribution line the session gives; chain shell steps with `&&`.

## Review Focus (what the reviewer checks first)

1. **Old ids and old databases:** a V10 database with a plan, runs, forecasts and verifications opens and reads identically. Tests: `v10_database_migrates` (Task 1), `old_ids_are_track_one` (Task 5).
2. **No "done" test is written twice:** one helper decides; grep finds no stray `L2 | L3 | L4`. Test: `done_helper_is_the_only_rule` (Task 1).
3. **A review task cannot be made done by an agent that dodges the owner:** guards on switching. Tests in Task 3.
4. **Hidden never changes numbers and never hides something new.** Tests in Task 9.
5. **Remove for good is not undone by a plan file** and leaves no nudge, wait or session pointing at it. Tests in Task 8.
6. **Rebuild reproduces all views** with every new event kind. Test: `rebuild_reproduces_new_views` (Task 9, extended in 10).

## File Structure (expected; "core" = `crates/bridge-screen-core/src`)

- core: `store/schema.rs` (V11), `plan/model.rs`, `plan/track.rs` (new), `plan/ops.rs`, `plan/project.rs`, `plan/runs.rs`, `plan/rollup.rs`, `verify/mod.rs`, `verify/ladder.rs`, `import/{parse,diff,mod,project,scope}.rs`, `claim.rs`, `init.rs`, `forecast/`, `telemetry/mod.rs`, `waits/`, `hide/mod.rs` (new), `owner.rs`, `view/{mod,drill,groups,history_text}.rs`, `handoff/briefing.rs`, `projection.rs`
- binary: `src/mcp.rs`, `src/mcp/tracks.rs` (new), `src/cli.rs`, `src/commands.rs`, `src/onboard.rs`, `src/hook/{context,handlers,handoff}.rs`, `src/status.rs`, `src/watch.rs`, `src/report.rs`, `src/usage.rs`, `src/ui/api.rs`, `src/words.rs`
- web: `src/App.svelte`, `src/lib/{api,types,words,route}.ts`, `src/lib/glance/{ProjectRow,TrackBar,NeedsYou,Header,HiddenDrawer}.svelte`, `src/lib/drill/{Project,Task,History}.svelte`, `src/lib/ui/ConfirmRemove.svelte`, `tests/`, `e2e/`
- plugin: `plugin/skills/bridge-screen/SKILL.md`; docs: `docs/lead/*`, `CLAUDE.md` layout, spec notes

---

### Task 1: Foundations — one "done" rule and schema version 11

**Files:** core `plan/model.rs` (helper), the ~12 places that test L2/L3/L4 by hand (`forecast/mod.rs:176`, `view/mod.rs` ×2, `view/drill.rs` ×3, `view/groups.rs`, `import/diff.rs`, `recovery.rs`, `verify/{mod,ladder}.rs`, `handoff/briefing.rs`, `plan/rollup.rs`), `store/schema.rs`, `store/mod.rs`, `projection.rs`.

Behaviour:
- Add to `NodeStatus`: `counts_as_done()` (L2, L3, L4, and — added in Task 3 — Reviewed), `is_claimed_not_done()` (Claimed, L1), `has_work()`. Replace every hand-written match with them. The SQL in `verify::progress` uses a shared const list of done status names.
- Schema V11 (`SCHEMA_VERSION = 11`): `ALTER TABLE plan_nodes ADD COLUMN check_kind TEXT NOT NULL DEFAULT 'tests'`; `ALTER TABLE plan_source_items ADD COLUMN owner_removed INTEGER NOT NULL DEFAULT 0`; `ALTER TABLE forecast_history ADD COLUMN track INTEGER NOT NULL DEFAULT 1`; `CREATE TABLE hides (repo_id, subject, kind, label, stamp, hidden_pos, hidden_at, PRIMARY KEY (repo_id, subject))`; add `hides` to `VIEW_TABLES`. Wire `if current < 11` in `migrate`.

Tests first: `done_helper_is_the_only_rule` (behaviour per status, and a grep-style test that no source file outside `model.rs` spells `L2 | L3 | L4`); `v10_database_migrates` (build a V10 database by applying `V1..V10` and appending recorded events for a plan, a run close, a forecast and a verification; open with the new code; every row and every read model is unchanged and the new columns hold their defaults); `rebuild_after_migration_matches`.

Steps: write tests → run, expect FAIL → implement → `cargo test -p bridge-screen-core` → commit `refactor(core): one done rule; schema v11`.

### Task 2: Plan import — step checkboxes are not tasks

**Files:** `import/parse.rs`, `import/diff.rs`, `import/mod.rs`, `import/project.rs`, `import/scope.rs`, tests in `import/tests.rs`.

Behaviour:
- Parser still reads steps (ticks matter) but `ParsedTask` gains `ticks: None | Some | All` computed from them.
- `diff`: a parsed task produces no step nodes, ever. For an existing task, if `ticks == All` and the task is not done → `claims.push(task)` (evidence "all steps ticked in <file>"); `Some`/`None` change nothing. New-task path likewise.
- Retire the noise: every `prev` item with `step_no` set yields `RemoveNode` (importer actor, reason "step checkboxes are no longer imported as tasks") **even when claimed** (their ticks fold into the parent); keep it only when it has L2+ (then list it in `keeps` with a plain note). Counted in a new `steps_removed`, excluded from `changed_ratio`/`replan`, listed in the dry run and in the import summary ("dropped 45 step items").
- `scan` at SessionStart: a seen file whose recorded `items` include steps counts as changed once, so it is re-imported (budget unchanged: stat first, 20 files, 1 MiB).
- `scope.rs`: "has a ticked step" is unchanged.
- Projector: `node_removed` deletes the `plan_source_items` link rows of removed step nodes.

Tests first: `steps_are_not_nodes` (the superpowers template, 5 steps → one task, zero subtasks); `all_steps_ticked_claims_task`; `some_steps_ticked_changes_nothing`; `old_step_nodes_dropped_on_next_import` (build the milestone-12 shape: 9 tasks × 5 step subtasks, some claimed; re-import → all step nodes dropped, parents kept, ticks folded into parents' claims); `verified_step_is_kept_and_named`; `step_removal_does_not_trigger_replan`; `unchanged_file_with_steps_reimported_once`; `scope_question_still_sees_ticks`; `reimport_twice_is_quiet` (second import writes nothing). Include a test that parses every file in `docs/superpowers/plans` and asserts no subtasks.

Commit `fix(import): step checkboxes are not tasks; drop old step noise`.

### Task 3: No-test tasks — core

**Files:** `plan/model.rs` (`CheckKind {Tests, Review}`, `NodeStatus::Reviewed`, `PlanNode.check`), `plan/query.rs`, `plan/ops.rs`, `plan/project.rs`, `verify/mod.rs` (`refresh_status`, `progress`), `verify/ladder.rs`, `plan/rollup.rs`, `claim.rs`, `init.rs`, `view/groups.rs`, `forecast/mod.rs`.

Behaviour:
- `check` optional on `PlanOp::AddNode` and `EditNode` (schemas via `schemars`; `deny_unknown_fields` stays), on `ClaimTarget::New`, and on `init_plan` task drafts. `NodeAdded`/`NodeEdited` carry it; projector writes `check_kind`; `get_node`/`load_tree` read it.
- Switch guard (in `validate` for `EditNode`): to `review` needs a reason in `EditMeta`; refused when the node's newest owner verdict is "broken" and in force, or its status is L2+ ("it already passed tests"). Switch back to `tests` allowed; status re-derived.
- `refresh_status(tx, repo, task, failed)`: order failed → `l2..l4` → (review and `saved`) `reviewed` → `l1` → claimed → active → todo. `saved(task)` = newest `claims_done` row: files non-empty and a current L1 verification exists; or files empty and evidence ≥ 10 words and ≥ 60 chars. Call `refresh_status` after `claimed_done` and after the L1 `verified` event (already the path for status).
- `progress()` returns `reviewed_tasks`; reviewed minutes count as done. Rewrite the SQL as Rust over the track's leaf nodes (needed in Task 5 anyway): leaves = live tasks and subtasks with no live children.
- `verify` MCP/ladder: refuse L2–L4 on a review task with the plain message; nudges and staleness skip `reviewed`.
- Roll-up: `Reviewed` is done. Forecast: reviewed leaves are finished (remaining excludes them; calibration uses their real time).

Tests first: `review_task_done_when_file_committed` (claim_done with files + L1 pass → `reviewed`, done% moves, no owner step); `review_task_done_with_long_summary_and_no_files`; `short_summary_stays_claimed`; `files_named_but_uncommitted_stays_claimed_until_commit`; `missing_file_does_not_count`; `owner_broken_takes_review_task_back` (→ failed, done% drops, claim_done again → reviewed); `owner_works_beats_reviewed` (→ l4); `switch_to_review_needs_reason`; `switch_refused_after_owner_broken`; `switch_refused_when_already_l2`; `verify_refused_on_review_task`; `no_nudge_for_reviewed`; `reviewed_never_stale`; `old_events_read_as_tests`; `claim_task_new_task_accepts_check`; `init_plan_accepts_check`; `rollup_counts_reviewed_as_done`; `progress_counts_reviewed_tasks_apart`.

Commit `feat(core): review tasks — done without tests`.

### Task 4: No-test tasks — MCP, owner reach, wording

**Files:** `src/mcp.rs` (tool schemas, replies), `core/view/drill.rs` (task and project drill actions), `core/view/history_text.rs`, `src/words.rs`, `core/owner.rs` (no new action: the existing `Verify`/`Broken`).

Behaviour:
- `plan_edit`, `claim_task`, `init_plan` tool docs mention `check` ("`review` for research, planning, docs and similar work no test can check; done once the result is saved"). `claim_done` reply for a review task says whether the result counts as saved or what is missing ("commit <files>, or add a fuller summary").
- Task drill and each project-drill task row carry `actions: [✓ Works, ✗ Broken]` for tasks that are claimed, `l1`, `l2`, `l3` or `reviewed` (not failed, not owner-L4); `Broken` asks for a note. Same `OwnerAction`s the nudge card uses, so stale clicks are checked already.
- Wording: status `reviewed` reads "done — no tests" in `words.rs`/`history_text.rs`; History sentences for the check switch ("Task 2.1 changed to: no tests possible — <reason>") and for a review task saved.

Tests first: `task_drill_offers_verify_actions_for_claimed`; `no_verify_actions_for_todo_or_owner_l4`; `broken_from_project_page_marks_failed`; `reviewed_words_are_plain` (against the forbidden-code list); `history_sentence_for_check_switch`; MCP test `plan_edit_check_roundtrip`.

Commit `feat: owner can verify any claimed task; review tasks in words`.

### Task 5: Tracks — ids, the module, per-track runs and progress

**Files:** `plan/track.rs` (new), `plan/runs.rs`, `plan/ops.rs` (`assign_id`), `plan/query.rs`, `plan/rollup.rs`, `verify/mod.rs` (`progress`), `view/drill.rs` and `view/mod.rs` call sites, tests.

Behaviour (pseudocode):
```
track_of(id): if id starts "T<digits>-" (case-insensitive) -> that number else 1
local(id):    id without the prefix;  raw(track, local): track==1 ? local : "T{track}-{local}"
goal_id(1) = "G";  goal_id(k) = "T{k}-G"
tracks(conn, repo): goal nodes (kind goal, parent NULL, not dropped) -> Track{no, goal_id, title, intent, state}
                    state = closed if newest of track_closed/track_reopened for that track is closed
next_track_no(conn, repo): max over ALL goal rows incl. dropped, +1 (1 if none)
```
- `runs::load(conn, repo, track)`: filters `run_closed` by `data.track` (missing = 1); `top_number` reads `local(id)`; `shown(id)` returns the local id restarted per run and track; `RunClosed` gets optional `track`; `close_in/close_if_finished/state/finished/run_tree` take a track; boundary = the track goal's `next_child`; `closed_ids` = children of the track's goal.
- `assign_id`: for a goal: no goal row ever → `G`; an open track exists → `GoalExists` error listing the open tracks ("this folder already has track 1 'Ship it'; to start another, ask the owner and use start_track"); no open track but goals existed → next number. Children ids: under goal `T2-G` → `T2-1`; deeper `T2-1.1` (existing `format!("{parent}.{n}")` already works). `plan_edit` into a closed track is refused with the reason.
- `verify::progress(conn, repo, track)` and a pooled `progress_all`; replace any SQL that parses the first number of an id.
- Callers keep working: wrap `track` = 1 where a single plan is assumed until Tasks 6–7 route properly (each such call site is listed in the commit message and removed by Task 7).

Tests first: `old_ids_are_track_one` (a repo with `G`, `1`, `1.1`: tracks() lists one, `shown`, progress, `run_closed` all as before); `prefix_grammar` (`T2-1.3` → 2 / `1.3`; `t2-G`; garbage ids; `T-1`, `T0-1` not tracks); `track_numbers_never_reused` (remove track 1, start another → `T2-…`; remove it, next is `T3`); `second_goal_refused_while_track_open`; `goal_allowed_after_all_closed`; `runs_are_per_track` (closing a run in track 2 leaves track 1's current run alone; display numbers restart per track); `progress_per_track_and_pooled`; `plan_edit_into_closed_track_refused`; `ids_in_track_two_start_at_one`; `run_closed_without_track_is_track_one`.

Commit `feat(core): tracks — ids, per-track runs and progress`.

### Task 6: Tracks — start, close, reopen (core, owner action, MCP, CLI)

**Files:** `plan/track.rs` (`start_track_in`, `close_track_in`, `reopen_track_in`), `plan/project.rs` (kinds list), `owner.rs` (`StartTrack`, `CloseTrack`, `ReopenTrack`; `CloseRun` gains optional `track`), `src/mcp/tracks.rs` (`start_track`), `src/mcp.rs`, `src/cli.rs`, `src/onboard.rs` or `commands.rs`.

Behaviour:
- `start_track_in(tx, ctx, title, intent)` appends the goal `node_added` (internal; not a `PlanOp` variant, so not in the `plan_edit` schema). Returns the goal id.
- MCP `start_track {title, intent, owner_said}`: `owner_said` required (same free-text check as `init_plan`) when an open track exists; free when none. Reply names the new goal id and the first ids to use.
- `track_closed {track}` / `track_reopened {track}` events (owner session); `close_track_in` refuses while any leaf is neither done nor removed, naming up to three ("can't finish track 'Docs refresh' yet: 2 of 7 tasks aren't done (…). If the rest was abandoned, remove it first; nothing was changed."). Not in `PLAN_KINDS`' projection (read from events); add the kinds to the history texts.
- Owner actions through `act`; stale when the track is already closed/open. `CloseRun {repo, track?}`: the track is optional when exactly one is open, otherwise refused with the list.
- CLI: `bridge-screen track list | start "<goal>" [--intent T] | close N | reopen N [-C DIR]`, `bridge-screen run close [--track N]`.

Tests first: `start_track_needs_owner_words_when_open`; `start_track_free_when_none_open`; `agent_cannot_start_track_with_plan_edit`; `start_track_refused_for_blank_words`; `close_track_refused_with_open_tasks_names_them`; `close_then_reopen`; `closed_track_leaves_open_list`; `close_run_needs_track_when_two_open`; `cli_track_start_creates_goal`; `track_events_replay_on_rebuild`.

Commit `feat: start, close and reopen tracks`.

### Task 7: Tracks — every reader picks a track

**Files:** `claim.rs` (`candidates`, task titles with track), `handoff/briefing.rs`, `hook/context.rs`, `hook/handlers.rs`, `src/mcp.rs` (`plan_status`, `status` tool, `plan_edit` errors), `init.rs`, `import/mod.rs` (`target_for`, `live_goal`, `has_plan`, `container_titled`, `scan`), `forecast/` (+ `forecast_history.track`, `forecast_ran.track`), `telemetry/mod.rs` and `src/usage.rs`, `plan/render.rs` and CLI `plan show`, `src/status.rs`.

Behaviour:
- Briefing: one "Tracks (n open): …" line block (≤ 5 listed, most recently active first by newest task event, "+k more"), the session's own track marked; stays ≤ 40 lines; no git. With one open track the text is today's. Rule text: if the work doesn't clearly belong to a listed track, ask the owner to continue one or start a new one.
- Claim reminder: candidates across open tracks, each with the track name when 2+ are open; no match and 2+ open → repeat the question.
- `plan_status` (MCP `status`) prints each open track's goal, milestones, forecast, next tasks.
- `init_plan`: refused when an open track exists (plain message pointing at `start_track`); otherwise the drafted goal becomes the next track number.
- Import target (spec §Behaviour 2): linked track → title-matched container in an open track → the single open track → else park as `needs_link` with a briefing line listing tracks; `bridge-screen import --track N`; a new goal only when no open track. A finished run in the chosen track closes that run only.
- Forecast per track: remaining minutes of the track, hours/day from the track's own task time, refusal rules unchanged and said per track; `forecast_ran` carries `track`; `forecast_history.track` projected; calibration repo-wide. CLI `forecast [--track N]`.
- Usage: `usage --by track`, `--track N` (track read from each reply's task id).
- `plan show` renders every open track under its goal with `--track N` to narrow.

Tests first: `briefing_lists_open_tracks_and_asks`; `briefing_unchanged_with_one_track`; `briefing_caps_track_list`; `claim_reminder_names_tracks`; `claim_reminder_asks_when_nothing_matches_and_two_tracks`; `init_plan_refused_with_open_track`; `import_single_track_goes_there`; `import_two_tracks_parks_file`; `import_track_flag_places_file`; `import_title_match_picks_track`; `forecast_per_track_uses_own_pace`; `forecast_refuses_track_without_recent_activity`; `forecast_history_has_track`; `usage_by_track`; `plan_show_track_flag`; hook latency tests extended: SessionStart with 5 open tracks and 20 plan files, UserPromptSubmit with 5 tracks (budget unchanged).

Commit `feat: sessions, import, forecast and usage understand tracks`.

### Task 8: Remove for good

**Files:** `plan/ops.rs` (cleanup after `RemoveNode`), `plan/project.rs` (`NodeRef.by_owner`, projector), `verify/nudge.rs` or its projector, `waits/`, `owner.rs` (`Remove`), `import/{diff,mod,project}.rs`, `view/history_text.rs`, `src/cli.rs`.

Behaviour:
- After a `RemoveNode` op (agent, owner or importer), in the same transaction: append `task_released` for every live session whose active task is in the subtree; `wait_cleared` (why `target_removed`) for open waits targeting it; the projector deletes the subtree's `verify_nudges` and `hides` of a removed track, and sets `plan_source_items.owner_removed = 1` when `by_owner`. `needs_owner` verify items vanish because `overdue()` already ignores dropped nodes (add a test, not code).
- `OwnerAction::Remove {repo, id, title, confirm}`: refuse without `confirm`; stale when the node is gone, dropped, or its title differs; goal id removes the whole track; calls the `RemoveNode` path with `by_owner`; reply in plain words ("Removed task 1.2 'Login form' (it was claimed, not checked). It has left the bars, the queue and the forecast.").
- Importer: skip items with `owner_removed`, and skip a file whose linked container was removed by the owner (said once in the summary); items retired by the importer itself are not marked and return if the file lists them again.
- CLI `bridge-screen remove <id> [--yes] [-C DIR]` (asks without `--yes`).
- History sentence "You removed …" with what it was and its state.

Tests first: `remove_drops_subtree_everywhere` (bars, run tree, forecast leaves, glance); `remove_clears_nudges_and_overdue_card`; `remove_clears_open_waits`; `remove_releases_live_session_task`; `remove_track_drops_all_its_nodes_and_bar`; `remove_needs_confirm`; `remove_stale_when_title_changed`; `finished_removed_task_still_teaches_calibration`; `plan_file_does_not_resurrect_owner_removed_task`; `importer_retired_task_returns_when_relisted`; `plan_file_of_removed_track_is_skipped_once`; `agent_remove_node_also_cleans_up`; `history_says_you_removed_it`; `cost_keeps_the_spend`.

Commit `feat: remove abandoned work for good`.

### Task 9: Hiding — events, stamps, actions

**Files:** `hide/mod.rs` (new: `HIDE_KINDS`, projector, `stamp_of_card`, `stamp_of_track`, `stamp_of_project`, `hidden_set`), `projection.rs`, `owner.rs` (`Hide`, `Unhide`), `view/mod.rs` (`glance` split, `Glance.hidden`, count/escalation from the visible set), `src/cli.rs`.

Behaviour (pseudocode):
```
stamp_of_card(card): per NeedsKind -> newest event position per the spec table, 0 if none
stamp_of_track(repo, n): max pos of {task_claimed, claimed_done, verified, verification_failed,
                         node_added, track_closed, run_closed} whose task/node is in the track
stamp_of_project(repo):  max(session_started pos in repo, every track stamp, every card stamp)
hidden(subject) iff hides row exists and current stamp == row.stamp
glance(): build everything as today; then drop hidden cards, track bars, projects into Glance.hidden;
          needs_you_count/escalated from visible cards; progress numbers untouched
act(Hide{repo, subject, stamp}): stale unless current stamp == stamp; append hidden{subject,label,stamp}
act(Unhide{repo, subject}):      stale unless a hides row exists; append unhidden{subject}
```
- `hidden` / `unhidden` projected into `hides` (event pos and ts only; `hidden` replaces the row).
- Each `NeedsYou`, `TrackRow`, `ProjectRow` carries its `subject` and current `stamp` so a click sends what it saw.
- Hide actions are not offered on the desktop card; the card's nonce path refuses `hide`, `unhide`, `remove`, `close_track` (check how `owner_action` validates today and add the refusal + test).
- CLI: `bridge-screen hidden` (list with ages), `hide <subject-words>`, `unhide <n>`.
- Map each name in the spec's stamp table to the real event kind; where one does not exist, use the closest raising event and note it in the test.

Tests first: `hide_card_then_quiet_stays_hidden`; `hidden_card_returns_on_new_question` (and one test per card kind from the table); `hidden_leftover_stays_hidden_when_another_resolves`; `hidden_track_returns_on_claim`; `hidden_track_stays_through_usage_and_time`; `hidden_project_returns_on_new_session`; `hidden_project_returns_when_a_card_inside_is_new`; `hide_stale_when_something_new_arrived`; `unhide_stale_when_not_hidden`; `hiding_never_changes_numbers` (done%, forecast, task counts, cost identical before/after); `count_and_escalation_follow_visible`; `hidden_ghost_ignored_when_subject_gone`; `agent_cannot_hide`; `card_nonce_refuses_destructive_actions`; `rebuild_reproduces_new_views` (a log with every new event kind; all tables equal after `rebuild`).

Commit `feat: hide cards, bars and projects until something new happens`.

### Task 10: Read models — glance, project page, status, watch, report

**Files:** `view/mod.rs` (`TrackRow`, `ProjectRow.tracks`), `view/drill.rs` (project drill per track, "Finished tracks", per-run sections inside each track, owner-facing ids), `view/groups.rs`, `src/status.rs`, `src/watch.rs`, `src/report.rs`, `src/words.rs`, `view/history_text.rs`, `web/src/lib/words.ts` (labels only).

Behaviour:
- `ProjectRow.tracks` as in spec; with one open track every old field equals it. A project with all tracks finished and none new reads as today's "Run finished" row. Sort: finished below active.
- Project drill: one section per open track (goal, bar, milestones, forecast), a "Finished tracks" list with reopen, "Completed runs" inside each track; every task row shows the display id, the raw id in muted text, the track name when 2+ tracks, "done — no tests" where it applies, and the row actions (verify, remove).
- `status` prints one line per open track under the project; `watch` and the HTML report likewise; the pooled figure feeds the first status line and the tab.
- History kinds: `hidden`, `unhidden`, `track_closed`, `track_reopened`, goal `node_added` ("Track started"), `node_removed` by owner, check switch: one line each in `history_text.rs`; every new kind and label is tested against the forbidden-code list.

Tests first: `glance_one_track_equals_old_shape`; `glance_two_tracks_two_bars`; `finished_track_leaves_glance`; `project_drill_sections_per_track`; `finished_tracks_listed_with_reopen`; `status_lists_tracks`; `report_lists_tracks`; `history_sentences_for_new_kinds`; `branch_groups_unchanged_with_tracks`.

Commit `feat(view): one bar per track in every screen model`.

### Task 11: Web — tracks, drawer, remove, review

**Files:** `web/src/lib/types.ts`, `api.ts` (act helpers), `glance/ProjectRow.svelte`, `glance/TrackBar.svelte` (new), `glance/NeedsYou.svelte` (Hide button, quiet), `glance/Header.svelte` ("3 need you · 2 hidden", Hidden button), `glance/HiddenDrawer.svelte` (new, right-side drawer, focus trap, Escape, "Bring back" per row, ages from the model), `drill/Project.svelte`, `drill/Task.svelte`, `ui/ConfirmRemove.svelte` (new), `words.ts`, `jargon.json`, vitest in `web/tests`, Playwright in `web/e2e`.

Behaviour: one `TrackBar` per open track (name, goal, big %, forecast, caption "includes 3 tasks done without tests", mini bars, "⋯" menu: Hide, Finish track, Remove for good); the project header shows no big % with 2+ tracks; Hide on cards, bars and projects; drawer lists hidden things grouped by kind; Remove opens the confirm dialog with the plain impact text from the server; ✓ Works / ✗ Broken buttons from the model's `actions`; reviewed tasks labelled "done — no tests". All text from the model or `words.ts`, no codes.

Tests first (vitest): `trackbar_renders_caption_for_review_tasks`; `projectrow_two_tracks_no_big_percent`; `projectrow_one_track_unchanged`; `hidden_drawer_lists_and_brings_back`; `drawer_escape_closes`; `confirm_remove_shows_impact_and_needs_click`; `remove_button_disabled_until_confirm`; `needsyou_hide_sends_stamp`; `stale_hide_shows_server_message`; `header_counts_visible_and_hidden`; `jargon_clean_for_new_text`. Playwright (`e2e/tracks.spec.ts`, seeded server): two bars; hide a bar → in drawer → bring back; hide card; remove a task through the dialog; ✓ Works from the project page.

Commit `feat(web): track bars, hidden drawer, remove confirm, review labels`.

### Task 12: Web — Back and swipe

**Files:** `web/src/lib/route.ts` (new), `web/src/App.svelte`, `web/src/lib/api.ts` (`takeToken`), tests, `e2e/back.spec.ts`.

Behaviour: `toHash`/`fromHash` for the eight screens; one `go(screen)` that pushes unless identical; `popstate` restores; first screen from the hash; "← Glance" uses `history.back()` when the previous entry is the dashboard's own (a counter in `history.state`) else replaces; `takeToken` keeps the route part when stripping `#token=`; unknown or gone targets show "That page is gone" and replace with the glance; every `onOpen*` callback goes through `go`.

Tests first (vitest): `route_roundtrip_all_screens`; `fromHash_rejects_garbage_to_glance`; `takeToken_keeps_route`; `takeToken_plain_token_still_works`; `go_does_not_push_twice`. Playwright: open project → open task → Back → Back lands on glance; reload on the project page stays; `#token=` first load then Back leaves nothing broken; page gone case.

Commit `feat(web): Back and swipe work inside the dashboard`.

### Task 13: Skill, CLI help, wording

**Files:** `plugin/skills/bridge-screen/SKILL.md`, `src/cli.rs` help text, `src/mcp.rs` tool descriptions, `docs/lead/message-audit.md` rows are Task 14.

Behaviour: the skill gets (a) "When no test can check the work" — use `check: review` for research, planning, docs and plan-file tidying; how it counts as done (saved file or a fuller summary); that the owner can still mark it broken; never switch a task to dodge a failing test (the switch is refused after "broken" and is visible); (b) "Tracks" — read the briefing's track list; if the work doesn't clearly match, ask the owner "continue track X or start a new one?"; start one only with the owner's words via `start_track`; ids are raw (`T2-1.3`); (c) one sentence that removal and hiding are the owner's. Keep the skill short; `claude plugin validate ./plugin` passes; the plugin test that checks skill files still passes.

Tests first: skill-file test asserts the three headings and the tool names exist in `mcp.rs`'s tool list (so the skill cannot name a tool that isn't there); `plugin_validates` (run when `claude` exists on PATH, otherwise skipped with a message).

Commit `docs(plugin): skill teaches tracks and no-test tasks`.

### Task 14: Docs

Update `docs/lead/`: `decision-log.md` entries (next numbers after #42): review status and its guards; track ids and "numbers never reused"; per-track runs/forecast; hide by stamp; remove for good (and why a plan file cannot undo it); Back by hash routes; steps are not tasks. `glossary.md`: track, review task ("done — no tests"), hide / hidden drawer, stamp (plain: "what counts as new"), remove for good. `project-map.md`: `plan/track.rs`, `hide/`, `route.ts`, `HiddenDrawer`, `TrackBar`. `current-state.md` overwritten, dated, with what is checked by running vs read. `message-audit.md`: a row per new owner-facing message (removal confirm, hide stale, close-track refusal, start-track refusal, import dropped steps, parked plan file, review saved/missing, "That page is gone", drawer texts). `web/src/lib/jargon.json`: add any new internal codes (`reviewed`, `track_closed`, `target_removed`, `hidden`, `owner_removed`, `check_kind`, `needs_link` if not present). `CLAUDE.md` layout lines for the new modules and commands; spec notes in `2026-10-13-bridge-screen-design.md` §5.2 (event kinds), §10.1, §11.2, §12.1 pointing at the new spec. `intent.md` is not touched.

Tests: `jargon.test.ts` and the message-audit coverage test (if present) pass.

Commit `docs: tracks, hiding, removing, review tasks, Back`.

### Task 15: Checks

Run, fix, repeat: `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; in `web/`: `npm run lint`, `npm run check`, `npm run test`, `npm run build`, then `cargo build --release` and `npm run e2e`; hook latency `cargo test --release -p bridge-screen --test hooks -- --ignored hook_latency --nocapture` (all existing budgets hold, the new track cases within 50 ms; SessionStart cases within their stated budget); `claude plugin validate ./plugin`; `bridge-screen rebuild` against a copy of a real old database (temp home) and compare `status` before and after; import `docs/superpowers/plans/2026-10-22-project-pages-and-distribution.md` into a temp repo and confirm no step subtasks. Report in the lead-briefing shape: what was run and passed, what was only read, what could not be run (name it). No `--no-verify`.
