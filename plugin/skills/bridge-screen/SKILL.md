---
name: bridge-screen
description: How to work with bridge-screen, the tracker that records which plan task this session is on, its detours, checkpoints and handoffs between sessions. Use at the start of every session in a tracked repo (read the briefing), before tangent work, when a task is done, whenever bridge-screen injects a note, and before changing the plan.
---

# bridge-screen

bridge-screen tracks this project for the owner: the plan (goal → milestones → tasks →
subtasks), which task each session is on, detours, and agent-active time. Hooks record
what happens; your part is to say **which task** you are on, **declare tangents**, and
keep the plan true. It never blocks your work.

## 0. Read the briefing; read it back

Each session starts with a `bridge-screen briefing — stable|watcher|unstable`. Unless it
is stable, call `handoff_readback` **before your first edit** with the goal, what's in
flight (name the task id) and your next action. If it answers "Close, but the log
says…", carry on from what it says.

## 1. Claim your task first

- bridge-screen lists likely matches in a note. If one **clearly** fits, claim it:
  `claim_task {"id": "1.2", "mode": "auto", "confidence": 0.8}` (honest 0–1 confidence).
- **Unsure** which task, or where the work stands? Ask the owner, then claim with
  `"mode": "confirmed"`.
- **Nothing fits**: claim a new task under the right parent, estimate in agent-minutes:
  `claim_task {"new_task": {"parent": "2", "title": "Rate-limit login", "estimate_min": 30}, "mode": "auto", "confidence": 0.7}`.
- Switching work? Claim the new task (the old one is released). Goals and milestones
  can't be claimed.

## 2. Keep the plan true with `plan_edit`

Use it for new work, wrong estimates, dropped tasks, dependencies, or a milestone going
from uphill (unknowns remain) to downhill. Always give a `reason`; a deliberate re-plan
sets `"replan": true`. Ops apply together or not at all; an error names the failing op
(`op 2: …`) — fix it and resend. Goals and milestones need an intent (2–3 sentences of
purpose and end state). If the repo has no plan, see "Starting on a repo with no plan".

```json
{"ops": [{"op": "add_node", "parent": "1", "kind": "task", "title": "Audit log", "estimate_min": 45},
         {"op": "set_estimate", "id": "1.2", "estimate_min": 20}],
 "reason": "Audit log came up while building login"}
```

### Starting on a repo with no plan

- Plan files (any `*.md` named plan, roadmap, todo or tasks) are imported at SessionStart;
  a ticked step is only claimed. With no ticked step the owner is asked which are finished:
  relay it, **never answer it yourself**.
- A repo with work but no plan: run `bridge-screen init` and follow it. Draft the goal,
  milestones and tasks (with estimates) from its evidence, mark the tasks that look done
  with the files that show it, **show the draft to the owner once**, then call
  `init_plan` with the draft and their words (`owner_said`).
- A brand-new repo: ask the owner what the goal is.

## 3. Detours: tangent work that blocks your task

A detour isn't your task but blocks it (a broken build, a missing fixture). Never hand a
tangent to another agent; declare it and do it:

- **Before** the tangent, `start_detour` with a one-line `reason` and a `resume_cue`
  naming the next action on the work you leave and its file or test:
  `{"reason": "build broken by lockfile", "resume_cue": "rerun tests/login.rs::rejects_empty"}`.
  Write the cue now — it is your only reliable memory of where you were.
- When done, `close_detour` (`"done"` or `"abandoned"`): it returns the task and your cue
  verbatim. Follow it.
- `status` lists suspended items; `resume` goes back to any of them (`"1.2"` or `"D1"`).
- A detour that grows into real work: `promote_detour` with a parent and title.
- Work the **owner asked for** in this session that isn't your task ("also add a
  licence"): `start_detour` with `"requested_by_owner": true` — never drift or away time.

### Waiting on something

Blocked on another task (`2.4`, or `api:2.4` in another repo) or on something outside
(`Sam's API key`)? Call `waiting_on` with the target and a reason, and keep working on what
you can. A task wait clears by itself when the task is verified (L2+) and you get a note;
clear a free-text one with `clear_wait`. Make it a plan link (`promote_wait`) only when the
owner agrees your task always needs it.

## 4. Labels and navigator notes

Each unit is on-task, a detour, `requested` (the owner asked for it this session — not
your own idea), or drift (none of these). When asked, call `label_unit` once with an
honest label and a one-line `why`; drift is information for the owner, not blame. An **amber** note means you've been away from the
task longer than its tolerance: continue only if it still blocks the task. **Red** means
the owner has been flagged; keep going only if essential, and follow any owner answer.

## 5. Done, verified, checkpointed

- Task done? `claim_done {"evidence": "cargo test: 41 passed", "files": ["src/login.rs"],
  "symbols": ["validate_login"]}` (your active task by default). That is L0; the files
  you name are checked at HEAD for L1 once committed.
- Climb the ladder honestly with `verify`. **L2**: your tests pass and the work is
  committed — `{"level": 2, "evidence": "cargo test", "exit_code": 0, "summary": "41
  passed"}`; report the real exit code (a failing one is refused; fix, commit, verify
  again); bridge-screen may re-run the owner's test command. **L3**: say exactly what you
  drove end to end and what you saw. **L4**: only with the owner's own words
  (`owner_said`). A failed re-run sends the task back: fix it and claim it again.
- At a boundary (claim_done, detour closed, task switched) commit a checkpoint before you
  stop. If Stop asks, commit exactly like this (fix and retry if a commit hook fails):

  ```sh
  git commit -m "wip(bridge): 1.2 Login form" -m "Bridge-Task: 1.2
  Bridge-Next: <one-line next action>
  Bridge-Session: <session id>"
  ```

- Stopping with work in flight? Leave `handoff_note {"in_flight": "…", "if_then": ["if X,
  then Y"]}`. WIP commits are squashed automatically before `gh pr create`.
- **Decisions**: when a task is done, before compaction and before you stop, call
  `log_decision {"what": "…", "why": "…", "alternatives": ["…"]}` for what the work
  settled. Changing one? Supersede it (`"supersedes": "D4"`); don't contradict it.
- "X is done on `branch` @sha": a task yours depends on is verified — merge or rebase.

## 6. Leftovers of dead sessions

The briefing lists uncommitted files a crashed or abandoned session left, each with a
suggestion: `resolve_leftover {"ids": ["l3f9a2c1"], "action": "undo"}`. `adopt` commits,
`resume` claims their task (in their worktree only), `undo` stashes with a snapshot —
nothing is lost. Ask the owner when unsure. Report-only leftovers belong to a live
session or to a crashed one elsewhere that may be waiting for its owner: leave them.

## 7. Check where things stand

`status` shows your task, its level and next step, unverified claims, decisions, cost,
handoff and leftovers, detours, the forecast and next tasks. When the owner asks how
things stand, call `show_progress` (its buttons are the owner's: never call
`owner_action`). Reports only on the owner's request: `progress_report`, show the
summary, publish with the Artifact tool once they confirm, then `report_published`.

## 8. Project pages, and reporting to the owner

- The owner leads but doesn't read code. For any status, problem or checklist, use the
  `lead-briefing` skill: effect first in plain words (no codes: "tests pass", not "L2"),
  state, the ask, checked vs believed, details last.
- Each repo can have five project pages, kept outside the repo and shared by every
  worktree: `lead_page_read` / `lead_page_write` (`intent`, `project-map`, `current-state`,
  `decision-log`, `glossary`). Read intent and current-state at the start of work.
- If the briefing says there are none, ask the owner once, then relay their answer:
  `lead_pages {"action": "set_up", "owner_said": "<their words>"}` (or `"decline"`). Never
  without asking. After set-up, follow the next steps it returns.
- Keep them true: **current-state** after each merge or working day (replace it);
  **decision-log** (`"mode": "append"`) for a choice someone will later ask "why?" about;
  **glossary** for each new term; **project-map** when a part is added or changes job.
  The **intent** is the owner's: change it only with their words in `owner_said`.

## Notes from bridge-screen

Hook notes start with `bridge-screen:` and end with the one action expected. Act on
them; don't repeat them back to the owner.
