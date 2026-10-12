# Problem Brief: Agent Progress Tracker

> **For the agent reading this:** This is a problem brief, not a spec. Your job in this session is to turn it into a **design spec and architecture** — data model, components, hook/MCP wiring, storage layout, dashboard design — and get the owner's approval. **Do not start implementing.** Where this brief marks something "verify", check it against current Claude Code docs before designing around it. Ask the owner when a requirement is ambiguous; don't guess.

Working name: **the tracker** (rename freely).

---

## 1. The problem

The owner builds multi-day to multi-month projects (2 days – 2 months) almost entirely through Claude Code. On these projects he loses track of:

1. **Distance to goal** — how close the project actually is to what it set out to build.
2. **Direction** — whether current work advances the goal or is a tangent.
3. **Leftovers** — what a dead, abandoned, or crashed session left behind.

### Current workaround and why it fails
He keeps a hand-maintained `roadmap.md` that every agent is told to read and update.
- He has to **nag every agent** to update it.
- It is **too dense to read** — high cognitive load, no at-a-glance answer.
- It **drifts from reality** — nothing checks claims against the code.

When he asks an agent "where are we?", he gets long walls of text and still can't tell how close he is or whether the session went off course.

### Concrete failure cases (design must solve these)
- **Tangent swallowing the goal.** Building an app → Docker breaks → the whole session becomes a Docker fix → agent asks him Docker questions → he's thinking "what happened to the thing we started?" The Docker fix was legitimate, but nothing made it visible that the session had left the goal.
- **Broken handoff.** A session didn't finish its task; he closed it; the next session knew nothing, so he re-explained everything. The previous agent's partial changes were left in place and caused conflicts in the app.

---

## 2. Who and how

- **User:** solo developer. Private tool — **nobody else should be able to see it**. May be open-sourced later if good.
- **Primary tool:** Claude Code, mostly in the **Claude desktop app**, sometimes **terminal CLI**.
- **Planning:** starts big projects with a planning session using the **superpowers** skill; agent writes the plan, he iterates on it until satisfied. Superpowers produces `plans/` and `specs/` style folders (verify exact layout and file format).
- **Parallelism:** 1 to N concurrent sessions (don't assume a fixed number), **each in its own git worktree**, each sometimes running 2–3 subagents. No task claiming needed — he assigns tasks manually per session. Sessions sometimes depend on each other's output.
- **Work contexts:** personal projects (usually monorepos) and **office work** (company repos, Jira-style tickets).

---

## 3. Requirements

### 3.1 Plan structure
- Hierarchy: **Goal → Milestones → Tasks → Subtasks**. Small projects may use only Tasks → Subtasks.
- **Import and sync superpowers plan/spec files** if feasible (strongly preferred, not a hard requirement).
- Plan changes are **mostly small edits**; occasionally a deliberate **re-plan event**. Both must be supported and recorded.

### 3.2 Progress
- Progress is a **percentage weighted by effort** (story points), not raw task count.
- **1 story point = a fixed unit of real time** (e.g. 2 hours; exact value to be decided). Agent estimates points per task.
- **Time = agent-active time only, measured in seconds** — time the agent was actually working. Exclude idle time and time waiting on the user. (Not "started Thursday, now Monday = 4 days".)
- Record **estimate vs actual** per task and use history to calibrate future estimates.
- Adding tasks mid-project **may lower the percentage** — that's acceptable and expected.

### 3.3 Tangents
Every session has **one declared active task**. Each unit of work (one prompt → agent stops) gets a label:

| Label | Meaning | Example |
|---|---|---|
| **On-task** | Directly advances the active task | Building the login screen |
| **Detour** | Not the task, but blocks it | Docker broke, must fix to continue |
| **Drift** | Neither advances nor blocks the task | Refactoring an unrelated module |

- **Never block tangents. Never delegate them away** — the same agent continues, since it has the context.
- Detection is two-layered:
  - Agent **self-declares** a switch (e.g. a tool call like `start_detour(reason, blocks: <task>)`).
  - The tracker **cross-checks** at stop time: files touched + work summary vs the active task's scope. Undeclared mismatch → flag as **possible drift**.
- Detours become **child nodes** of the active task with their own time/token accounting, so the dashboard can say "Session 2: 35% of time on Docker detour".
- When a detour closes, the agent is **reminded to return** to the active task.

### 3.4 Session handoff
- **Enforce checkpoint commits** at task/subtask boundaries in each worktree (via hooks). Agents don't commit by default; the owner approves forcing it.
- A new session must **detect half-finished work** (uncommitted changes, incomplete tasks, unverified items) and **resume** from where the previous one stopped, without the owner re-explaining.

### 3.5 Verification
A task claimed "done" must actually be done. Verification levels, in order:

1. **Code exists** — the expected code is present in the expected files.
2. **Unit tests** pass.
3. **End-to-end** — tested by Claude (e.g. via browser tools).
4. **Owner** — he tests it himself and confirms.

- Distinct states such as `claimed`, `verified-by-agent (level N)`, `verified-by-owner`.
- **Avoid a pile of unverified work:** at session start, the agent verifies outstanding unverified items on the spot and updates their state before continuing.

### 3.6 Decisions and history
- Log **every decision**, including decisions **later changed** (keep the link between old and new).
- **Auto-captured**, then de-noised by the agent.
- Capture triggers should be **event-based**, not clock-based: task completion, **before context compaction** (where detail is lost), and session end. (Owner originally suggested every 30 min via a subagent; agreed event-based is cheaper and catches the moments that matter. Verify Claude Code has a pre-compaction hook.)
- History is **hidden by default**, available on demand.

### 3.7 Telemetry
Per task / session / project: tokens, cost, agent-active time, which agent/subagent did the work, retries, number of steps and current step, decisions made and changed. Verify where token/cost data can be obtained (e.g. session transcripts).

### 3.8 Cross-session awareness
- Show **dependencies** between sessions ("session B waiting on session A's X").
- Show **which sessions need the owner's input right now** (verify a notification/waiting hook exists).

### 3.9 Dashboard / UI
- **Glance view (top priority):** % done, current task per session, sessions needing the owner. Nothing else on the first screen.
- Details, history, telemetry: **click to expand** only. Principle: *information at a glance, explanation only when needed.*
- **One dashboard across all projects**, grouped by **ticket/tag** (office: everything under one Jira-style ticket groups together; personal: smarter grouping, but usually one repo = one project).
- Delivery:
  - **Local web dashboard** in the browser.
  - **Compact view** in CLI and in the desktop app (verify what the desktop app can render; realistically compact text + a link).
- **Must never auto-launch.** Hooks only append events to local storage; no session starts a server or opens a browser. The owner opens it on his own terms with one command. Single-instance guard if a server is already running.

### 3.10 Storage and privacy
- **State is never committed** — not to office repos, not to personal repos.
- Lives in a **local folder outside the repo** (e.g. under the home directory, keyed per repo), shared across that repo's worktrees.
- Fully local. No external servers.

### 3.11 Install / integration
- **Drop-in** for both new and existing codebases: Claude Code plugin, MCP server, CLI, or a combination.
- **Zero effort per session**; one command is acceptable.
- **Existing half-built repos:** infer current state by scanning the codebase, optionally guided or seeded by the owner.
- **Claude Code first.** Agent-agnostic is nice-to-have later; don't let it complicate v1.
- Language/stack: owner has no constraint — choose what fits.

---

## 4. v1 scope

v1 must do three things well:

1. **Auto-updating plan state** (with progress %).
2. **Tangent labeling** (on-task / detour / drift).
3. **Session handoff** (checkpoint commits + resume half-done work).

Plus the minimum glance dashboard (% done, current task per session, sessions needing owner). Everything else in section 3 is for the architecture to accommodate, but may ship after v1.

---

## 5. Things to verify before designing

- Current Claude Code hook events and their payloads (session start/end, prompt submit, stop, subagent stop, pre-compaction, notification/waiting-for-input).
- Whether hooks fire identically in the desktop app and the CLI.
- What the desktop app can display from a plugin/MCP tool.
- Where per-session token and cost data can be read.
- Superpowers' plan/spec file format and folder layout.
- Prior art: existing tools solving this, to avoid rebuilding something that exists.

---

## 6. Expected output of this session

A design doc covering: architecture overview, components and their responsibilities, data model (plan tree, events, sessions, decisions, telemetry), storage layout, which hooks/MCP tools do what, tangent detection logic, verification flow, handoff flow, dashboard screens (glance + drill-down), v1 vs later, and open questions for the owner.
