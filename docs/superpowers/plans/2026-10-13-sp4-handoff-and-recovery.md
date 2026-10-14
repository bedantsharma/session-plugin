# Sub-project 4: Handoff and Recovery — Implementation Plan

> **For agentic workers:** Execute task-by-task, test-first. Steps use checkbox (`- [ ]`) syntax. This plan gives interfaces, behaviour rules (pseudocode) and test cases — no full code. Read the cited spec sections and the existing code (sub-projects 1–3) first and follow its patterns: events → projector → views (`Store::write`, `append_in`, `append_in_at` for timestamped tests, `VIEW_TABLES`, schema migrations), hook handlers + never-fail wrapper in `crates/bridge-screen/src/hook/`, MCP tools with `Args<T>` and session binding in `src/mcp.rs`, the navigator (`NoteKind::Checkpoint` and `NoteKind::ReadBack` are already reserved) and its channels.

**Goal:** A new session picks up where a dead or abandoned one stopped, without the owner re-explaining, and half-finished changes never silently cause conflicts. Checkpoint commits are enforced at task boundaries; dead sessions are classified (clean / dirty / crashed); their leftovers are analysed and resolved by adopt / resume / undo (undo never loses work); every session starts with an I-PASS briefing and, when anything is pending, a read-back; WIP commits are squashed before a PR opens.

**Architecture:** A new core module `git` wraps every git operation (always the `git` command, run in a given worktree). New core modules `checkpoint`, `recovery` and `handoff` own the logic; all state is events + views (schema V4). Hooks wire it in: Stop (checkpoint continuation), PostToolUse (commit detection), SessionStart (recovery analysis + briefing), SessionEnd (end classification), PreToolUse (read-back nudge, auto-squash before `gh pr create`). New MCP tools: `claim_done`, `resolve_leftover`, `handoff_readback`, `handoff_note`. New CLI: `squash`, `resolve`.

**Tech Stack:** existing workspace only; no new dependencies expected (add any with `cargo add`).

**Spec:** `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` — §3 (ARIES, I-PASS, Altmann & Trafton rows), §5.2 (handoff events), §7.5 (note priority: checkpoint first), §8.1–8.4 (checkpoints, end classification, recovery, briefing and read-back), §11.1–11.2.

## Global Constraints

- Everything from sub-projects 1–3 still applies (clippy pedantic `-D warnings`, no `unsafe`/`set_var`, never-fail fast hooks, no prompt text or full messages stored, tests never touch the real `~/.claude`/`~/.bridge-screen` or run state-changing `claude` commands, commit per task with the `Co-Authored-By` line, never leave a shell command waiting on stdin).
- **Never lose work.** Undo is always `git stash push` (including untracked files) plus a snapshot record before anything is reverted; squash always leaves a backup ref first. No `git reset --hard`, `git clean`, or `git checkout -- <file>` on files that weren't stashed first.
- **Never touch another live session's worktree.** Leftovers whose worktree belongs to a live session are reported only.
- The fast hook path stays git-free: git runs only when a checkpoint is pending, on SessionStart/SessionEnd, after a Bash tool call that looks like `git commit`, and before `gh pr create`.
- The Stop continuation budget from sub-project 3 still holds (at most once per unit, never with `stop_hook_active`); Checkpoint has the highest priority for it.
- Crash detection is lazy and deterministic: a session with no events for `crash_timeout_min` (config, default 30) is marked crashed by a `session_crashed` event whose data carries the last event time; projectors never read "now".
- Git commands run with `-C <worktree>`, a scrubbed environment where needed (no inherited `GIT_DIR`/`GIT_WORK_TREE`), and `GIT_TERMINAL_PROMPT=0`; their failures become clear errors, never panics.
- Commit identity comes from the user's git config (never set `user.name`/`user.email`); repo commit hooks run (never `--no-verify`).

## Review Focus

1. **Undo really is reversible:** after `resolve_leftover undo`, the stash contains every undone file (tracked and untracked) and `git stash apply` restores them byte-for-byte. Test: `undo_is_reversible` (Task 4).
2. **Live session protection:** recovery started in worktree A must never stash, commit or revert anything in worktree B while B's session is live. Test: `live_worktree_is_report_only` (Task 4).
3. **Checkpoint loop:** an agent that ignores the checkpoint request and stops again must not be asked again in the same unit, and the miss must be visible. Test: `ignored_checkpoint_is_flagged_not_repeated` (Task 3).
4. **Squash safety:** squash on a dirty worktree, with no WIP commits, or when the remote moved must leave history untouched and say why; a successful squash leaves a backup ref. Test: `squash_refuses_unsafe_states` (Task 6).
5. **Briefing size and honesty:** a repo with many leftovers and unverified items still gets a briefing ≤ 40 lines that lists counts and the top items, and a stable session with nothing pending gets the 3-line version with no read-back. Test: `briefing_is_bounded_and_scaled` (Task 5).

---

## File Structure

```
crates/bridge-screen-core/src/
  git.rs                 # all git operations (status, diff, commit, stash, squash, push)
  store/schema.rs        # V4 migration
  checkpoint.rs          # boundaries, pending state, commit detection
  recovery.rs            # end classification, crash scan, leftover analysis + resolution
  handoff.rs             # briefing (I-PASS), read-back check, handoff notes
  projection.rs          # route new events, new VIEW_TABLES
crates/bridge-screen/src/
  hook/handlers.rs       # Stop, PostToolUse, SessionStart, SessionEnd, PreToolUse wiring
  hook/context.rs        # briefing / re-briefing text
  mcp.rs                 # claim_done, resolve_leftover, handoff_readback, handoff_note
  cli.rs, commands.rs    # squash, resolve
  status.rs              # checkpoint / leftover / read-back state
plugin/skills/bridge-screen/SKILL.md
```

---

### Task 1: The git module

**Interface (`bridge_screen_core::git`, every fn takes the worktree path first):**
- `status(wt) -> WorktreeStatus { branch, head: Option<sha>, files: Vec<FileState { path, staged, unstaged, untracked }> }` (porcelain v2, `-z`)
- `is_dirty(wt) -> bool`
- `head(wt) -> Option<String>`; `commit_message(wt, sha) -> String`; `trailers(wt, sha) -> Vec<(key, value)>` (via `git interpret-trailers --parse` or `log --format=%(trailers)`)
- `diff_names_since(wt, sha) -> Vec<String>` (committed + uncommitted)
- `commit_all(wt, subject, trailers) -> sha` (add -A, commit; repo hooks run) and `commit_paths(wt, paths, subject, trailers) -> sha`
- `stash_paths(wt, paths, message) -> StashRef { sha, message }` (includes untracked; only the given paths)
- `merge_base(wt, base) -> Option<sha>`; `default_base(wt) -> String` (`origin/HEAD` target, else `main`, else `master`)
- `commits_since(wt, base_sha) -> Vec<CommitInfo { sha, subject, trailers }>` (oldest first)
- `create_ref(wt, name, sha)`; `soft_reset(wt, sha)`; `upstream(wt) -> Option<String>`; `push_force_with_lease(wt, expected_remote_sha)`
- `worktree_list(wt) -> Vec<WorktreeInfo { path, branch, head }>`

**Rules:** env scrubbed of `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE`; `GIT_TERMINAL_PROMPT=0`; non-zero exit → `Error::Git(stderr)`; paths with spaces and unicode work.

- [ ] Failing tests against temp repos (init with a local user config inside the temp repo only, plus a bare repo as `origin` where needed): status of staged/unstaged/untracked/renamed files; commit_paths commits only those paths and carries trailers that `trailers()` reads back; stash_paths stashes tracked + untracked of the given paths and leaves others; merge_base/commits_since on a branch; worktree_list sees a linked worktree; a path with spaces; push_force_with_lease refuses when the remote moved.
- [ ] Implement; commit `core: git operations module`.

---

### Task 2: Schema V4 and the handoff model

**Views (V4 migration; `SCHEMA_VERSION = 4`):**
- `sessions` gains: `worktree` (toplevel path), `end_kind` (clean|dirty|crashed|NULL), `last_checkpoint`, `checkpoint_pending` (reason or NULL), `checkpoint_asked_unit`, `checkpoint_missed INTEGER`, `readback` (not_needed|pending|ok|mismatch), `briefing_status` (stable|watcher|unstable)
- `checkpoints(sha PK, session_id, repo_id, task_id, next_action, ts)`
- `leftovers(leftover_id PK, repo_id, from_session, worktree, path, task_id, suggested, state open|resolved|report_only, resolution, stash_ref, resolved_by, ts)`
- `handoff_notes(session_id, task_id, in_flight, if_then JSON, ts)` (latest per session wins for briefing)
- `claims_done(repo_id, task_id, session_id, evidence, ts)` — the L0 record sub-project 5 will extend

**Events:**
- `claimed_done { task_id, evidence }` — task status → `claimed` (unless already higher)
- `checkpoint_due { reason: claimed_done|detour_closed|task_switched }`
- `checkpoint { sha, task_id, next_action }`, `checkpoint_missed { unit_no }`
- `session_ended` gains `{ dirty_files: [..], end_kind }` (computed by the hook — see Task 4)
- `session_crashed { last_event_at }`
- `leftover_found { leftover_id, from_session, worktree, path, task_id, suggested, report_only }`
- `leftover_resolved { leftover_id, action: adopt|resume|undo, stash_ref?, commit?, by: agent|owner }`
- `handoff_note { task_id, in_flight, if_then[] }`, `readback { result: ok|mismatch, diff? }`
- `squashed { branch, old_shas[], new_sha, backup_ref }`, `squash_failed { reason }`

**Projector rules:** straightforward upserts; `checkpoint_due` sets `checkpoint_pending` (keep the first reason until a checkpoint clears it); `checkpoint` clears pending/asked and sets `last_checkpoint`; `session_crashed` sets `end_kind = crashed`, `state = ended`, closes the open unit at `last_event_at` (reuse sub-project 2/3 rules); `session_ended` stores `end_kind`; `leftover_resolved` closes the leftover and, for `resume`, nothing else (the claim is a separate `task_claimed`).

- [ ] Failing tests (timestamped events, in-memory store): V3→V4 migration keeps data; each event updates its view; crashed session closes its unit at last_event_at (active time unchanged by the crash gap); rebuild-equality extended with a scripted handoff scenario.
- [ ] Implement; commit `core: handoff model — checkpoints, leftovers, notes (schema v4)`.

---

### Task 3: Checkpoint enforcement

**MCP `claim_done { id?, evidence }`** (id defaults to the active task): records `claimed_done` (L0 only — verification levels come in sub-project 5), emits `checkpoint_due {claimed_done}`, result: `Marked <id> claimed done. Commit a checkpoint before you stop, then leave a handoff_note if work remains in flight.`

**Boundaries that emit `checkpoint_due`:** `claimed_done`; `detour_closed`; `task_claimed` that switched away from a previous task (reason `task_switched`). Emit it in the same transaction as the triggering event (in `claim`, `close_detour`, `claim_done`).

**Stop flow (extends sub-project 3):**
```
if session.checkpoint_pending:
    if git.is_dirty(worktree):
        if checkpoint_asked_unit == this unit or stop_hook_active → emit checkpoint_missed (once); no continuation
        else → candidate Checkpoint (channel Stop, top priority):
               "bridge-screen: checkpoint before stopping — commit as `wip(bridge): <id> <title>` with trailers
                Bridge-Task: <id>, Bridge-Next: <one-line next action>, Bridge-Session: <session id>."
               (record checkpoint_asked_unit via note_sent)
    else → clean: emit checkpoint {sha: HEAD, task, next_action: null} to clear pending
```
**Commit detection:** PostToolUse for Bash whose command contains `git commit` (also run at Stop when pending): if HEAD changed since `last_checkpoint`, read its trailers; if it has `Bridge-Task` or the subject starts with `wip(bridge):` → `checkpoint {sha, task, next_action = Bridge-Next}`. Commits without trailers made while pending also clear it (record with next_action null).

**Checkpoint missed:** shows in `status` as `checkpoint missed` and moves the session into needs-you (Task 7 displays it).

- [ ] Failing tests (temp repo + binary hooks): claim_done → dirty → Stop returns the checkpoint continuation with the exact commit format; agent commits with trailers → PostToolUse records the checkpoint (next_action from the trailer) and clears pending; `ignored_checkpoint_is_flagged_not_repeated`; clean worktree clears pending silently; detour close and task switch create checkpoint_due; Checkpoint outranks a label request in the same Stop; known-session hooks with no pending checkpoint still spawn no git.
- [ ] Implement; commit `core: checkpoint boundaries; hooks: enforce checkpoint commits; mcp: claim_done`.

---

### Task 4: End classification, crash scan and leftover recovery

**End classification (SessionEnd hook):** `git status` in the session's worktree → `session_ended { end_kind: clean if no dirty files and no claimed-but-unfinished task else dirty, dirty_files }`. "Claimed-but-unfinished" = session's active task status is `active` (not claimed/L1+).

**Crash scan:** `recovery::scan_crashed(conn, repo, now, timeout)` → for each non-ended session with `last_event_at < now − timeout`, emit `session_crashed`. Called from SessionStart and from `status`.

**Leftover analysis (`recovery::analyse(store, repo, my_session, my_worktree)`, at SessionStart):**
```
dead = sessions in repo with end_kind ∈ {dirty, crashed} whose leftovers were never analysed
for each dead session D (this worktree first):
    wt = D.worktree ; if wt no longer exists → leftover note "worktree removed", nothing to resolve
    live_owner = a live session (not me) whose worktree == wt
    files = git.status(wt) dirty files (if wt == my_worktree, these are mine to resolve)
    for each file:
        task = task of the latest unit (any session) that recorded tool_intent/file_changed for it, else D.active_task
        suggested =
            resume  if task is live and status active (not claimed/L1+) and D.active_task == task
            adopt   if task status is claimed or higher
            undo    if the file was only touched in units labelled drift or in an abandoned detour, or no unit ever touched it
            resume  otherwise
        emit leftover_found {…, report_only: live_owner.is_some()}
```
**Resolution (`recovery::resolve(store, leftover_ids, action, by)`) — MCP `resolve_leftover { ids[], action }` and CLI `bridge-screen resolve <leftover-id|prefix>… adopt|resume|undo`:**
```
refuse if report_only (live owner) — error names the live session
adopt  → git.commit_paths(wt, paths, "wip(bridge): adopt leftovers from <short D>", trailers Bridge-Task) → record commit
resume → if wt == caller's worktree: claim the task (task_claimed, mode confirmed) ; else error "resume it from <wt>"
undo   → stash = git.stash_paths(wt, paths, "bridge-screen undo <ids>") ; write snapshots/<repo>/<leftover_id>.json {stash sha, paths, from_session, ts}
         (stash_paths already removes them from the worktree — no other revert step)
emit leftover_resolved per id ; owner resolution (by owner) wins over a later agent attempt (agent gets "already resolved by the owner")
```

- [ ] Failing tests (temp repo with a linked worktree, binary hooks + MCP): SessionEnd dirty vs clean; crash scan marks a silent session and is idempotent; analysis attributes files to tasks via unit history and suggests per the rules; `undo_is_reversible` (tracked + untracked, `git stash apply` restores byte-for-byte; snapshot file written); adopt commits only those paths with trailers; resume claims; `live_worktree_is_report_only`; owner-resolved leftover refuses a later agent action; removed worktree handled without errors.
- [ ] Implement; commit `core: end classification, crash scan and leftover recovery; mcp: resolve_leftover; cli: resolve`.

---

### Task 5: Briefing, read-back and handoff notes

**Briefing (SessionStart, sources startup/resume/clear/fork) — `handoff::briefing(conn, session) -> Briefing { status, text, readback_required }`, ≤ 40 lines:**
```
status: unstable if open leftovers or a failed/missed item (checkpoint_missed, failed task)
        watcher  if open detours, suspended items, or claimed-but-unverified items (claims_done not yet ≥ L1)
        stable   otherwise
text (I-PASS order):
  bridge-screen briefing — <status>
  Goal: <goal title> — <goal intent, first sentence>
  Milestone: <current milestone title> — <intent first sentence>          (milestone of active/candidate task)
  Summary: <active task or "no task yet">; last checkpoint <short sha> "<next action>"; resume cue if suspended
  Actions:
    - Verify first: <up to 3 claimed-but-unverified tasks> (+N more)
    - Leftovers: <up to 5 "path (task) → suggested">, (+N more) — resolve with resolve_leftover
    - Next: <resume cue or Bridge-Next of last checkpoint>
  If/then: <if_then lines from the latest handoff_note of the previous session on this task>
  Dependencies: <deps of the active/candidate task: ready / waiting on <id> (<session>)>
  Read back before your first edit: call handoff_readback with the goal, what's in flight, and your next action.   (only if not stable)
stable with nothing pending → 3 lines: status, goal, "Claim your task and go."
```
**Compact re-briefing (source `compact`):** active task, its resume cue/next action, open detours — ≤ 8 lines, no read-back.

**Read-back — MCP `handoff_readback { goal, in_flight, next_action }`:** compare with the log: in_flight must mention the active task id (or a leftover's task id when no task); next_action must share ≥ 1 keyword with the expected next action (resume cue / Bridge-Next) when one exists. Emit `readback {ok|mismatch, diff}`. Mismatch → result text is the one correction: `Close, but the log says: active task <id> "<title>", next action "<cue>". Carry on from there.` Never more than one correction per session.

**Read-back nudge:** PreToolUse on an edit tool while `readback == pending` → `ReadBack` note via PreToolUse context (once): `bridge-screen: read back the briefing first (handoff_readback) — it takes one call.` Not blocking.

**Handoff note — MCP `handoff_note { in_flight, if_then[] }`:** records `handoff_note` for the active task. Requested by the `claim_done` and `start_detour` results ("if work remains in flight, leave a handoff_note").

- [ ] Failing tests: `briefing_is_bounded_and_scaled` (30 leftovers + 10 unverified → ≤ 40 lines with counts; stable → 3 lines, readback not_needed); status classification; if/then lines carried from the previous session's note; compact re-brief; read-back ok / mismatch with exactly one correction; nudge once on the first edit and never when stable.
- [ ] Implement; commit `core: I-PASS briefing and read-back; mcp: handoff_readback, handoff_note`.

---

### Task 6: Squash — manual and automatic before a PR

**`bridge-screen squash [-C DIR] [--onto BASE] [--dry-run]`:**
```
base = --onto or git.default_base ; mb = merge_base(HEAD, base)
commits = commits_since(mb) ; wips = those with subject "wip(bridge):" or a Bridge-Task trailer
refuse (history untouched, clear message) if: worktree dirty ; no wips ; HEAD detached
backup = refs/bridge-screen/backup/<branch>/<unix ts> → create_ref(backup, HEAD)
message = "<branch>: <N> commits squashed" + blank line + one line per task touched ("<id> <title>") + one line per non-wip subject
soft_reset(mb) ; commit_all(message, trailers: Bridge-Squashed-From: <backup ref>)
if upstream exists and was pushed: push_force_with_lease(expected = upstream sha as fetched before) ; failure → squash_failed + keep local result, report
emit squashed {branch, old_shas, new_sha, backup_ref}
--dry-run prints what would happen and changes nothing
```
**Automatic before a PR (PreToolUse):** Bash command matching `\bgh\s+pr\s+create\b`, or an MCP tool whose name ends with `create_pull_request` → run the squash flow in the session's worktree before the tool runs. On refusal because there are no WIP commits → do nothing silently. On any other failure → `squash_failed`, `needs_owner {kind: "squash"}`, and PreToolUse context: `bridge-screen: WIP commits weren't squashed (<reason>); the PR will show them. Run bridge-screen squash later if you want them collapsed.` Never block the PR.

- [ ] Failing tests (temp repo + bare origin): squash collapses WIP + regular commits into one with the generated message and a backup ref that still points at the old HEAD; `squash_refuses_unsafe_states` (dirty, no wips, detached, remote moved → history unchanged, clear reason); dry-run changes nothing; already-pushed branch gets force-pushed with lease; PreToolUse on `gh pr create` squashes first and lets the command run; on failure the hook returns the context and exits 0.
- [ ] Implement; commit `cli: squash; hooks: squash WIP commits before a PR opens`.

---

### Task 7: Status, skill, docs and latency

- `status`: per session add `checkpoint pending` / `checkpoint missed`, `read-back pending`, and per repo a line `leftovers: N open (M report-only)` when any. Needs-you order: checkpoint missed and unstable sessions after red drift.
- `status()` MCP tool: include pending checkpoint, open leftovers with suggested actions, and read-back state.
- `SKILL.md`: checkpoint commit format; `claim_done` when a task is done; read the briefing and call `handoff_readback` before editing when asked; resolve leftovers (prefer the suggestion; undo is safe, it stashes); leave a `handoff_note` when stopping mid-task. Keep the whole skill within its line limit test (raise the limit to 120 if needed, and update that test).
- `CLAUDE.md`: new modules and commands. `README.md`: mention squash and recovery.
- Latency: re-run the ignored latency tests; add a case for PostToolUse on a Bash `git commit` with a pending checkpoint (git runs here — target p95 < 150 ms) and confirm the plain PostToolUse/Stop paths with no pending checkpoint are unchanged.
- `claude plugin validate ./plugin` and `.` pass.
- [ ] Commit `docs: handoff and recovery in status, skill and docs; latency recheck`.
