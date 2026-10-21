# Intent — bridge-screen

**Purpose:** The owner builds 2-day to 2-month software projects almost entirely through Claude Code, often with several agent sessions running in parallel. bridge-screen lets them see, at a glance and without reading code or long agent messages, how close each project is to its goal, whether each session is still doing what it was asked, and what crashed or abandoned sessions left behind.

**Done looks like:**
- The owner no longer keeps a roadmap file by hand.
- One glance (terminal, browser or the Claude desktop app) answers those three questions in under 5 seconds.
- A new session picks up a crashed session's work without the owner re-explaining it.
- It works on any repo the owner opens, including half-built ones, without hand-seeding the plan.
- All nine sub-projects of the design spec have shipped.

**Not doing:**
- No cloud service, accounts or team features: one person, one machine.
- No background process; nothing runs between sessions.
- No blocking of agents' work (the one exception: a checkpoint commit before stopping at a task boundary).
- No AI model calls from bridge-screen itself; judgement comes from the agents.
- Nothing published anywhere unless the owner asks for it explicitly.

**Hard constraints:**
- Everything stays local and private; nothing is written into a repo except the checkpoint commits the owner approved.
- Never lose work: undo always stashes first, squash always keeps a backup.
- bridge-screen must never slow down or break a Claude Code session (hooks under 50 ms, always succeed).
- Licence: PolyForm Strict 1.0.0 (noncommercial use; no redistribution or derivative works by others).

**Current focus:** Make the dashboard readable and alive (task 6.4), then make bridge-screen's own reports follow the lead-briefing shape, then plug-and-play onboarding for existing repos (sub-project 8).

Drafted by Claude from the design spec and the owner's decisions, 2026-10-21. **Not yet confirmed by the owner** — please correct anything here; agents never change this page without you.
