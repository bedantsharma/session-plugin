# Decision log — bridge-screen

Choices someone will later ask "why?" about. Newest last. Entries are never deleted; a changed decision is marked "Superseded by #N". Smaller design decisions live in the spec (`docs/superpowers/specs/2026-10-13-bridge-screen-design.md` §2) and the sub-project plans.

## #1 One local program plus a Claude Code plugin, no background service — Accepted — 2026-10-13
**Context:** The tracker must see every session without the owner running anything, and must stay private.
**Decision:** We will ship one Rust program that Claude Code calls on each event, plus a plugin that wires it in; nothing runs between sessions.
**Consequences:** Private and simple to install; but anything "live" only updates when a session or a screen is open.

## #2 An append-only event log, with views rebuilt from it — Accepted — 2026-10-13
**Context:** Progress, drift and recovery all need a trustworthy history, and bugs in the derived picture must be fixable.
**Decision:** We will record every change as an event that is never edited, and compute everything else from the events.
**Consequences:** Any view can be rebuilt after a fix; storage grows (archiving is planned in sub-project 9).

## #3 Guide agents, never block them — Accepted — 2026-10-13
**Context:** A tracker that stops agents would get turned off.
**Decision:** bridge-screen only adds short notes to the agent's context; the single block is asking for a checkpoint commit before stopping at a task boundary.
**Consequences:** Agents can still drift; the owner sees it and decides.

## #4 Plans in pseudocode; one agent builds, a second reviews and fixes — Accepted — 2026-10-13
**Context:** Full-code plans were too long; per-task agents were costly.
**Decision:** Each sub-project gets a short pseudocode plan, one implementer agent, then one reviewer agent that fixes what it finds.
**Consequences:** Cheaper and faster; the review pass has caught 9–12 real bugs per sub-project.

## #5 "Done" counts only work verified by tests or better — Accepted — 2026-10-14
**Context:** Agents say "done" often; the owner needs a number they can trust.
**Decision:** Progress counts tasks verified at L2 (tests pass at a recorded commit) or above; claimed work is shown separately.
**Consequences:** Honest progress, but work done before tracking started reads 0% until seeded.

## #6 Ad-hoc "waiting on" dependencies deferred to sub-project 8 — Accepted — 2026-10-14
**Context:** The spec doesn't assign it; sub-project 5 was already large.
**Decision:** Only plan-level dependencies get "ready" notes for now.
**Consequences:** Tracked as task 8.1.

## #7 A test command that can't run is not a failed test — Accepted — 2026-10-14
**Context:** A typo in the test command marked every task failed.
**Decision:** "Command not found / not executable" records the agent's result as reported and asks the owner to fix the command.
**Consequences:** No false failures; a broken command is visible instead.

## #8 Licence: PolyForm Strict 1.0.0 — Accepted — 2026-10-14
**Context:** The owner wants to share it with friends but keep control.
**Decision:** Noncommercial use only; nobody else may modify or redistribute it.
**Consequences:** Friends can use it for personal projects, not at work; outside contributions would need separate agreements; homebrew-core is closed.

## #9 Needs-you holds only things you can act on — Accepted — 2026-10-14
**Context:** Idle sessions in the queue would make the red count meaningless.
**Decision:** The count includes drift decisions, sign-offs, leftovers, questions and permission prompts. Finished sessions show "done", others "idle", neither counted.
**Consequences:** "All clear" means nobody is blocked on you.

## #10 Brief alerts, an optional reason on Continue — Accepted — 2026-10-14
**Context:** Constant blinking breaks accessibility rules and gets ignored; reasons help later but slow every click.
**Decision:** The count pulses briefly when it crosses 4; Continue takes an optional reason that must be a real one if given.
**Consequences:** Calm screen; better records when you want them.

## #11 The agent never sees the dashboard's access token — Accepted — 2026-10-14
**Context:** With the token an agent could click your needs-you buttons for you.
**Decision:** Only `bridge-screen ui` prints the full link; the desktop card's buttons need a one-time code.
**Consequences:** You open the dashboard from your terminal, not from an agent's message.

## #12 Readability before "fits on one screen"; motion with a pause button — Accepted — 2026-10-15
**Context:** The first dashboard was cramped and lifeless.
**Decision:** 15 px rows, real spacing, cards; live motion only where it means something, stoppable by a header toggle or the system's reduced-motion setting.
**Consequences:** The glance may scroll a little; it feels alive without being noisy.

## #13 Hill chart words: "exploring" and "executing" — Accepted — 2026-10-15
**Context:** "Downhill" reads as "getting worse".
**Decision:** Screens say exploring (unknowns remain, % may change) and executing (path clear, % reliable).
**Consequences:** Agents still use uphill/downhill when editing the plan.

## #14 This repo's progress was seeded by hand — Accepted — 2026-10-15
**Context:** Sub-projects 1–3 finished before the plan had tasks; nothing had been marked done.
**Decision:** Added one task per finished sub-project and the owner's sign-off (L4) on all completed tasks; added estimated tasks for the remaining work.
**Consequences:** The % is approximate (estimates are rough); real fix is automatic import (sub-project 8).

## #15 Reports to the owner follow the lead-briefing shape — Accepted — 2026-10-21
**Context:** The owner leads the project but doesn't read the code, and found this shape easiest to follow.
**Decision:** Every report from Claude or a subagent leads with the effect, separates checked from believed, and puts technical details last; five living docs in `docs/lead/` are kept current. bridge-screen's own messages will follow the same shape next.
**Consequences:** Rules in `CLAUDE.md`; subagent prompts must carry them.

## #16 No forecast dates below 3 finished tasks — Accepted — 2026-10-21
**Context:** The spec asked for a "wide default distribution" when there is little history; that would put invented dates on screen.
**Decision:** With fewer than 3 verified tasks (or no recent activity) screens show the reason instead of dates ("need 2 more"). From 3 to 5 the forecast uses every model's history together; a model gets its own from 6.
**Consequences:** New projects show no dates for a while; the dates that do appear rest on real finished work.

## #17 Cost is computed when you look, from config prices — Accepted — 2026-10-21
**Context:** Prices change and defaults can be wrong; stored dollar amounts would freeze mistakes.
**Decision:** Only token counts are recorded; every screen prices them from `config.toml` at read time. Defaults come from the claude-api pricing reference (taken 2026-10-21); a model with no price shows "unknown", never $0. Haiku 5.5 is left out (its price changes above 100K-token prompts), and fast-mode replies are kept apart as "<model> (fast)" with no default price (the reference gives no fast cache rates), so neither is ever shown at a wrong rate.
**Consequences:** Fixing a price fixes all past costs. Costs for unpriced models (Haiku 5.5, fast mode) stay "unknown" — or make a total read "≥ $X" — until you add a price.
