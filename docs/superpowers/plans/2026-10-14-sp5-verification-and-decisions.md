# Sub-project 5: Verification and Decisions — Implementation Plan

> **For agentic workers:** Execute task-by-task, test-first. Steps use checkbox (`- [ ]`) syntax. This plan gives interfaces, behaviour rules (pseudocode) and test cases — no full code. Read the cited spec sections and the existing code (sub-projects 1–4) first and follow its patterns: events → projector → views (`Store::write`, `append_in`, `append_in_at` for timestamped tests, `VIEW_TABLES`, schema migrations in `store/schema.rs`), the `git` module (scrubbed env, `-C <worktree>`), hook handlers + never-fail wrapper in `crates/bridge-screen/src/hook/`, MCP tools with `Args<T>` and session binding in `src/mcp.rs`, the navigator (`NoteKind::VerifyNudge` and `NoteKind::DependencyReady` are already reserved) and its channels, and the briefing in `handoff/briefing.rs`.

**Goal:** A task claimed done is actually done, and the reasons behind the work survive. Every task climbs an honest verification ladder (L0 claimed → L1 code exists → L2 unit tests pass → L3 end-to-end → L4 owner), every verification is tied to a commit and drops a level when later commits touch what it depended on, unverified work leads the next briefing and escalates to the owner when ignored, and decisions are captured at task completion, before compaction and at session end, with supersede chains.

**Architecture:** A new core module `verify` owns the ladder: L1 checks against `HEAD` through the `git` module, the L2 test re-run (a bounded child process, run from the MCP server, never from a hook), staleness detection and the status derivation. A new core module `decision` owns decisions, supersede chains and duplicate merging. All state is events + views (schema V5). Hooks wire in only cheap SQL paths (nudges, dependency-ready notes, decision-capture flag) plus staleness at SessionStart (git already runs there). New MCP tools: `verify`, `log_decision`; `claim_done` gains structured evidence. New CLI: `verify` (owner L4 / broken), `decisions`, `config set|unset` for per-repo test settings. `status` gains done% / claimed%.

**Tech Stack:** existing workspace only. If a crate is needed (e.g. for waiting on a child with a timeout), add it with `cargo add <crate>@<ver>` — never hand-edit `Cargo.toml`. A `try_wait` poll loop needs no crate and is preferred.

**Spec:** `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` — §5.2 (verification and decision events), §5.3 (status L0–L4), §5.6 (done% counts ≥ L2), §7.5 (note priority: verification nudge last; dependency ready), §8.1 (squash re-points verifications), §8.4 (unverified items lead the briefing; re-briefing shows this session's decisions), §8.5 (verification ladder), §8.6 (decisions), §9.1 (dependency ready note), §10.1 (needs-you: owner verification ✓/✗), §11.2 (`claim_done`, `verify`, `log_decision`).

**Branch / worktree:** `feat/sp5-verification-decisions` in `/Users/bedantsharma/PycharmProjects/session-plugin-sp5` (branched from `feat/sp4-handoff-recovery`, which is not merged yet). Work only in that worktree — the main checkout belongs to another live session.

## Global Constraints

- Everything from sub-projects 1–4 still applies: clippy pedantic `-D warnings`, no `unsafe`/`set_var`, never-fail fast hooks (log + exit 0), no prompt text or full assistant messages stored, tests never touch the real `~/.claude`/`~/.bridge-screen` or run the real `claude`, commit per task ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, pre-commit/pre-push hooks never bypassed (`--no-verify` is forbidden), never leave a command waiting on stdin.
- **Never run tests from a hook.** The L2 re-run happens only inside the `verify` MCP tool call (or the owner's CLI). Hooks stay ≤ 50 ms p95 on their fast paths; the only new git work in hooks is staleness at SessionStart and the L1 check when a checkpoint commit is recorded (both already git paths).
- **Rebuild never re-runs anything.** Every outcome (L1 result, re-run exit code, duration, timeout) is stored in the event; projectors only read events. Projectors never read "now".
- **Every verification is tied to a commit SHA** (spec §8.5). L1–L3 by the agent are refused while the files they depend on have uncommitted changes ("commit a checkpoint first").
- **Only the owner sets the test command** (`bridge-screen config set test-command …`, recorded as `config_changed` with `session_id = owner`). No MCP tool can set or change it. The re-run executes it with `sh -c` in the session's worktree, stdin null, stdout/stderr captured (only the exit code, duration and the last non-empty output line ≤ 200 chars are kept), and the whole process group killed at the timeout.
- Default re-run timeout `verify_timeout_s = 120` in `config.toml`; a per-repo `test_timeout_s` overrides it.
- Done% (spec §5.6): `Σ estimate_min(live leaves at ≥ L2) / Σ estimate_min(all live leaves)`; claimed% = leaves at claimed/L1, shown separately and never counted in done%. Leaves without an estimate count as 0 minutes for both sums (and are listed as "unestimated: N").
- Decision text limits: `what` ≤ 200 chars, `why` ≤ 500, `alternatives` ≤ 5 items × 200 chars; evidence ≤ 500 chars; re-run summary ≤ 200 chars. Over-long input is truncated on a char boundary, never rejected.

## Review Focus

1. **Verification on uncommitted work:** `verify` (L2) with uncommitted edits in the task's files must be refused with a "commit first" message and record nothing; once committed it records the new HEAD. Test: `verify_refuses_uncommitted_work` (Task 3).
2. **A hanging or forking test command:** a test command that sleeps past the timeout or spawns a background child is killed (whole process group) at the timeout, the MCP call returns, and the verification is recorded *as reported* with `rerun: timed_out`. Test: `rerun_timeout_kills_process_group` (Task 3).
3. **Staleness idempotence:** repeated SessionStarts with no new commits never drop a level twice; a commit touching an unrelated file drops nothing; a commit touching a depended-on file drops exactly one level. Test: `stale_drops_once_per_change` (Task 4).
4. **Squash keeps verifications:** after `bridge-screen squash` (or the automatic pre-PR squash), verifications on the old WIP SHAs are re-pointed to the new SHA, and the next SessionStart doesn't mark them stale or lose them. Test: `squash_keeps_verifications` (Task 4).
5. **Decision chains:** superseding an already-superseded decision, a decision from another repo, or itself is refused with a message naming the newer decision; a duplicate `what` in the same task merges into one row (and rebuild gives the same rows). Test: `decision_chain_rules` (Task 6).

---

## File Structure

```
crates/bridge-screen-core/src/
  store/schema.rs          # V5 migration
  verify/mod.rs            # public API: ladder, L1 check, re-run, staleness, status derivation, progress
  verify/project.rs        # VERIFY_KINDS projector: verifications, nudges, repo_settings
  verify/rerun.rs          # bounded child process (timeout, process-group kill, last line)
  verify/tests.rs
  decision/mod.rs          # log_decision, supersede rules, duplicate merge, queries
  decision/project.rs      # DECISION_KINDS projector
  decision/tests.rs
  git/mod.rs               # + exists_at, grep_at, changed_between
  checkpoint.rs            # claim_done gains files/symbols; L1 at checkpoint
  handoff/briefing.rs      # unverified/stale lines, re-brief decisions, capture ask
  navigator.rs             # VerifyNudge, DependencyReady candidates
  projection.rs            # route new kinds; new VIEW_TABLES
  config.rs                # verify_timeout_s
crates/bridge-screen/src/
  mcp.rs                   # verify, log_decision; claim_done files/symbols; status additions
  hook/handlers.rs, hook/handoff.rs, hook/nav.rs   # staleness at SessionStart, nudges, dep ready
  cli.rs, commands.rs      # verify, decisions, config set/unset
  status.rs                # done%/claimed%, verify-overdue needs-you
plugin/skills/bridge-screen/SKILL.md
```

---

### Task 1: Schema V5 and the verification/decision model

**Views (V5 migration; `SCHEMA_VERSION = 5`):**
- `verifications(verif_id PK, repo_id, task_id, level INTEGER 1..4, orig_level, sha, files JSON, evidence, by auto|agent|rerun|owner|owner_via_agent, rerun TEXT NULL (passed|failed|timed_out|not_configured), session_id, state current|stale|failed, stale_checked_sha, ts)`; index `(repo_id, task_id)`.
- `decisions(decision_id PK, repo_id, task_id NULL, session_id, what, what_key, why, alternatives JSON, supersedes NULL, superseded_by NULL, merged_count INTEGER DEFAULT 1, ts, updated_ts)`; index `(repo_id, task_id, what_key)`.
- `verify_nudges(repo_id, task_id, nudges INTEGER, level_at_first INTEGER, escalated INTEGER, PRIMARY KEY(repo_id, task_id))`.
- `repo_settings(repo_id PK, test_command NULL, test_timeout_s NULL)`.
- `sessions` gains `decision_capture TEXT NULL` (precompact|claimed_done|null).
- `claims_done` gains `files JSON DEFAULT '[]'`, `symbols JSON DEFAULT '[]'`.

**Events (`verify::VERIFY_KINDS`, `decision::DECISION_KINDS`):**
- `claimed_done` gains optional `files[]`, `symbols[]` (serde default empty; old events read fine).
- `verified { verif_id, task_id, level, sha, files[], evidence, by, rerun? }`
- `verification_failed { task_id, level, sha?, evidence, by, rerun? }`
- `verification_stale { verif_id, task_id, from_level, to_level, head_sha, changed[] }`
- `verify_nudged { task_id, level }`
- `decision { decision_id, task_id?, what, why, alternatives[], supersedes? }`
- `decision_asked {}` (the capture request went out; clears `decision_capture`)
- `config_changed` already exists (tangent projector, `TUNING_KEYS`); add `SETTING_KEYS = ["test_command", "test_timeout_s"]` routed into `repo_settings` (value `null` = unset).
- `squashed` (exists, currently history-only) now also projects: re-point verifications.

**Status derivation — `verify::refresh_status(tx, repo, task)`, called by every projector that changes verifications or claims:**
```
rows = verifications of task
if the latest event among {verified, verification_failed, claimed_done} for the task is verification_failed:
    status = failed           # back to work; claim_task moves failed → active (existing mark_claimed)
elif any row with state current or stale:  status = "l{max(level)}"   # stale rows already hold the dropped level
elif a claims_done row exists:             status = claimed
else: leave status alone (todo/active)
never touch dropped nodes
```
**Projector rules:**
- `verified` → insert row (`state current`, `orig_level = level`, `stale_checked_sha = sha`); refresh_status. If `level ≥ 2` and the task had a `verify_nudges` row → delete it (the nudge was answered).
- `verification_failed` → set every row of the task to `state failed`; refresh_status.
- `verification_stale` → row.level = to_level, `state = stale` (a row at level 0 counts for nothing, so refresh_status falls back to claimed); `stale_checked_sha = head_sha`; refresh_status.
- `squashed` → for rows with `sha ∈ old_shas`: `sha = new_sha`, `stale_checked_sha = new_sha` (same repo only).
- `verify_nudged` → upsert nudges+1 (`level_at_first` set on insert).
- `precompact` (session projector) → `decision_capture = 'precompact'`; `claimed_done` → `decision_capture = 'claimed_done'` unless already set; `decision` or `decision_asked` from that session → NULL.
- `decision` → see Task 6 (merge and supersede rules live in `decision::project`).

**Progress — `verify::progress(conn, repo) -> Progress { done_pct, claimed_pct, done_min, claimed_min, total_min, unestimated }`**: live leaves = tasks/subtasks without children, not dropped. Done = status l2..l4. Claimed = claimed or l1.

- [ ] Failing tests (in-memory store, timestamped events): V4→V5 migration keeps data and old `claimed_done` events still project; status derivation table (claimed → verified L1 → L2 → failed → re-claimed → L2 again); stale drops one level and refresh_status follows; squashed re-points only matching SHAs of the same repo; `config_changed test_command` lands in `repo_settings` and unset clears it; progress counts L2+ in done%, claimed/L1 only in claimed%, ignores dropped nodes and parents; rebuild-equality (`tests/rebuild.rs`) extended with a scripted verification + decision scenario.
- [ ] Implement; commit `core: verification and decision model (schema v5)`.

---

### Task 2: L1 — code exists at HEAD

**git additions (`git/mod.rs`):**
- `exists_at(wt, rev, path) -> bool` (`git cat-file -e <rev>:<path>`, false on missing)
- `grep_at(wt, rev, word, paths) -> bool` (`git grep -q -F -w -e <word> <rev> -- <paths>`; exit 1 = false, other non-zero = error)
- `changed_between(wt, from, to) -> Vec<String>` (`git diff --name-only -z <from> <to>`, includes renames' both names via `--no-renames`)

**`claim_done` (MCP + `checkpoint::ClaimDoneRequest`) gains `files: Vec<String>` and `symbols: Vec<String>`** (optional; paths relative to the worktree root, ≤ 50 each). Tool description: "list the files (and optionally the key functions/types) that show the work exists — bridge-screen checks them at HEAD for L1".

**`verify::check_l1(conn, wt, repo, task) -> L1Outcome { Passed{sha, files}, Missing{paths, symbols}, NotCommitted{paths}, NoEvidence }`:**
```
claim = latest claims_done for task
files = claim.files, else files recorded on units tagged with this task (any session) that exist in the worktree
if files empty → NoEvidence
if any file has uncommitted changes (git status) → NotCommitted
head = git.head(wt)
missing = files where !exists_at(head, f) ; missing_syms = symbols where !grep_at(head, s, files)
Passed if both empty
```
On Passed the caller appends `verified {level 1, by auto}` (`auto` is the fifth value of the `by` enum: auto|agent|rerun|owner|owner_via_agent). `check_l1` itself only reads.

**When L1 runs (git already allowed on these paths):**
1. In the `claim_done` MCP call, right after recording the claim, if `check_l1` passes → append `verified {level 1, by auto}` and say so in the result; `NotCommitted` → result says L1 will be checked at the checkpoint commit; `Missing` → result names the missing paths/symbols ("fix the evidence with claim_done again").
2. When a checkpoint commit is recorded (`checkpoint::detect_commit` path, PostToolUse after `git commit`, and the Stop clean-worktree path) and the session has a claimed task without a current L1+ verification → run `check_l1`; on Pass append `verified L1`; on `Missing` add a PostToolUse context line: `bridge-screen: L1 check — <paths> not found at <short sha>. Fix the evidence with claim_done.`
3. At SessionStart staleness re-check (Task 4) for rows that dropped to level 0.

- [ ] Failing tests (temp repo + binary MCP/hooks): git helpers (exists_at / grep_at with spaces and unicode in paths, word boundaries; changed_between); claim_done with files on a clean committed tree → status l1 at HEAD; claim_done before committing → claimed, then the checkpoint commit → l1 with the commit's SHA; missing file → stays claimed and the message names it; no files given → falls back to the unit's recorded files; known-session hooks with no claimed task still spawn no git on PostToolUse.
- [ ] Implement; commit `core: L1 checks at HEAD; mcp: claim_done takes files and symbols`.

---

### Task 3: L2–L4 — `verify` tool, test re-run, owner verification

**Per-repo settings CLI (owner only):** `bridge-screen config set test-command "<cmd>" [-C DIR]`, `config set test-timeout <secs>`, `config unset test-command|test-timeout`. Each appends `config_changed {key, old, new}` with `session_id = owner`. `bridge-screen config` prints them under the repo's thresholds.

**Re-run — `verify::rerun::run(wt, command, timeout) -> RerunOutcome { Passed{secs, last_line}, Failed{code, secs, last_line}, TimedOut{secs}, SpawnError(msg) }`:**
```
spawn `sh -c <command>` in wt: stdin null, stdout+stderr piped to one buffer (read on threads so pipes never fill),
       own process group (std CommandExt::process_group(0) on unix)
poll try_wait every 50 ms until timeout
timeout → kill the group (`kill -TERM -<pgid>` via the kill command, then -KILL after 2 s), reap, TimedOut
last_line = last non-empty line of the output, trimmed to 200 chars
```

**MCP `verify { id?, level: 2|3|4, evidence, exit_code?, summary?, owner_said? }`** (id defaults to the active task; runs on a blocking thread so the server stays responsive):
```
task must be claimed or higher (else: "claim_done first")
level 2:
    exit_code required; != 0 → refuse, record nothing: "Exit code N means the tests failed — fix them, commit, and verify again."
    files = task files (claims_done.files ∪ unit files); any uncommitted → refuse "commit a checkpoint first; verifications are tied to a commit"
    if current level < 1: run check_l1 first; Missing → refuse naming the paths
    cmd = repo_settings.test_command
    none → verified {level 2, by agent, rerun not_configured} ; result adds "Ask the owner to set a test command: bridge-screen config set test-command '<cmd>'"
    else outcome = rerun(wt, cmd, timeout)
        Passed   → verified {level 2, by rerun, rerun passed, evidence = agent evidence + " · re-run ok in Ns"}
        Failed   → verification_failed {level 2, by rerun, rerun failed, evidence = last_line}; result: "The re-run failed (exit N): <last line>. 2.3 is back to failed; fix it and verify again."
        TimedOut → verified {level 2, by agent, rerun timed_out} ("recorded as reported; the re-run took longer than Ns")
        SpawnError → verified as reported, rerun not_configured-like note with the error
level 3: needs current level ≥ 1 and committed files; evidence must say what was driven; verified {level 3, by agent}
level 4: owner_said required (the owner's words, ≤ 200 chars); verified {level 4, by owner_via_agent}
every verified row: sha = HEAD of the session's worktree, files = task files
```
Result text always states the new level and done% of the project ("2.3 verified L2 (re-run passed in 4s). Project 41% done.").

**CLI `bridge-screen verify <task-id> [--broken "<note>"] [--level 4] [-C DIR]`** (owner): ✓ → `verified {level 4, by owner, sha = HEAD of DIR's worktree}`; `--broken` → `verification_failed {by owner, evidence = note}`. Refuses dropped nodes, goals and milestones. Both write `owner_action` too, like the drift/label commands.

- [ ] Failing tests: rerun passes/fails/times out (`sh -c 'sleep 30'` with a 1 s timeout returns within ~3 s); `rerun_timeout_kills_process_group` (command starts a background `sleep 60 &` writing its pid to a temp file, then sleeps; after the timeout that pid is gone); output larger than the pipe buffer doesn't deadlock; `verify_refuses_uncommitted_work`; L2 with no test command records as reported with the owner hint; L2 re-run failure sets failed and a later claim_task makes it active again; L2 with exit_code 1 refused; L3 requires L1; L4 requires owner_said; MCP agent can't set the test command (no tool); CLI `verify` ✓ and `--broken`; config set/unset round trip and `config` shows them.
- [ ] Implement; commit `core: L2 test re-run, L3 and L4; mcp: verify; cli: verify and per-repo test settings`.

---

### Task 4: Staleness and squash re-pointing

**`verify::scan_stale(store, repo, wt) -> Vec<StaleChange>`, run at SessionStart (sources startup/resume/clear/fork, after recovery, before the briefing), in the starting session's worktree:**
```
head = git.head(wt) ; rows = current/stale rows of the repo, newest 50
group rows by stale_checked_sha
for each group sha:
    skip if sha == head
    skip if !is_ancestor(sha, head)           # other branch, or rewritten history → leave alone
    changed = changed_between(wt, sha, head)  # one git call per distinct sha
    for row in group:
        hit = row.files ∩ changed
        if hit empty → no event (the next run recomputes from the same sha: cheap and idempotent)
        else → verification_stale {from_level: row.level, to_level: row.level - 1, head_sha: head, changed: hit}
then for rows that reached level 0: run check_l1; Pass → verified L1 at head
```
Idempotence comes from `stale_checked_sha = head_sha` after a drop: a second SessionStart at the same head finds `sha == head` and skips. A drop happens at most once per (row, new commit range that touches its files).

**Squash:** the `squashed` projector (Task 1) re-points SHAs. Also check that `squash` keeps `old_shas` complete (every commit between merge base and old HEAD, not only WIP ones) so verifications made on a non-WIP commit in the range are re-pointed too — fix `squash::Done.old_shas` if it lists only WIP commits.

**Briefing integration (handoff/briefing.rs):** stale rows appear in "Verify first" as `2.3 Login form (stale: auth.rs changed @abc1234)`.

- [ ] Failing tests (temp repo, binary hooks): `stale_drops_once_per_change`; verification on another branch's commit is skipped; L1 row that drops to 0 is re-checked to L1 at the new head when its files still exist; deleted file → stays claimed and listed; `squash_keeps_verifications` (verify L2 on a WIP commit, squash, SessionStart → still L2, sha = new sha); SessionStart with 50+ verifications stays within the existing SessionStart latency test.
- [ ] Implement; commit `core: verification staleness at session start; squash re-points verifications`.

---

### Task 5: Unverified items — briefing, nudges, owner escalation, dependency ready

**Briefing ("Verify first"):** list tasks at claimed or L1 whose claim was made in a session other than the briefed one (spec: "older than one session"), plus stale rows, up to 3 shown with `(+N more)`; keep the 40-line bound. Each task listed in a briefing appends one `verify_nudged {task, level}` (in the same write as `briefed`).

**Prompt nudge (`NoteKind::VerifyNudge`, channel `PromptSubmit`, lowest priority, SQL only):** for the live session that claimed a task done in an earlier unit and it's still claimed/L1: `bridge-screen: 2.3 is claimed done but not verified (L1). Run the tests and call verify(2.3, level 2).` State key `verify:<task>:<level>`; the navigator's repeat window applies; each delivery appends `verify_nudged`.

**Escalation:** when `verify_nudges.nudges ≥ 2` and the task's level hasn't changed since `level_at_first` and not yet escalated → `needs_owner {kind: verify, task_id}` (new `NeedsOwnerKind::Verify`, not a wait) and set `escalated = 1`. The CLI `status` shows it in needs-you as `verify 2.3 Login form — bridge-screen verify 2.3 (or --broken "…")`. A later `verified ≥ L2` or `verification_failed` clears it (delete the nudges row → the needs-you entry disappears; needs-you reads from `verify_nudges where escalated = 1`).

**Dependency ready (`NoteKind::DependencyReady`, channel `PromptSubmit`, SQL only):** for each live session whose active task has `depends_on` X with X at ≥ L2: `bridge-screen: X <title> is done on \`<branch of the verifying session>\` @<short sha>. Merge or rebase to pick it up.` State key `dep:<X>:<sha>`, sent once per state key per session (use `notes_sent`), skipped when the verifying session is this session.

- [ ] Failing tests: briefing lists only older-session claims, stale rows, counts beyond 3, still ≤ 40 lines with 30 leftovers + 10 unverified (extend `briefing_is_bounded_and_scaled`); nudge at prompt once per repeat window and never above higher-priority notes; two ignored nudges → needs-you entry in `status`; L2 clears it; dependency-ready note once per (session, dep, sha), not to the verifier itself; prompt hook for a session with nothing unverified spawns no git and stays on the fast path.
- [ ] Implement; commit `core: unverified items lead the briefing, nudge, escalate; dependency-ready notes`.

---

### Task 6: Decisions

**MCP `log_decision { what, why, alternatives[], supersedes?, task? }`** (task defaults to the active task; may be null for repo-wide decisions):
```
validate: what/why non-empty (trimmed); truncate per Global Constraints
what_key = lowercase, whitespace-collapsed, trailing punctuation stripped
supersedes given → must exist in this repo; must not be itself; if already superseded → refuse "D4 was already superseded by D7 — supersede D7 instead"
duplicate = live (not superseded) decision with same repo, same task, same what_key
id = duplicate.id if duplicate else next "D<n>" for the repo (max n + 1, inside the write transaction)
append decision {decision_id: id, …}
result: "Logged D7 (supersedes D4)." | "Merged into D3 (same decision on 2.3)."
```
**Projector (`decision::project`):** if a row with `decision_id` exists → merge: `why` = new, alternatives = ordered union (≤ 5), `merged_count += 1`, `updated_ts` = event ts; else insert. If `supersedes` → set the old row's `superseded_by`. Clears the session's `decision_capture`.

**Capture triggers (spec §8.6) — never their own Stop turn:**
- `claim_done` result ends with: `Log any decisions this task settled (log_decision): what, why, alternatives.` — only if no decision is logged for that task yet.
- `handoff_note` result ends with the same line (clean session end goes through the handoff note).
- PreCompact sets `decision_capture`; the SessionStart `compact` re-briefing then shows this session's decisions (up to 5, `D3 <what>`) and, when the flag is set, the ask line; appending `decision_asked`. If another note goes out first (any channel), it gets the ask as one extra line and appends `decision_asked` — one ask per flag.

**Queries / CLI:** `decision::list(conn, repo, task?, include_superseded) -> Vec<Decision>`; `decision::chain(conn, id) -> Vec<Decision>` (oldest → newest). CLI `bridge-screen decisions [-C DIR] [--task ID] [--all]` prints `D7 2.3 <what> — <why> (alternatives: …) [supersedes D4]`; superseded ones only with `--all`. MCP `status()` adds `Decisions on <task>: N (latest: D7 <what>)`.

- [ ] Failing tests: `decision_chain_rules` (self, other repo, already-superseded refused; chain D1→D4→D7 read back in order); duplicate merges into one row with union of alternatives and rebuild gives the same rows; ids are sequential per repo and unique under two concurrent MCP writers (store concurrency pattern); claim_done/handoff_note results carry the ask only when no decision exists for the task; precompact → compact re-brief contains the decisions and the ask exactly once; CLI `decisions` output with and without `--all`.
- [ ] Implement; commit `core: decisions with supersede chains and merging; mcp: log_decision; cli: decisions`.

---

### Task 7: Status, skill, docs and latency

- CLI `status`: per project line adds `done 41% · claimed 12%` (and `· N unestimated` when any); needs-you gains verify-overdue items (order: after checkpoint missed and unstable sessions, before idle); per session `verify pending: 2.3` when it has an unverified claim.
- `plan show`: verified levels shown as now (`[l2]`), plus `stale` when the row is stale and `failed` as is; `✓owner` marker for L4.
- MCP `status()`: active task's level and the next ladder step ("next: run tests, verify level 2"), unverified tasks of this session, decisions line (Task 6).
- `SKILL.md`: `claim_done` with files/symbols; climb the ladder honestly (L2 = tests pass and are committed; never report a failing exit code; L3 = what you drove; L4 only with the owner's words); log decisions at task completion and before compaction (supersede, don't contradict); respond to dependency-ready notes. Keep within the skill's line-limit test.
- `CLAUDE.md`: layout (`verify/`, `decision/`), new commands (`verify`, `decisions`, `config set|unset`). `README.md`: verification ladder and decisions.
- Latency: re-run the ignored latency tests; UserPromptSubmit with nudge/dep-ready candidates stays within 50 ms p95; SessionStart staleness with 50 verifications recorded in the latency output.
- `claude plugin validate ./plugin` passes; `cargo test --workspace`, clippy, fmt, web lint/build pass.
- [ ] Commit `docs: verification and decisions in status, skill and docs; latency recheck`.
