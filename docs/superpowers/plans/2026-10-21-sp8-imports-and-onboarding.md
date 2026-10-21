# Sub-project 8: Imports and Onboarding — Implementation Plan

> **For agentic workers:** Execute task-by-task, test-first. Steps use checkbox (`- [ ]`) syntax. This plan gives interfaces, behaviour rules (pseudocode) and test cases — no full code. Read the cited spec sections and the existing code first and follow its patterns: events → projector → views (`Store::write`, `append_in`, `VIEW_TABLES`, schema migrations), `plan_edit_in` for every plan change, hook handlers + the never-fail wrapper, MCP tools with `Args<T>`, `owner::act` for owner actions, the `view` read models every screen renders, and `words.rs` / `web/src/lib/words.ts` / `jargon.json` for owner-facing wording. Add dependencies only with `cargo add` / `npm install` (prefer none: `sha2`, `jiff`, `ulid` are already there).

**Goal:** bridge-screen works on any repo the owner opens without hand-seeding: it reads the repo's plan files by itself and keeps up with their changes, an agent can draft a plan for a half-built repo that the owner confirms once, the same branch across repos shows as one group, a task can wait on a task in another repo, and an agent can say "I'm waiting on X" and hear when X is done.

**Architecture:**
- **Import (new core module `import`):** a pure parser turns a superpowers plan file into a `ParsedPlan`; a diff against what the last import of that file produced becomes ordinary plan events through `plan_edit_in` (with a `source` on each node), grouped under `replan` when more than 30% of the tasks changed. SessionStart checks the plan folder cheaply (size + mtime, then hash) and imports new or changed files; `bridge-screen import` does the same by hand. Files are never written.
- **Init (new core module `init`):** `bridge-screen init` prints a read-only evidence summary of the repo for the agent; the agent drafts the plan, shows it to the owner once, and applies it with the new `init_plan` MCP tool, which creates the tree in one batch and records tasks it judged done as claimed with evidence, so they reach L1 by the existing check.
- **Dependencies (new core module `waits`, plus `plan`):** plan links may point at a task in another repo; ad-hoc waits (`waiting_on`) clear when their target reaches L2+, and the waiting session gets the same "done on `<branch>` @sha" note the plan links already use.
- **Branch groups (`view/groups.rs`):** each task remembers the branch it was claimed on; sessions with the same non-default branch in two or more repos form a group with a combined done%.

**Spec:** `docs/superpowers/specs/2026-10-13-bridge-screen-design.md` — §5.2 (plan events, `waiting_on`, `wait_cleared`), §5.3 (`branch`, `source`, `depends_on`), §9.1 (dependencies), §10.1 (branch groups, amber "waiting", wait target), §10.2 (project drill: dependencies), §11.2 (`waiting_on`), §12.1 (superpowers import), §12.2 (existing half-built repos), §13 item 8. The SP5 plan's "Not in this sub-project" section describes the deferred waits.

**Branch / worktree:** `feat/sp8-imports-onboarding` in `/Users/bedantsharma/PycharmProjects/session-plugin-sp8` (from `main`). Work only there. Another session is changing `navigator.rs` and `session/project.rs` (scoring units cut short by a new message); if it merges first, rebase and keep both changes.

## Facts about plan files (checked 2026-10-21)

- **Superpowers 7.0.0 template:** title `# <Feature> Implementation Plan`; `**Goal:** <one sentence>`; `### Task N: <name>`; a `**Files:**` list of `- Create: \`path\``, `- Modify: \`path:12-40\``, `- Test: \`path\``; steps `- [ ] **Step k: <text>**` (checked: `- [x]`). 6.4.1 is the same shape.
- **This repo's own plans differ:** tasks are `### Task N: <name>`, but steps are plain `- [ ] <text>` lines, often with no `**Files:**` list, and the title is `# Sub-project N: … — Implementation Plan`. The parser must accept both.
- **This repo already has a hand-made plan** (milestones SP1–SP9 and the polish round) and nine plan files. Importing them naively would add nine duplicate milestones.

## Global Constraints

- Everything from sub-projects 1–7 and the polish round still applies:
  - clippy pedantic `-D warnings`; no `unsafe` / `set_var`;
  - hooks never fail a session and stay ≤ 50 ms p95;
  - tests never touch the real `~/.claude` / `~/.bridge-screen` or run the real `claude`;
  - commit per task ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; never `--no-verify`; chain shell steps with `&&`.
- **Never write to a plan file, spec or any repo file** (spec §12.1, §6.4). Import and init only read.
- **No new git in hooks.** SessionStart already runs git; the import check there uses file reads only (`read_dir`, `metadata`, file contents). `bridge-screen init` (CLI, owner- or agent-run) may run `git log` through the scrubbed helper.
- **SessionStart import budget:** at most 20 plan files and 1 MiB read per SessionStart; files whose size and mtime match the last import are skipped without reading. Anything over the budget waits for the next SessionStart or `bridge-screen import`.
- **A checked step is `claimed_done`, never `verified`** (spec §12.1). Inferred-done tasks from init start at L1 (the existing L1 check), never higher.
- **Import never removes work in progress:** a task removed from the file is `node_removed` only if it is `todo`; a task that is active, claimed or verified is kept and gets a "no longer in the plan file" reason recorded on a `node_edited` (no title change), so the owner decides.
- **Owner-facing wording** follows the lead-briefing rules in `CLAUDE.md` (effect first, plain words, no raw codes). New words go into `words.rs` / `words.ts`, new owner-facing messages into `docs/lead/message-audit.md`, new terms ("branch group", "waiting on", "imported plan") into `docs/lead/glossary.md`.

## Decisions this plan takes (owner can overrule; log them in `docs/lead/decision-log.md`)

1. **First import in a repo that already has a plan records a baseline, not nodes.** If a repo has any plan node and has never imported, the first scan records every existing plan file as `plan_file_seen { path, hash, baseline: true }` and creates nothing. Files that appear or change after that are imported. `bridge-screen import --into <milestone> <file>` links a file to an existing milestone by hand (tasks matched by title). Why: this repo, and any repo planned by hand, would otherwise get duplicates.
2. **Free-text waits are allowed** ("waiting on Sam's API key"), but only task targets clear by themselves; free-text waits are cleared by the agent (`clear_wait`) or the owner.
3. **A task's branch is the branch of the session that last claimed it** (no new field to fill in by hand). Branch groups ignore the repo's default branch (`main`, `master`, or what `refs/remotes/origin/HEAD` points to, read as a file).
4. **Cross-repo targets are written `<repo name>:<task id>`** (repo name = the `repos.name` folder name). An ambiguous name is refused with the repo roots listed.

## Review Focus

1. **Re-importing the same file changes nothing:** same hash → no events; same content after an mtime change → no events. Test: `reimport_is_a_no_op` (Task 3).
2. **A repo with a hand-made plan gets no duplicates** on first sight of its plan folder (decision 1), and still imports a plan file added afterwards. Test: `existing_plan_gets_a_baseline` (Task 3).
3. **Odd plan files don't break SessionStart:** an empty file, a file with no `### Task` heading, a binary or non-UTF-8 file, duplicate task numbers, a 5 MB file, and a removed plan folder each produce no nodes and no error, and SessionStart still briefs. Test: `odd_plan_files_are_skipped` (Task 2) plus `session_start_survives_bad_plans` (Task 3).
4. **A task with work in progress is never removed by an import** (Global Constraints). Test: `import_keeps_tasks_in_progress` (Task 3).
5. **Waits clear once and notify once:** a target reaching L2 clears every open wait on it (same and other repos) in the same transaction as the verification, the note goes out once per wait, a later stale drop does not reopen it, and rebuild gives the same waits. Test: `wait_clears_once_and_notes_once` (Task 6).

---

## File Structure

```
crates/bridge-screen-core/src/
  store/schema.rs            # V10 migration
  projection.rs              # register IMPORT_KINDS, WAIT_KINDS; VIEW_TABLES += new views
  plan/project.rs            # NodeAdded.source; DepChange.dep_repo_id; branch at task_claimed
  plan/ops.rs                # AddNode.source (internal), AddDep/RemoveDep repo, cross-repo cycle check
  plan/query.rs, model.rs    # PlanNode.source, branch, external_deps
  import/mod.rs              # scan(), import_file(), ImportOutcome
  import/parse.rs            # parse(text) -> ParsedPlan (pure)
  import/diff.rs             # diff(previous, parsed) -> Vec<PlanOp> + changed ratio (pure)
  import/project.rs          # plan_imported, plan_file_seen → plan_sources, plan_source_items
  import/tests.rs
  init.rs                    # evidence(dir) -> Evidence; apply(tx, draft) for init_plan
  checkpoint.rs              # claimed_done_draft() split out of claim_done_in
  waits/mod.rs, project.rs, tests.rs   # open/clear/promote waits, the waits view, clear_for()
  verify/ladder.rs           # call waits::clear_for after any L2+ verified
  navigator.rs               # dependency_ready: other repos + cleared waits
  handoff/briefing.rs        # Dependencies line: external deps and waits; no-plan line
  view/groups.rs             # branch groups for the glance
  view/mod.rs, view/drill.rs # waiting light/word; groups; drill: source, external deps, waits
  owner.rs                   # promote_wait, clear_wait owner actions
crates/bridge-screen/src/
  cli.rs, commands.rs        # `import`, `init`, `wait list|clear|promote`
  hook/handlers.rs           # SessionStart: import check in start_steps
  mcp.rs                     # waiting_on, clear_wait, promote_wait, init_plan; claim asks for an estimate
  status.rs, watch.rs, words.rs
plugin/skills/bridge-screen/SKILL.md, plugin/commands/bridge-screen.md
web/src/lib/glance/{BranchGroups,SessionRow,ProjectRow}.svelte, lib/drill/{Project,Session}.svelte, lib/words.ts, lib/types.ts, lib/jargon.json
docs/lead/{current-state,decision-log,glossary,message-audit,project-map}.md
```

---

### Task 1: Schema V10 and the plan's new fields

**Views (V10; `SCHEMA_VERSION = 10`):**
- `plan_nodes` gains `branch TEXT` (the `source` column exists since V1, default `'manual'`; it now holds `manual`, `init`, or JSON `{"file": "<repo-relative path>", "task": N, "step": k?}`).
- `plan_deps` is rebuilt with `dep_repo_id TEXT NOT NULL` and PK `(repo_id, node_id, dep_repo_id, depends_on)`; the migration copies rows with `dep_repo_id = repo_id`.
- `plan_sources(repo_id, path, hash, size, mtime, node_id NULL, baseline INTEGER, imported_at)`, PK `(repo_id, path)`.
- `plan_source_items(repo_id, path, task_no, step_no NULL, node_id, title)`, PK `(repo_id, path, task_no, step_no)` (`step_no` stored as 0 for a task row).
- `waits(wait_id PK, repo_id, session_id, target_repo_id NULL, target_task NULL, target_text, reason, opened_at, cleared_at NULL, cleared_why NULL, cleared_sha NULL)`, index `(target_repo_id, target_task, cleared_at)`.

**Payloads:**
- `NodeAdded` gains `#[serde(default)] source: Option<NodeSource>`; `NodeSource = Manual | Init | File { file, task, step? }`, serialised as above. Old events read as manual.
- `DepChange` gains `#[serde(default)] dep_repo_id: Option<String>` (None = same repo).
- `task_claimed`'s projector sets `plan_nodes.branch` to the claiming session's `sessions.branch` (deterministic on rebuild: sessions are projected first).
- `PlanNode` gains `source: NodeSource`, `branch: Option<String>`, `external_deps: Vec<ExternalDep { repo_id, repo_name, id }>`; `depends_on` keeps same-repo ids only.

- [ ] Failing tests:
  - V9→V10 migration keeps every plan dep and node; old `node_added` events without `source` load as manual.
  - `task_claimed` records the session's branch on the node; a later claim from another branch replaces it.
  - Rebuild equality (`tests/rebuild.rs`) still holds with the new columns.
- [ ] Implement; commit `core: schema v10 — plan sources, task branch, cross-repo deps, waits`.

---

### Task 2: Parsing a plan file

`import::parse::parse(text: &str) -> ParsedPlan` — pure, no I/O.

```
ParsedPlan { title: Option<String>, goal: Option<String>, tasks: Vec<ParsedTask>, warnings: Vec<String> }
ParsedTask { number: u32, title: String, files: Vec<String>, steps: Vec<ParsedStep> }
ParsedStep { number: u32, text: String, checked: bool }

title  = first "# " heading, minus a trailing " Implementation Plan" / " — Implementation Plan"
goal   = text after "**Goal:**" up to the end of that paragraph
task   = "### Task N: <name>" (N a positive integer; "Task N." and "Task N —" accepted)
files  = under the task's "**Files:**" line: "- Create|Modify|Test: `path`" → path without ":lines"
steps  = "- [ ] " / "- [x] " lines inside the task, outside fenced code blocks:
           "**Step k: text**" → number k, text; otherwise number = position, text = line text
           text cut to 200 characters, single line
ignore everything inside ``` fences (code blocks hold checkbox-looking lines)
duplicate task number → keep the first, add a warning; a task with no title → skip, warn
```

- [ ] Failing tests (`import/tests.rs`, fixtures as string literals):
  - `parses_superpowers_template`: title, goal, two tasks with files (line ranges stripped) and `**Step k:**` steps, one checked.
  - `parses_this_repos_plan_style`: plain `- [ ]` steps, no Files list, `# Sub-project 7: … — Implementation Plan` title.
  - `fenced_checkboxes_are_not_steps`.
  - `odd_plan_files_are_skipped`: empty text, no task headings, duplicate numbers, a 5 MB string → no panic, tasks as expected, warnings present.
- [ ] Implement; commit `core: superpowers plan parser`.

---

### Task 3: Import and diff

**Shared claim helper (`checkpoint.rs`):** split `claim_done_in` so its `claimed_done` event comes from `pub fn claimed_done_draft(conn, repo_id, session_id, actor, task, evidence, files, symbols) -> Result<EventDraft>` (the same validation and evidence trimming). `claim_done_in` keeps adding `checkpoint_due` and the decision ask; import and `init_plan` use only the draft — no checkpoint request and no decision ask, because nothing was just finished in this session.

**Types:** `SourceItem { task_no, step_no: Option<u32>, node_id, title, status: NodeStatus }` (a `plan_source_items` row joined with its node's status); `ImportCtx { session_id, actor, dry_run }` (`session_id` = the SessionStart session, or `owner` from the CLI); `ScanBudget { files: usize, bytes: u64 }`; `ImportOutcome = Imported { path, node_id, added, edited, removed, kept, claimed, replan } | Unchanged { path } | Baseline { path } | Skipped { path, why }`; `ImportDiff { ops: Vec<PlanOp>, claims: Vec<ClaimRef>, keeps: Vec<(String, String)>, changed_ratio: f64, replan: bool }`, where `ClaimRef` names a parsed task/step (its node id is known only after the ops apply). `repo` arguments are the existing `RepoInfo`.

**Diff — `import::diff::diff(prev: &[SourceItem], parsed: &ParsedPlan, target: &ImportTarget) -> ImportDiff`** (pure). `prev` is the file's `plan_source_items`; tasks match by number, and by title when the number moved.

```
ImportTarget = NewGoal | NewMilestone { goal_id } | Existing { node_id }   // decided by import_file
for each parsed task:
  no match              → AddNode task (title, expected_files = files, source File{file,task})
                          + AddNode subtask per step (source File{file,task,step})
  title changed         → EditNode title
  files changed         → EditNode expected_files
  steps added/removed   → AddNode / RemoveNode subtask (same in-progress rule as tasks)
  step newly checked    → a ClaimRef     (applied as claimed_done; never verified)
for each prev task with no parsed match:
  node todo             → RemoveNode
  node active/claimed/L1+ → Keep { node, reason: "no longer in <file>" }
goal text changed       → SetIntent on the target node
changed_ratio = tasks added + removed + retitled  ÷  max(prev tasks, parsed tasks)
replan = changed_ratio > 0.30 and prev is not empty
```

**Import — `import::import_file(tx, repo: &RepoInfo, path: &Path, ctx: &ImportCtx) -> Result<ImportOutcome>`:**
```
read file (≤ 1 MiB, UTF-8 lossy refused → skipped with warning); hash = sha256
same hash as plan_sources → Unchanged (only refresh size/mtime via plan_file_seen)
parsed has no tasks → Skipped { why }
target:
  file already linked (plan_sources.node_id)  → Existing { that node }
  repo has no goal                            → NewGoal (goal = title, intent = goal or "Imported from <file>")
  otherwise                                   → NewMilestone under the goal (intent = goal text or "Imported from <file>")
ops = diff(…); apply with plan_edit_in (reason "imported from <file>", replan when diff says so)
claims → claimed_done_draft with evidence "ticked in <file>" and no files (so no L1 check; it stays claimed)
append plan_imported { path, hash, size, mtime, node_id, tasks, changed_ratio, replan }
```
`plan_imported` / `plan_file_seen { path, hash, size, mtime, baseline }` project into `plan_sources`; `plan_source_items` is maintained by the import projector from `node_added` events whose `source` is a file (so rebuild works without re-reading files).

**Scan — `import::scan(store, repo, session_id, budget: ScanBudget) -> Result<Vec<ImportOutcome>>`:**
```
dir = <repo root>/docs/superpowers/plans (missing → nothing)
files = *.md sorted by name, at most budget.files (20)
repo has plan nodes and no plan_sources rows → baseline: plan_file_seen{baseline:true} for each, no nodes
else for each file: stat; size+mtime equal to plan_sources → skip unread; else import_file (stop at budget.bytes, 1 MiB)
```

**Callers:**
- SessionStart: `start_steps` runs `import::scan` before recovery (not after a compaction); errors are logged and the briefing still goes out. The briefing's summary line mentions "Imported <file>: N tasks" once, for files imported in this SessionStart.
- CLI `bridge-screen import [-C DIR] [FILE] [--into <node>] [--dry-run]`: no FILE → scan without the 20-file limit; `--into` links FILE to an existing goal/milestone (tasks matched by title, case-insensitive) before importing; `--dry-run` prints the ops without writing. Output in plain words ("Added 6 tasks from sp7 plan; 2 steps were already ticked, recorded as claimed").

- [ ] Failing tests (core + `crates/bridge-screen/tests/import.rs` with a temp repo):
  - `first_import_creates_goal_then_milestones`: first file → goal with its tasks; second file → milestone under the goal.
  - `reimport_is_a_no_op` (Review Focus 1).
  - `edits_become_plan_events`: retitle, new task, new step, newly checked step → `node_edited`, `node_added`, `claimed_done`; nothing `verified`, no `checkpoint_due`.
  - `big_change_is_a_replan`: 4 of 10 tasks changed → one `replan` groups the batch.
  - `import_keeps_tasks_in_progress` (Review Focus 4).
  - `existing_plan_gets_a_baseline` (Review Focus 2).
  - `into_links_a_file_to_a_milestone`: tasks matched by title get the file as source; unmatched ones are added.
  - `session_start_survives_bad_plans`: SessionStart with odd files and an unreadable folder still prints a briefing; errors go to the hook log.
  - The file on disk is byte-identical after every test.
- [ ] Implement; commit `feat: import superpowers plans at SessionStart and with bridge-screen import`.

---

### Task 4: `bridge-screen init` for half-built repos

**Evidence — `init::evidence(dir: &Path) -> Result<Evidence>`** (CLI only, read-only):
```
Evidence {
  readme: first heading + first paragraph of README* (≤ 400 chars),
  docs: up to 30 paths under docs/ (names only),
  plans: plan files (and whether each is already imported), specs: docs/superpowers/specs/*.md,
  git: commit count, first and last commit dates, last 15 subject lines (scrubbed git helper),
  todos: count of TODO/FIXME in tracked text files (`git grep -c`), top 10 files,
  test_command: the repo's configured one, else a guess from Cargo.toml / package.json / pyproject.toml (shown as a guess),
  has_plan: whether the repo already has a goal
}
```
`bridge-screen init [-C DIR] [--goal "<text>"]` prints the evidence and the steps for the agent: draft goal (with intent), milestones (with intents) and tasks with estimates; mark tasks that look done with the files that show it; show the draft to the owner once; then call `init_plan`. If plan files exist and aren't imported, it says to run `bridge-screen import` first. If the repo already has a goal, it says so and stops.

**MCP tool `init_plan`:**
```
input  { goal: { title, intent }, milestones: [{ title, intent, tasks: [Task] }], tasks: [Task],   // tasks directly under the goal allowed
         owner_said: String  /* the owner's words confirming the draft, ≤ 200 chars, required */ }
Task   { title, estimate_min?, expected_files: [], done: Option<{ evidence, files, symbols }> }
refuse when the repo already has a goal ("this repo already has a plan; use plan_edit")
one write: plan_edit_in AddNode ops (source Init), then for each done task claimed_done_draft
           with its files, then verify::l1::check_l1 at the session's worktree HEAD and its
           l1_draft / verified_draft, as claim_done's MCP path does today (no checkpoint request)
reply: "Plan created: 3 milestones, 14 tasks; 5 look done (checked: 4 exist at HEAD)."
```
- The SessionStart briefing's no-plan line becomes: "No plan yet. If this repo has work already, run `bridge-screen init` and follow it; otherwise ask the owner what the goal is." (and the import outcome when plan files were found).
- `SKILL.md` gets a short "Starting on a repo with no plan" section (keep the skill under ~150 lines); `commands/bridge-screen.md` mentions `/bridge-screen init` → run `bridge-screen init`.
- `claim_task` on a task with no estimate replies with one extra line asking for one (`plan_edit` set_estimate), spec §12.1 "filled in by the agent at its first claim" — only if not already done.

- [ ] Failing tests:
  - `evidence_reads_without_writing`: temp repo with README, docs, a TODO, 3 commits → fields filled; the repo's status is unchanged.
  - `init_plan_builds_the_tree_and_l1`: done task with an existing file reaches L1, one with a missing file stays claimed; non-done tasks are todo; sources are `init`.
  - `init_plan_refuses_an_existing_plan` and `init_plan_needs_owner_words`.
  - `claim_without_estimate_asks_once`.
- [ ] Implement; commit `feat: bridge-screen init and the init_plan tool`.

---

### Task 5: Plan links across repos

- `PlanOp::AddDep` / `RemoveDep` gain `#[serde(default)] repo: Option<String>` — a repo name or id. Resolution: id match, else unique `repos.name` match; none → `Invalid "no repo named X"`; several → `Invalid "X matches several repos: <roots>"`. `plan_edit` and `bridge-screen plan edit` accept it; the MCP `plan_edit` description says "`repo` for a task in another repo".
- **Cycle check across repos:** `would_cycle` follows edges into other repos (`plan_deps` rows with any `dep_repo_id`), bounded by 1000 visited nodes.
- **`navigator::dependency_ready`** walks `depends_on` and `external_deps`; for an external one it reads the verification and branch in the other repo; the note names the repo: "bridge-screen: api:2.4 Login endpoint is done on `feat/auth` @abc1234. Merge or rebase to pick it up." Key `dep:<repo_id>:<id>:<sha>`.
- **Briefing Dependencies line** lists external deps as `<repo>:<id> ready|waiting`.

- [ ] Failing tests:
  - `cross_repo_dep_by_name`, `ambiguous_repo_name_is_refused`.
  - `cross_repo_cycle_is_refused`: A:1.1 → B:2.1 → A:1.1.
  - `external_dep_ready_note`: B:2.1 verified L2 in B's session → A's session gets the note once at its next prompt; the briefing says ready.
- [ ] Implement; commit `feat: plan links to tasks in other repos`.

---

### Task 6: "Waiting on" — ad-hoc dependencies

**Events (`WAIT_KINDS`):** `waiting_on { wait_id, target_repo_id?, target_task?, target_text, reason }`, `wait_cleared { wait_id, why: target_done | agent | owner | promoted, sha? }`.

**MCP tools:**
- `waiting_on(target: String, reason: String)` — target is a task id (`2.4`), `<repo>:<task id>`, or free text. A task target that is already L2+ is refused with "2.4 is already done @sha — merge or rebase to pick it up". Reply: "Waiting on 2.4 Login form (feat/auth). You'll get a note here when it's done." For a task target that isn't already a plan link of the active task, the reply adds the offer (spec §9.1): "If your task will always need it, ask the owner whether to make it a plan link (promote_wait)." The session drill shows the same offer as a button.
- `clear_wait(wait_id)` — the agent's own waits only.
- `promote_wait(wait_id)` — task targets only: appends `dep_added` (the session's active task depends on the target, via `plan_edit_in`) and `wait_cleared { why: promoted }`. Use only when the owner agreed.

**Clearing:** `waits::clear_for(tx, repo_id, task_id, sha) -> Result<Vec<EventDraft>>` — every open wait (any repo) targeting that task gets `wait_cleared { why: target_done, sha }`. Called inside the same transaction right after any `verified` with level ≥ 2 is appended: `verify::ladder::verify` (L2/L3) and `owner_verdict` (L4). A later `verification_stale` never reopens a wait.

**The note:** `dependency_ready` also emits, for each `wait_cleared { target_done }` on this session's waits not yet noted, the same text as a plan link (key `wait:<wait_id>`). Free-text waits never produce it.

**Where it shows:**
- Glance `SessionRow` gains `waiting: Vec<WaitRef { wait_id, target, since_min }>`; a session with an open wait and nothing redder gets the amber light and the word "waiting", and the row shows "waiting on 2.4 Login form" (spec §10.1).
- Briefing Dependencies line lists open waits ("waiting on 2.4 (since 40m)") and ones cleared since the last session.
- `status` (MCP and CLI) lists them; the session drill lists open and cleared waits.
- Owner actions (`owner::act`, stale-click checked): `clear_wait`, `promote_wait`; CLI `bridge-screen wait list|clear <id>|promote <id> [-C DIR]`; the session drill gets the two buttons. A wait is **not** a needs-you item (the session isn't blocked on the owner).

- [ ] Failing tests:
  - `wait_clears_once_and_notes_once` (Review Focus 5), including a waiter in another repo.
  - `free_text_wait_clears_by_hand_only`.
  - `waiting_on_a_done_task_is_refused`.
  - `promote_wait_adds_a_plan_link`: `dep_added` + `wait_cleared{promoted}`; refused for free text.
  - `waiting_session_is_amber`: glance light amber, word "waiting", no needs-you item.
  - MCP contract tests for the three tools; `clear_wait` on another session's wait is a tool error.
- [ ] Implement; commit `feat: waiting_on — ad-hoc dependencies that clear when the target is done`.

---

### Task 7: Branch groups

`view::groups::branch_groups(conn, repos: &[ProjectRow]) -> Result<Vec<BranchGroup>>`:
```
BranchGroup { branch, repos: Vec<GroupRepo { repo_id, name, done_pct, claimed_pct, sessions: usize }>, done_pct, claimed_pct }
branches = branches of live sessions, minus each repo's default branch (main, master, or the
           target of refs/remotes/origin/HEAD read as a file under the repo's git common dir)
group    = a branch seen in ≥ 2 repos
per repo % = done% over that repo's live leaves whose plan_nodes.branch = branch (spec §5.6 formula)
group %    = same formula over the union of those leaves (estimate-weighted)
a repo with no tasks on that branch still appears, with "no tasks on this branch yet" and no %
sorted by branch name
```
`Glance` gains `branch_groups: Vec<BranchGroup>`; `for_repo` keeps groups that include the repo.

- [ ] Failing tests (`view/tests.rs`):
  - `same_branch_in_two_repos_is_a_group` with the combined % weighted by estimate.
  - `default_branch_is_never_a_group` (main in both repos; a repo whose origin HEAD is `trunk`).
  - `one_repo_is_not_a_group`.
- [ ] Implement; commit `core: branch groups on the glance`.

---

### Task 8: Screens and words

- **Dashboard:**
  - `BranchGroups.svelte` under the projects: one line per group — mono branch name, the combined bar (same clay/claimed styling as `ProgressBar`), "2 repos" and each repo's % in a tooltip. Hidden when there are no groups.
  - `SessionRow.svelte`: amber "Waiting" chip (new `sessionWord` case) and "waiting on <target>" in the task line.
  - Project drill: each task shows where it came from ("from sp7 plan, task 3", "drafted at setup", nothing for manual), its branch, and external deps as `<repo>:<id>` with ready/waiting.
  - Session drill: open and cleared waits, with "Clear" and "Make it a plan link" buttons (owner actions).
- **`status` / `watch`:** a "branch groups" line per group (`feat/auth · 2 repos · 45%`); session lines say "waiting on 2.4"; `watch` shows groups under projects.
- **Desktop card:** session lines say "waiting on …"; groups are not shown on the card.
- **Words:** every new owner-facing message goes through `words.rs` / `words.ts`, is listed in `message-audit.md`, and no new code reaches a screen (`jargon.json` gains `waiting_on`, `wait_cleared`, `plan_imported`, `plan_file_seen`, `init_plan`).
- [ ] Failing tests: vitest for `BranchGroups` (renders, hides when empty), `SessionRow` waiting chip, project drill source text; Rust `status` and `watch` snapshot tests; the jargon test.
- [ ] Implement; `npm run lint && npm run check && npm run test && npm run build`; commit `feat: screens show branch groups, waits and where each task came from`.

---

### Task 9: Latency, rebuild, docs and the full check

- Extend the release `hook_latency` test: SessionStart with 20 unchanged plan files (stat only) and with one changed 300 KiB plan file — both within the existing SessionStart budget (150 ms); UserPromptSubmit with 20 open waits and 5 external deps — ≤ 50 ms p95.
- Extend `tests/rebuild.rs`: imports (including a replan and a baseline), init, waits (open, cleared, promoted) and task branches rebuild identically.
- Update `CLAUDE.md` (Layout: `import/`, `init.rs`, `waits/`, `view/groups.rs`; Commands: `import`, `init`, `wait`), `docs/lead/project-map.md`, `glossary.md`, `decision-log.md` (the four decisions above), `message-audit.md`, and overwrite `current-state.md`.
- Run everything: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`, the release latency test, `cd web && npm run lint && npm run check && npm run test && npm run build`, `npm run e2e` after `cargo build --release`, and `claude plugin validate ./plugin`.
- **By hand, on a copy of the real data** (`BRIDGE_SCREEN_HOME` pointing at a scratch copy of `~/.bridge-screen`): start a session in this repo → baseline recorded, no duplicate milestones; add a scratch plan file in a temp repo → imported; `bridge-screen init` in a temp half-built repo prints evidence.
- [ ] Commit `test: SP8 latency, rebuild and docs`.

---

## Not in this sub-project

- The archive (sub-project 9), including `archive_search`.
- Writing anything back to plan files, or importing non-superpowers formats (GitHub issues, Linear, TODO.md).
- Showing branch groups on the desktop card.
