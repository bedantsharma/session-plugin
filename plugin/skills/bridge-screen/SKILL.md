---
name: bridge-screen
description: How to work with bridge-screen, the tracker that records which plan task this session is on, its detours, checkpoints and handoffs between sessions. Use at the start of every session in a tracked repo (read the briefing), before tangent work, when a task is done, whenever bridge-screen injects a note, and before changing the plan.
---

# bridge-screen

bridge-screen tracks this project for the owner: the plan (goal → milestones → tasks →
subtasks), which task each session is on, detours, and agent-active time. Hooks record
what happens; your part is to say **which task** you are on, **declare tangents**, and
keep the plan true. It never blocks your work.

Tools (MCP server `bridge-screen`): `claim_task`, `plan_edit`, `status`, `start_detour`,
`close_detour`, `resume`, `promote_detour`, `label_unit`, `claim_done`, `handoff_readback`,
`handoff_note`, `resolve_leftover`.

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
- Switching work? Claim the new task; the previous one is released. Goals and
  milestones can't be claimed.

## 2. Keep the plan true with `plan_edit`

Use it for new work, wrong estimates, dropped tasks, dependencies, or a milestone going
from uphill (unknowns remain) to downhill. Always give a `reason`; a deliberate re-plan
sets `"replan": true`. Ops apply together or not at all; an error names the failing op
(`op 2: …`) — fix it and resend. Goals and milestones need an intent (2–3 sentences of
purpose and end state). If the repo has no plan, ask the owner before adding a goal.

```json
{"ops": [{"op": "add_node", "parent": "1", "kind": "task", "title": "Audit log", "estimate_min": 45},
         {"op": "set_estimate", "id": "1.2", "estimate_min": 20}],
 "reason": "Audit log came up while building login"}
```

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

## 4. Labels and navigator notes

Every unit of work is on-task, a detour, or drift (neither advances nor blocks the task).
When asked, call `label_unit` once with an honest label and a one-line `why` — drift is
information for the owner, not blame. An **amber** note means you've been away from the
task longer than its tolerance: continue only if it still blocks the task. **Red** means
the owner has been flagged; keep going only if essential, and follow any owner answer.

## 5. Done, checkpoints and handoff notes

- Task done? `claim_done {"evidence": "cargo test: 41 passed"}` (your active task by
  default). It is level L0 — say what you actually ran.
- At a boundary (claim_done, detour closed, task switched) commit a checkpoint before you
  stop. If Stop asks, commit exactly like this (the repo's commit hooks run; fix and retry
  if one fails):

  ```sh
  git commit -m "wip(bridge): 1.2 Login form" -m "Bridge-Task: 1.2
  Bridge-Next: <one-line next action>
  Bridge-Session: <session id>"
  ```

- Stopping with work in flight (mid-task, or a detour still open)? Leave
  `handoff_note {"in_flight": "…", "if_then": ["if X, then Y"]}` for the next session.
- WIP commits are squashed automatically before `gh pr create`; nothing to do.

## 6. Leftovers of dead sessions

The briefing lists uncommitted files a crashed or abandoned session left, each with a
suggestion. Prefer it: `resolve_leftover {"ids": ["l3f9a2c1"], "action": "undo"}`.
`adopt` commits them, `resume` claims their task (only in their worktree), `undo` stashes
them with a snapshot — nothing is lost. Ask the owner when unsure. Leftovers marked
report-only belong to a live session: leave them alone.

## 7. Check where things stand

`status` shows your task, a pending checkpoint, the read-back state, open leftovers,
detours, suspended items with their cues, units and active minutes, the goal and the next
tasks. The owner can run `/bridge-screen` for all sessions.

## Notes from bridge-screen

Hook notes start with `bridge-screen:`, are short, and end with the one action expected.
Act on them; don't repeat them back to the owner.
