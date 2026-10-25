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

## #18 Asked-for work is declared, not guessed — Accepted — 2026-10-21
**Context:** Work you asked for (a licence, docs) was flagged as drift and turned sessions red three times in one day. bridge-screen never stores your prompts, so it can't read what you asked for.
**Decision:** A new label, "asked for" (`requested`): the agent declares it when opening the detour (`requested_by_owner`) or labels the unit; you can relabel any unit. It is never drift, never time away, never a tuning sample.
**Consequences:** No more false alarms for asked-for work, as long as agents declare it honestly; real drift right after still turns red.

## #19 Milestone and goal status are worked out, never stored — Accepted — 2026-10-21
**Context:** Finished milestones and the goal kept showing "to do", because only tasks get a status.
**Decision:** Their status is computed from their tasks each time a screen reads it (done when every task passed its tests; needs attention if one failed). Nothing new is written to the plan.
**Consequences:** Adding a task to a finished milestone makes it "in progress" again by itself; a rebuild has nothing extra to reproduce.

## #20 Only you can take back a sign-off — Accepted — 2026-10-21
**Context:** A wrong ✓ couldn't be undone; "broken" is a different claim.
**Decision:** `bridge-screen verify <task> --withdraw` and the task page's "Take back sign-off" button take back your newest verdict (a sign-off, or a "broken" while the task is still failed) and restore the state before it exactly. The agent's tools refuse it, even with a card code. The forecast history isn't rewritten.
**Consequences:** One wrong click is recoverable; each withdraw is recorded like any other action. A task nobody holds goes back to "to do" (before, the status was left as it was).

## #21 Plain words on screens; one list of banned codes — Accepted — 2026-10-21
**Context:** Screens used bridge-screen's own terms ("checkpoint missed", "L1", "read-back pending").
**Decision:** Every owner-facing message was rewritten (`docs/lead/message-audit.md`). Codes stay inside the program; a single list (`web/src/lib/jargon.json`) is tested against the glance, `status`, `watch`, the dashboard and the card. The raw History page is exempt on purpose. The built bundle itself isn't scanned (its code holds the codes as map keys); the tests check what the components render instead.
**Consequences:** New screen text that leaks a code fails the tests.

## #22 A replaced transcript is recognised by its first bytes — Superseded by #23 — 2026-10-21
**Context:** A transcript replaced by one as long or longer kept its old reading position, so new usage was missed.
**Decision:** The reading position is stored with the file's identity: its inode and a short hash (8 bytes of SHA-256) of its first 256 bytes or fewer. A different identity means start again from the top; message ids keep anything from counting twice. The hash can't be turned back into text.
**Consequences:** One extra 256-byte read per transcript per stop; a file under 256 bytes that is still growing is re-read from the top (cheap, no double counting).

## #23 A replaced transcript is recognised without reading it — Accepted — 2026-10-21
**Context:** #22 hashed the first 256 bytes of each transcript. Those bytes are message text, and the rule is that bridge-screen reads only ids, model names, token counts and timestamps from a transcript — a hash still means reading the text.
**Decision:** The file's identity comes from its metadata only: device, inode and creation time (where the file system records one). A file deleted and written again, or renamed over, gets a new identity and is read again from the top; message ids keep anything from counting twice.
**Consequences:** No transcript text is read for this. A file rewritten in place (same file, new contents) keeps its identity; it is still caught when it gets shorter, but not when it is rewritten as long or longer. Claude Code only appends to transcripts, so this isn't expected in practice. No extra read per stop.

## #24 Work cut short by a new message is still checked for drift — Accepted — 2026-10-21
**Context:** When you send a message while an agent is mid-turn, that piece of work never gets a normal stop, so it skipped the drift check and was always marked on task. A dashboard fix done under an unrelated telemetry task went unnoticed that way.
**Decision:** The next message closes that piece of work and scores it from the files it touched (there is no closing message to read, and none is read). Clearly off the task → marked drift at once; in between → the agent is asked to label it at its next stop; clearly on the task → on task. With no task files to compare with, the "unmapped folder" signal alone can only lead to a question, never to drift. Missing text no longer counts as evidence of drift at a normal stop either.
**Consequences:** Interrupted off-task work now shows up; the check costs one extra scoring pass only on prompts that interrupt (same cost as a stop). At most one question per piece of work; if the newer piece needs its own question first, the interrupted one falls back to on task.

## #25 A repo planned by hand records its plan files instead of importing them — Accepted; owner confirmed the "changed file with no matching milestone" rule on 2026-10-22 — 2026-10-21
**Context:** bridge-screen now reads a repo's plan files (`docs/superpowers/plans/`) by itself. This repo, and any repo planned by hand, would get a duplicate milestone for every file it already has. This repo's milestones are titled like "SP7 Telemetry and forecast" and its plan files like "Sub-project 7: Telemetry and Forecast", so a title match can't be counted on.
**Decision:** The first time bridge-screen sees the plan folder of a repo that already has a plan, it notes every file there as "already planned by hand" and creates nothing. Files added after that are imported. `bridge-screen import <file> --into <milestone>` links a file to an existing milestone by hand (tasks matched by title). When a hand-noted file changes: if a milestone (or the goal) has exactly the file's title, the file joins it, matched by title. **Provisional, owner to confirm:** if nothing matches, bridge-screen creates nothing, records the file as "needs a link", and says so in plain words once in the next session briefing and every time you run `bridge-screen import`: "<file> changed but isn't linked to a milestone, so nothing was imported; run `bridge-screen import --into <milestone> <file>` to link it."
**Options considered for the no-match case:** (a) create a new milestone (an earlier build did this: loses nothing but duplicates this repo's hand-made milestones), (b) create nothing and ask for a link (chosen: loses nothing, duplicates nothing; costs one command), (c) guess the closest title (could attach work to the wrong milestone).
**Consequences:** No duplicates in hand-planned repos. An edited old plan file stays unimported until you link it.

## #26 Waits on free text are allowed; only waits on a task clear by themselves — Accepted — 2026-10-21
**Context:** Agents are blocked on other tasks ("2.4", or a task in another repo) and on things outside bridge-screen ("Sam's API key").
**Decision:** `waiting_on` takes either. A wait on a task clears when that task is verified (tests pass or better), in the same step as the verification, and the waiting session hears once. A free-text wait is cleared by the agent or by you. A wait is never a needs-you item (the session isn't blocked on you); it shows as an amber "waiting".
**Consequences:** You can see who is waiting on what without being asked to do anything.

## #27 A task's branch is the branch it was last claimed on — Accepted — 2026-10-21
**Context:** Branch groups (the same branch in several repos) need to know which tasks belong to a branch.
**Decision:** Each task remembers the branch of the session that last claimed it; nothing to fill in by hand. Branch groups leave out each repo's default branch (`main`, `master`, or what origin's HEAD points to, read as a file).
**Consequences:** A task claimed again from another branch moves to that branch's group. A subtask without its own claim counts on its task's branch.

## #28 A task in another repo is written `<repo name>:<task id>` — Accepted — 2026-10-21
**Context:** Plan links and waits can point at a task in another repo.
**Decision:** The repo is named by its folder name (or its id). A name two registered repos share is refused, listing both folders, so you pick by id.
**Consequences:** Short to write; two checkouts with the same folder name need the id.

## #29 A plan file is read up to 1000 tasks and steps (100 steps in one task) — Accepted; owner confirmed (keep as is, no warning) on 2026-10-22 — 2026-10-22
**Context:** Reviewing the import, a 1 MB file of nothing but checkboxes made `bridge-screen import` run for more than two minutes, and the same would have frozen the start of a Claude session.
**Decision:** bridge-screen imports at most 1000 tasks and steps from one plan file, and at most 100 steps in one task; the rest is left out. Real plans have a few dozen. Also: plan files beyond the 20 read at one session start are picked up by the next one (they used to be skipped for good).
**Consequences:** A very long plan file is imported in part, silently at session start. Say if you want it to flag this in the briefing.

## #30 Project pages live in bridge-screen's folder, not in the repo — Accepted; owner decided the "no repo writes" part on 2026-10-22 — 2026-10-22
**Context:** A friend's repo has no living docs (intent, project map, current state, decision log, glossary), so every new session or new worktree makes the owner re-explain the project. You ruled out writing into the repo or committing anything.
**Decision:** The five pages are plain Markdown files in `~/.bridge-screen/lead/<repo-id>/`. All worktrees of a repo share the folder, so a new worktree sees them at once. They are created only after the owner says yes once. The intent page changes only with the owner's words; the decision log only grows (two sessions adding at once both land); a replaced page keeps its previous version.
**Consequences:** No repo writes, no merge conflicts, and every worktree sees the same pages. The pages are not shared between people or machines (a later "export into the repo" command is possible). "The owner said yes" is the agent's word: the tool can't tell a real answer from an invented one, so the owner's own command (`bridge-screen pages set-up`) is the reliable route.

## #31 The offer to set up project pages repeats for at most three sessions — Accepted (owner to check); the "no plan yet" exclusion is superseded by #35 — 2026-10-22
**Context:** A session that never asks, or an owner who is busy, would otherwise mean the offer is never seen; asking every session would nag.
**Decision:** The session briefing carries the offer in up to three sessions of a repo, ranked last and dropped first when the briefing is full, never next to the "no plan yet" or plan-import notes, and never after a compaction. A yes or a no ends it at once. After three, only the command remains.
**Consequences:** One small file write per offering session. An owner who ignores three offers has to run the command themselves.

## #32 The lead-briefing skill ships inside the plugin — Accepted; owner decided 2026-10-22 — 2026-10-22
**Context:** The report style you use (effect first, plain words, checked vs believed, details last) existed only as your own installed skill.
**Decision:** A copy of that skill (unchanged) is part of the plugin, and the bridge-screen skill tells agents to use it for any status, problem or checklist and how to keep the pages current.
**Consequences:** A friend who installs the plugin gets the same reporting style. The copy can drift from your own installed one; the licence and credit for it are still open (this repo is PolyForm Strict) and block the release, not the build.

## #33 crates.io: the core library first, then the program — Accepted — 2026-10-22
**Context:** `cargo install bridge-screen` needs both crates on crates.io, and the program depends on the library.
**Decision:** A workflow publishes `bridge-screen-core` and then `bridge-screen` on each real release tag, and skips prerelease tags because a published version can never be taken back. The program's dependency names both the path and the version, and a test fails if that version differs from the workspace version. The crates carry no dashboard: `cargo install` gives a program whose dashboard page says how to build it.
**Consequences:** One more secret to set up (`CARGO_REGISTRY_TOKEN`) and two crate names to claim. The crates.io build has no dashboard; the other routes do.

## #34 Windows: the program is released, the plugin's own launcher is not — Accepted; real support is the owner's call, not made — 2026-10-22
**Context:** The release builds a Windows program, but the plugin launcher is a shell script that downloads a Unix binary and nobody has a Windows machine to test on.
**Decision:** On Windows the launcher says so in plain words and, from a hook, exits quietly. A `bridge-screen` already on the PATH is still used. The README and the first-release checklist say this.
**Consequences:** Windows users install the program themselves and then the plugin works through it. Real support needs a Windows machine; left as a follow-up.

## #35 The offer also shows in a brand-new repo, after the "no plan yet" note — Accepted; owner decided 2026-10-22 (changes part of #31) — 2026-10-22
**Context:** #31 kept the offer out of any session whose briefing carried the "no plan yet" note. A brand-new repo has no plan, so a friend's first project never saw the offer until a plan existed, although a new project is the best moment for it. The reviewer confirmed there is room: the offer is two lines and the briefing cap still holds.
**Decision:** The "no plan yet" note no longer blocks the offer; both appear, the offer after the note. The plan-import note still blocks it, and a compaction re-briefing still carries the pointer only. Everything else in #31 stands (up to three sessions, ranked last, dropped first when full).
**Consequences:** A new repo's first briefing asks two things of the owner (the goal, and the pages). The offer still stops at a yes, a no or the third session.

## #36 The lead-briefing skill stays under the repo's licence, with a credit line — Accepted; owner decided 2026-10-22 — 2026-10-22
**Context:** The skill is the owner's own work and ships inside the plugin (#32), but carried no author or licence line, so it fell under this repo's licence (PolyForm Strict, #8) by default.
**Decision:** Keep it that way: people may use it, not modify or redistribute it. A `NOTICE.md` beside the skill names the author and says it is covered by the repo's licence. The skill's own files are copied unchanged.
**Consequences:** Friends can use the skill through the plugin but not reuse it separately. Opening it up later is possible; a more open licence, once given, can't be taken back.

## #37 The crates name their licence by file, and the fix ships as 0.1.2 — Accepted; owner decided 2026-10-26 — 2026-10-26
**Context:** The v0.1.1 crates.io publish was refused: "unknown or invalid license expression". crates.io takes an official SPDX name or a licence file; PolyForm Strict (#8) is not on the SPDX list (only the Noncommercial and Small-Business PolyForm licences are). Local packaging checks never test this; only crates.io's server does, at upload. Nothing was uploaded, but the v0.1.1 tag was already public with PyPI, Homebrew and the GitHub release built from it.
**Decision:** Both crates inherit `license-file = "LICENSE.md"` from the workspace instead of a `license` string, and a test fails if a non-SPDX `license` string comes back. The fix is released as 0.1.2 (all three versions bumped together) rather than moving the public v0.1.1 tag. The 0.1.2 wheel was built locally and carries `LICENSE.md`.
**Consequences:** crates.io gets its first version at 0.1.2 (no 0.1.1 there); every other route also moves to 0.1.2. The plugin manifest keeps its own `license` string, which Claude Code doesn't validate against SPDX.
