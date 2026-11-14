# Tracks, hiding, removing, no-test tasks and Back — design

Date: 2026-11-14. Builds on `2026-10-13-bridge-screen-design.md` (§5 data model, §6 storage, §8.5
verification, §10 screens, §11 agent interface, §12.1 plan import) and on the runs work of
2026-10-26 (decision #40). Owner decisions below were taken in conversation on 2026-11-14.

## Intent and the why

A real folder is not one neat plan. The owner reports five things that all come from that:

1. Some work cannot be tested (research, planning, writing, tidying plan files). Today it can
   never reach "done", so the bar never fills and the owner is nudged about it.
2. One folder can hold several unrelated pieces of work, but there is one bar per folder.
3. The front screen cannot be tidied: old cards, old bars and quiet projects stay in the way.
4. Work that was abandoned half-way ("the manager says we don't need it any more") sits in the
   bars, the queue and the forecast for ever.
5. The browser's Back button and the two-finger swipe leave the dashboard, because the dashboard
   never tells the browser it changed screens.

A sixth, smaller bug: importing a plan file turns every step checkbox into a subtask. Milestone 12
of this repo (`2026-10-22-project-pages-and-distribution.md`) shows "Write the failing tests",
"Run", "Implement", "Run", "Commit" under every task. That is noise the owner cannot act on and it
drowns the real counts.

## Decisions the owner made (2026-11-14)

1. **No-test work.** A task has a check kind: `tests` (today's behaviour, the default) or `review`.
   A `review` task counts as done in done% and in the roll-up as soon as its result is saved (a
   committed file, or a written summary given with `claim_done`), with no owner step. The owner can
   still mark it Broken. Agents set the kind when they create or edit a task and may switch one.
   Screens say so honestly ("done — no tests"). The owner can press ✓ Works / ✗ Broken on any
   claimed task from the project page, not only after two ignored nudges.
2. **Tracks.** One folder can hold several open tracks, each with its own goal, bar, numbering from
   1 and runs. The existing plan becomes track 1 unchanged. A new session asks the owner "continue
   track X or start a new one?" when its work does not clearly match. A track can be finished.
   The glance shows one bar per open track under the project.
3. **Hiding.** The owner can hide any needs-you card, any track bar and a whole project. Hidden
   things go to a "Hidden (n)" drawer where they can be brought back; one comes back by itself only
   when something new happens to it. Hiding never changes numbers. **Remove for good** is separate:
   a task, milestone or track that was abandoned disappears from bars, queues and forecasts for
   good, behind a confirm step, in plain words; the event log keeps the record.
4. **Back works.** Every screen change adds a browser history entry; Back and the swipe restore the
   screen; the `#token` handling keeps working; a reload on a drill screen is sensible.
5. **Plan import** stops importing step checkboxes as subtasks, and the noise already imported is
   dropped by the next import.

## Decisions I made (owner to check)

- **Track ids: prefix the raw id.** Track 1 keeps every id it has (`G`, `3`, `3.1`, `3.1.2`).
  Track `k` ≥ 2 uses the prefix `T<k>-` on the same grammar: goal `T2-G`, milestone `T2-1`, task
  `T2-1.3`, subtask `T2-1.3.1`. The track is read from the id alone, so `claim_task`, `verify`,
  `plan_edit` and every CLI command keep taking one raw id and need no track argument. Chosen over
  a separate `track` column on every table (every key, query and event would grow a field, and old
  events would need rewriting) and over a global renumbering (breaks every old id). The owner sees
  **display ids per track**: the prefix is dropped and numbers restart from 1 (and per run, as today),
  with the raw id in small muted text and the track name above.
- **Track numbers are never reused**, also after a track is removed: the next is the highest number
  ever used plus one. If track 1 is removed and a new track is started, it is `T2-…`; `G` is not
  reissued.
- **A track is its goal node.** Starting a track is a `node_added` of a goal; removing one is
  `node_removed` of that goal (the existing "dropped" state, whole subtree). Only "finished" needs
  a new event pair, `track_closed` / `track_reopened`. No `tracks` table: the list is read from
  goal nodes plus those events, like runs.
- **Per-track runs.** `run_closed` gains an optional `track` (missing = 1). Everything that read
  "the repo's runs" reads one track's runs.
- **Starting a track needs the owner's words** (`owner_said`, same free-text check as `init_plan`
  and the project pages) when an open track exists; with none open it is free. It is not a `plan_edit`
  op, so an agent cannot start tracks by accident. `plan_edit add_node` of a goal works only when no
  open track exists (first goal ever → `G`).
- **A finished track is closed by the owner** (CLI, or the track's menu): refused, in words, while
  a task is neither done nor removed. Agents cannot close tracks. Closed tracks leave the glance and
  sit under "Finished tracks" on the project page. Adding work to a closed track is refused with the
  reason; the owner reopens it.
- **Project-level numbers with several tracks.** The project header shows no single big percentage
  (one bar per track instead); the pooled figure (estimate-weighted over open tracks) is kept in the
  model for the status line and the tab. With exactly one open track nothing looks different from today.
- **Per-track forecast.** Remaining minutes of that track at the pace that track actually had
  (hours/day from its own task time). Calibration (how long tasks really took) stays repo-wide.
  Several tracks sharing one person-day are therefore not double-counted, and a track with no
  recent activity says so instead of showing a date.
- **A task done "without tests" is a status of its own, `reviewed`**, not a pretend L2. It is
  counted as done everywhere the code asks "is this done?" through one helper (today that test is
  written out in about a dozen places). It is never marked stale and never nudged.
- **What "saved" means for a review task** (deterministic, no new text stored): `claim_done` with
  files, and the L1 check passes (the files exist at a commit); or `claim_done` with no files and an
  evidence text of at least 10 words and 60 characters. The evidence text is what a claim already
  stores today. Files named but not yet committed leave it "claimed" until the checkpoint commit.
- **Guards on the switch to review.** The reason is required; refused when the owner's "broken"
  verdict is in force or the task already has L2+; shown in History as "changed to: no tests
  possible" with the agent's reason; shown on the project page.
- **Hiding is a stamp, not a timer.** Each hideable thing has a stamp: the position of the newest
  event that counts as "something new" for it. Hiding stores the stamp seen; the thing stays hidden
  while its stamp is unchanged and returns when it moves. No clock, so it replays the same on
  rebuild. The table below defines "new" per kind.
- **Remove for good reuses `node_removed`.** The same event the agents' `remove_node` op already
  writes, with `by_owner`. Cleanup of nudges, waits and sessions' active task happens for every
  removal (agent or owner), in the same transaction.
- **Back navigation uses hash routes** (`#/project/<repo>`) with `pushState`, not new server routes,
  so the single embedded page and the token flow stay as they are.

## Behaviour

### 1. No-test work (`check: review`)

- `plan_nodes.check_kind` (`tests` | `review`, default `tests`), set by `node_added` (`check`) and
  changed by `node_edited` (`check`). Both fields optional; events from before read as `tests`.
- Agents: `plan_edit` `add_node` / `edit_node` accept `check`; `claim_task` `new_task` accepts
  `check`; `init_plan` task drafts accept `check`. Plan-file tasks import as `tests`; an agent may
  switch one afterwards.
- Status derivation (`verify::refresh_status`), in order: failed → `l2`..`l4` by the highest level
  → for a `review` task with a saved result `reviewed` → `l1` → `claimed` → `active` → `todo`.
  For `tests` tasks the order is today's.
- Counting: `reviewed` is done in done%, roll-ups, run state, forecast remaining, branch groups,
  plan import, recovery and briefing. `Progress` gains `reviewed_tasks` so screens can say "includes 3
  tasks done without tests". Claimed% is unchanged.
- The `verify` tool on a `review` task is refused ("this task is checked by review, not tests");
  the owner's ✓ Works still sets L4 and wins over `reviewed`.
- Owner reach: the Task page and each task row of the project page offer **✓ Works** and **✗ Broken**
  (note required) for any task that is claimed, `l1`, `l2`, `l3` or `reviewed`; the same actions as
  the nudge card, through `owner::act`.
- Screens: a task done this way reads "done — no tests" (icon ✓ in a muted tone, not the tests-pass
  tone); the bar caption says how many tasks count this way.
- Calibration: reviewed tasks' real durations are used like any finished task (they are real work
  time). Open question below.

### 2. Tracks

**Reading ids.** One module (`plan/track.rs`) owns the grammar: `track_of(id) -> u32` (1 when no
prefix), `local(id)`, `raw(track, local)`, `goal_id(track)`, case-insensitive on input
(`t2-1.3` works). Everything that split ids on `.` or parsed the first number (`runs::top_number`,
the progress SQL, rollups) goes through it. The goal constant `"G"` appears in ten places in
non-test code; each is replaced by "the track's goal".

**Reads.** `tracks(conn, repo) -> Vec<Track>`: number, goal id, title, intent, state
(`open` | `closed`), ordered by number. A removed track (goal dropped) is
not listed. `Runs` is loaded per track; `run_tree`, `progress`, `run_state`, `forecast::run`,
`milestones`, `close_in` take a track.

**Starting.** `start_track_in(tx, ctx, title, intent)` (internal): next number, goal id, one
`node_added` (goal, intent required as today). Entry points: MCP `start_track {title, intent,
owner_said}`, CLI `bridge-screen track start "<goal>" [--intent …]` (owner, no words needed),
`init_plan` (a half-built repo with no open track; becomes the next track), the plan importer
(no open track). The reply names the new raw ids.

**Choosing a track.**
- By id: any call with an id uses the id's track. `plan_edit` parents, `claim_task` `existing` and
  `new_task.parent`, `verify`, `claim_done`, `resume`, `owner` commands: no new arguments.
- Plan import: (1) the file was imported before → its track; (2) a goal/milestone with the file's
  title in an open track's current run; (3) exactly one open track → it; (4) several → the file
  is parked as `needs_link` (existing mechanism: nothing is created, the briefing says so) and the
  owner or agent runs `bridge-screen import --track N <file>` or `--into <id>`. Never guess.
- `plan show`, `status`, `forecast`, `usage`: all open tracks by default; `--track N` narrows;
  `usage` gets `--by track`.
- A **new session** is told in the SessionStart briefing: "Tracks (2 open): 1 Ship bridge-screen —
  40%; 2 Docs refresh — 10%. If your work doesn't clearly belong to one, ask the owner: continue a
  track, or start a new one (`start_track`)." At most 5 tracks are listed (most recently active
  first, "+n more"). The claim reminder lists matches across open tracks with the track name and,
  when nothing matches and 2+ tracks are open, repeats the question. The skill carries the rule.

**Closing, reopening.** Owner action `CloseTrack {repo, track}` / `ReopenTrack`, CLI
`bridge-screen track close|reopen N`. Close appends `track_closed {track}`; refused unless every
task of the track is done (tests or reviewed) or removed; the message names up to three open tasks
and says "if the rest was abandoned, remove it first". Closing never touches runs.

**Glance.** `ProjectRow.tracks: Vec<TrackRow>` (open, not hidden): `track`, goal ref, goal status,
done%, claimed%, reviewed tasks, run row, unestimated, milestones, forecast. A project whose every
track is finished reads as today's "Run finished" row. The old top-level `ProjectRow` fields stay
(they describe the pooled figures; with one track they equal that track), so the status line, the
report, the card and branch groups keep working.

### 3. Hiding from the front screen

Subjects: `card:<needs-you id>`, `track:<repo>:<n>`, `project:<repo>`.

| Subject | Stamp: position of the newest… | So it comes back when… |
|---|---|---|
| drift card | `drift_flagged` of that session | the navigator flags it again |
| question card | `needs_owner` (question) of that session | the agent asks another question |
| permission card | `needs_owner` (permission) of that session | another prompt waits |
| verify card | `claimed_done` or `verification_failed` of that task | the agent claims it done again, or it fails |
| leftover card | `leftover_found` in the repo | a new leftover appears (resolving one does not bring it back) |
| plan-scope card | `plan_scope_asked` in the repo | more plan files are found and it is asked again |
| track bar | `task_claimed`, `claimed_done`, `verified`, `verification_failed`, `node_added`, `track_closed`, `run_closed` on a node of the track | someone works on it, checks it, adds to it or finishes it |
| project | `session_started` in the repo, any stamp above for the repo's cards and tracks | a session starts in it, or anything inside it would have come back |

Not "new": usage and time, nudges, drift labels, session ends, plan removals, hides, other tracks.
(Implementer: map each name to the real event kind; where a kind here does not exist, use the
closest event that raises that card and say so in the test.)

- Events `hidden {subject, label, stamp}` and `unhidden {subject}` (owner session), projected into a
  `hides` view table keyed by repo and subject (event position and timestamp only). `rebuild` refills it.
- Reading: the glance computes each subject's current stamp and hides it when equal to the stored one.
  A hidden thing whose subject no longer exists is ignored. Hidden items are returned in
  `Glance.hidden` (kind, label, repo, hidden-at from the event timestamp, the stamp), so the drawer needs one fetch.
- The needs-you count, tab badge and `status` line count only what is on screen; the header says
  "3 need you · 2 hidden". Progress numbers, forecasts, task counts and costs never change when
  something is hidden. A hidden project's live sessions stay in the live-session count.
- Owner actions `Hide {repo, subject, stamp}` (stale when the stamp moved: "something new just
  happened, so it was not hidden") and `Unhide {repo, subject}`. CLI: `bridge-screen hide`,
  `unhide`, `hidden`. Not offered on the desktop card.

### 4. Remove for good

- Owner action `Remove {repo, id, title, confirm}` (stale when the node is gone or its title
  changed; refused without `confirm`; the CLI asks for `--yes`). Dashboard: a row menu on every
  task, milestone and track, and a confirm dialog in plain words naming what goes ("Remove the track
  'Docs refresh' for good? 7 tasks, 2 done. 1 session is working on it. It leaves the bars, the
  queue and the forecast and can't be brought back from the dashboard. The history keeps a record.").
  Buttons "Remove for good" and "Keep it".
- Effect: `node_removed` with `by_owner` for the node (subtree becomes `dropped`; a track's goal for
  a whole track). In the same transaction, for every removal by anyone: `verify_nudges` rows of the
  subtree are deleted by the projector; open waits on it are cleared (`wait_cleared`, why
  `target_removed`); live sessions holding it get `task_released`; plan links to it are deleted
  (already); hides of the track are dropped. Needs-you items disappear because they read those views.
- Forecast and bars read live nodes only, so removed work leaves them. Already finished removed tasks
  keep teaching calibration (they really took that long). The repo's spend is a fact and stays in
  cost; the cost line is not rewritten.
- History keeps it: "You removed task 1.2 'Login form' (it was claimed, not checked)".
- A plan file cannot bring it back: `plan_source_items.owner_removed` marks the link; the importer
  skips items the owner removed and files whose container the owner removed (said once, in the
  import report). A task the importer itself retired and the file lists again does come back.
- Agents keep their `remove_node` op (reason required); only the owner's path is called "for good"
  and confirmed.

### 5. Back and swipe

- `web/src/lib/route.ts`: `toHash(screen)` and `fromHash(hash)` for the eight screens
  (`#/`, `#/project/<repo>`, `#/session/<id>`, `#/task/<repo>/<id>`, `#/history/<repo>`,
  `#/tuning/<repo>`, `#/calibration/<repo>`, `#/pages/<repo>`). `App.svelte` calls one `go(screen)`
  that `pushState`s (never twice for the same screen); a `popstate` listener restores the screen;
  the first screen is read from the hash. "← Glance" uses `history.back()` when the previous entry
  is the dashboard's own, else replaces with the glance.
- `takeToken` keeps stripping `#token=…` with `replaceState` but now keeps a route part
  (`#/project/R&token=abc` → `#/project/R`).
- Reload on a drill screen reopens it from the hash; when the thing no longer exists the screen
  says "That page is gone" with a link to the glance and replaces the entry with the glance.
- The hidden drawer is not a screen; Escape closes it.

### 6. Plan import: step checkboxes are not tasks

- **Rule:** inside a task, `- [ ]` / `- [x]` lines (labelled `**Step k:**` or plain) are never
  nodes. A task is the unit; the plan files in `docs/superpowers/plans` use steps as micro-steps
  (write the test, run it, implement, run, commit), 13–59 per file.
- Ticks still mean something: a task whose steps are all ticked (at least one) is recorded as
  claimed done ("all steps ticked in <file>"), as ticked steps were before. "Has a ticked step"
  still decides the "which files are finished?" question. Partly ticked changes nothing.
- **Noise already imported:** the next import of a file removes every previously imported step
  subtask (a `node_removed` each, importer actor, reason "step checkboxes are no longer imported as
  tasks"), including claimed ones (their ticks were folded into the parent). A step that really
  passed L2+ is kept and named in the import report. Step removals are counted apart, never trigger
  the "plan changed a lot" re-plan, and the dry run lists them. Nothing needs a manual clean-up; for
  an unchanged file, `bridge-screen import <file>` (or the next changed save) does it. SessionStart
  also does it once per file: a `plan_file_seen` whose recorded items include steps counts as changed.

## Events and views

| New event | Data | View / reader |
|---|---|---|
| `node_added` / `node_edited` | optional `check` | `plan_nodes.check_kind` |
| `node_removed` | optional `by_owner` | `plan_nodes` dropped; `verify_nudges`; `plan_source_items.owner_removed` |
| `track_closed`, `track_reopened` | `track` | read when asked (like runs) |
| `run_closed`, `forecast_ran` | optional `track` (missing = 1) | runs; `forecast_history.track` |
| `wait_cleared` | why `target_removed` | `waits` |
| `hidden`, `unhidden` | `subject`, `label`, `stamp` | `hides` |

Schema version 11: `plan_nodes.check_kind`, `plan_source_items.owner_removed`,
`forecast_history.track`, table `hides` (added to the rebuilt tables). All new columns default to
today's meaning.

## Migration

- Opening an older database adds the columns and table; no row is rewritten. Every old plan is
  track 1 (no prefix), every old `run_closed` and `forecast_ran` is track 1, every old task is
  `tests`, every old `node_removed` is not by the owner. Old ids keep working everywhere.
- Rebuilding from events gives the same views (a test replays a log containing every new event kind
  and compares each table).
- Milestone 12's step subtasks are dropped by the import rule above.
- An older program meeting a v11 database refuses it with the existing "schema too new" message;
  the plugin launcher fetches the matching program. Nothing is written inside any repo.

## Constraints

- Hooks ≤ 50 ms p95 and never fail; no new git process. SessionStart's track list is one query; the
  claim reminder one query; hides are read only by the glance, never by a hook.
- Owner actions go through `owner::act` with the stale-click check; agents cannot hide, remove,
  close tracks or answer for the owner; the card's one-time code never offers Hide, Remove or Close.
- Projectors use event timestamps and positions only. Plain words in every new owner-facing
  message; codes stay out of `web/src/lib/jargon.json`'s forbidden list.
- Transcript rules unchanged: no summary text is stored beyond the claim's own evidence.
- Tests never touch the real home or the real `claude`.

## Success checks

1. An old database opens, shows the same bars, and every old id still resolves.
2. A repo with two tracks shows two bars, each from 1, each with its own forecast; `claim_task
   T2-1.1` needs nothing else.
3. A research task marked `review` reaches done% when its file is committed, with no owner click;
   ✗ Broken still takes it back; the bar says how many count this way.
4. A hidden card stays hidden through quiet time and comes back when the agent asks again.
5. A removed task is gone from bars, queue, nudges, waits and forecast, shown in History, and a plan
   file does not bring it back.
6. Back returns from a task page to the project page, then the glance; reload on the project page
   keeps it; `#token=` still works on first load.
7. Importing `2026-10-22-project-pages-and-distribution.md` gives tasks with no step subtasks and
   removes milestone 12's old ones.
8. Latency test, rebuild test, clippy, fmt, web lint/check/test/build pass.

## Not doing

- Starting a track from the dashboard (CLI and agents only this round); un-removing from the
  dashboard; renaming or merging tracks; tracks across repos; per-track costs on the glance;
  importing plain checklists that have no task headings; a plan-file marker for `review`.

## Open questions

1. Should reviewed tasks feed forecast calibration? I said yes (real work time); a repo of mostly
   writing tasks could skew the ratio.
2. With several tracks the project header drops the big percentage. Is the pooled number worth
   showing anywhere on screen besides the tab and the status line?
3. Plain-checkbox plans (no `Task N` headings) are still skipped; that was true before and is left.

## Working rules for the build

- Plan is pseudocode only; one implementer works it in order, a second reviews and fixes.
- Report to the owner in the lead-briefing shape; keep `docs/lead/` current.
