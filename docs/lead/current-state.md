# Current state — 2026-10-21

**Bottom line:** Seven of nine sub-projects are merged: bridge-screen now also tracks what work costs and forecasts when each project will likely be done. — Green: on track.

**What works today:**
- Sessions are tracked automatically: active task, time, and whether each piece of work was on task, a detour or drift.
- Crashed or abandoned sessions are detected; their half-finished changes are offered back (adopt, resume, undo — undo never loses work), and every new session gets a short briefing.
- "Done" means verified: claimed → code exists → tests pass → end-to-end → you signed it off.
- Decisions are recorded with their reasons.
- Three readable screens (terminal, browser dashboard, desktop-app card), with live motion you can pause.
- **New:** cost in tokens and dollars per task, session and project; a finish range per project ("likely by … · could take until …"); a Calibration page showing how long tasks take against their estimates.

**In flight:** nothing running.

**Open problems:**
- **Friends can't install it yet:** no published release, so the plugin can't download the program. — Open; needs a first release.
- **New repos start at 0%:** bridge-screen doesn't read existing plans or history yet. — Open; planned for sub-project 8.
- **Work you ask for counts as "drift"** and flags the session red. — Open; next round.
- **Milestones never show "done"** even when all their tasks are verified (the % is right). — Open.
- **A session's branch name goes stale** if it switches branch mid-session. — Open.
- **After an upgrade, already-open sessions lose bridge-screen's tools** until a new session starts. — Open; workaround: start a new session.
- **Fast-mode work shows "cost unknown"** until fast mode has a price. — Open; your decision.

**Needs your decision:**
- Confirm or correct the intent page (`docs/lead/intent.md`).
- Fast-mode pricing: leave "unknown" (recommended) or price it at $8 / $40 per million tokens on Opus 5.5.
- Next: the lead-briefing round for bridge-screen's own messages (plus the drift-for-requested-work fix), then sub-project 8 (plug-and-play onboarding)? Recommended: yes.
- When to cut the first release.

**Next:** reinstall and start a new session to use cost and forecasts; then the next round once you choose it.

**How sure:** Sub-project 7 passed all five CI checks on GitHub (Rust on macOS and Linux, web, browser test, release plan) and the full suite locally before merging. The forecast's maths and how costs are matched to tasks were reviewed by reading, not by an independent test on your real data.
