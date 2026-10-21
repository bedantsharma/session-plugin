# Current state — 2026-10-21

**Bottom line:** Seven of nine sub-projects plus the polish round are merged. bridge-screen raises fewer false alarms and speaks in plain words. — Green: on track.

**What works today:**
- Sessions are tracked automatically: active task, time, and whether each piece of work was on task, a detour, drift — or **asked for** by you (new: never flagged as drift).
- Crashed or abandoned sessions are detected; their half-finished changes are offered back (adopt, resume, undo — undo never loses work), and every new session gets a short briefing.
- "Done" means verified: claimed → code exists → tests pass → end-to-end → you signed it off. **New:** you can take back a sign-off (`bridge-screen verify <task> --withdraw` or the dashboard button).
- **New:** milestones and the goal show done / in progress / needs attention from their tasks.
- **New:** branch names stay current; after an upgrade, open sessions say "start a new session".
- Decisions are recorded with their reasons.
- Three readable screens (terminal, browser dashboard, desktop-app card), now in plain words — every message is listed before/after in `message-audit.md`.
- Cost in tokens and dollars per task, session and project; a finish range per project; a Calibration page.

**In flight:** nothing running.

**Open problems:**
- **Friends can't install it yet:** no published release. — Open; needs a first release.
- **New repos start at 0%:** bridge-screen doesn't read existing plans or history yet. — Open; sub-project 8.
- **"Asked for" relies on the agent saying so:** bridge-screen doesn't read prompts, so the agent must tag it (you can relabel). — By design.
- **On `status`, a drifting session doesn't say which command answers it.** — Open; one-line fix.
- **Fast-mode work shows "cost unknown".** — Open; your choice (left unknown).

**Needs your decision:**
- Confirm or correct the intent page (`docs/lead/intent.md`).
- Next: sub-project 8 (plug-and-play onboarding), then the first release? Recommended: yes.

**Next:** reinstall (rebuild the web dashboard and the program) and start a new session to get the polish round; then sub-project 8.

**How sure:** The polish round passed all four CI checks on GitHub (Rust on macOS and Linux, web, browser test) and the full suite locally before merging. The reviewer upgraded a real older database by hand with no data lost. Nobody has clicked through the dashboard and card by hand yet.
