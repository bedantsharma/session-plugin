---
name: bridge-screen
description: How to work with bridge-screen, the tracker that records which plan task this session is on and how much agent time it took. Use at the start of every session in a tracked repo, whenever bridge-screen injects a note, and before changing the plan.
---

# bridge-screen

bridge-screen tracks this project for the owner: the plan (goal → milestones → tasks →
subtasks), which task each Claude session is working on, and agent-active time. Hooks
record what happens automatically. Your part is to say **which task** you are on and to
keep the plan true. It never blocks your work.

Its tools (MCP server `bridge-screen`): `claim_task`, `plan_edit`, `status`.

## 1. Claim your task first

Before you start work on a request, claim the task it belongs to with `claim_task`.

- bridge-screen lists likely matches in a note (`Likely matches for this request: …`).
  If one **clearly** fits, claim it yourself:
  `claim_task {"id": "1.2", "mode": "auto", "confidence": 0.8}`.
  Confidence is your honest 0–1 estimate that this is the right task.
- If you are **unsure** which task this is, or where the work stands, ask the owner
  first, then claim with `"mode": "confirmed"`.
- If **nothing fits**, claim a new task under the right parent, with an estimate in
  agent-minutes (your own working time, not wall-clock):
  `claim_task {"new_task": {"parent": "2", "title": "Rate-limit login", "estimate_min": 30}, "mode": "auto", "confidence": 0.7}`.
  A new task goes under a milestone (or the goal); a new subtask goes under a task.
- Switching to different work? Claim the new task. The previous one is released.

Claim tasks and subtasks only; goals and milestones can't be claimed.

## 2. Keep the plan true with `plan_edit`

Use `plan_edit` when the plan changes: new work, wrong estimates, dropped tasks,
dependencies, or a milestone moving from uphill (unknowns remain) to downhill.

- Always give a `reason`. A deliberate re-plan of many nodes: set `"replan": true`.
- All ops in one call apply together or not at all. An error names the failing op
  (`op 2: …`); fix that op and send the batch again.
- Goals and milestones need an **intent**: 2–3 sentences of purpose and end state
  ("Why it matters, and what is true when it's done"). Tasks may have one.
- Estimates are agent-minutes.

Example:

```json
{"ops": [
  {"op": "add_node", "parent": "1", "kind": "task", "title": "Audit log", "estimate_min": 45},
  {"op": "set_estimate", "id": "1.2", "estimate_min": 20}
 ],
 "reason": "Audit log came up while building login"}
```

If the repo has no plan yet, ask the owner whether to create one before adding a goal.

## 3. Check where things stand

`status` shows this session's active task, units of work, active minutes, the goal and
the next tasks to do. The owner can run `/bridge-screen` for all live sessions.

## Notes from bridge-screen

Hook notes start with `bridge-screen:`. They are short and end with the one action
expected. Act on them, but don't repeat them back to the owner.
