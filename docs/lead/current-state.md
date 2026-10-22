# Current state — 2026-10-22

**Bottom line:** The first release (v0.1.0) is live and installable; the project pages and the bundled reporting skill are built and checked on a branch, with a pull request (#18) waiting to merge. npm is the one install route still failing. — Amber: npm unfinished; the new work never tried in a real Claude session or a browser.

**What works today (on main):**
- Sessions are tracked automatically: active task, time, and whether each piece of work was on task, a detour, drift, or **asked for** by you. Work cut short by one of your messages is checked for drift too.
- Crashed or abandoned sessions are detected; their half-finished changes are offered back, and every new session gets a short briefing.
- "Done" means verified: claimed → code exists → tests pass → end-to-end → you signed it off; you can take a sign-off back.
- Milestones and the goal show done / in progress / needs attention from their tasks.
- Decisions are recorded with their reasons; cost per task, session and project; a finish range per project.
- Plan files are read by themselves; agents can say what they wait on; plan links reach other repos; three readable screens in plain words.

**Released (v0.1.0, from main):**
- **GitHub release:** programs for Mac (Apple and Intel), Linux and Windows, the shell and PowerShell installers, with checksums. The plugin downloads from here.
- **Homebrew:** the formula is in your tap (`bedantsharma/homebrew-tap`).
- **PyPI:** `bridge-screen` 0.1.0 with five wheels is live.
- **npm:** not live. The publish keeps failing on a one-time-code request.
- **crates.io:** not published; it ships with the next version.

**New on the branch (pull request #18, not merged):**
- **Project pages:** a repo with none gets one offer in the session briefing (up to three sessions, also in a brand-new repo with no plan yet); your yes (or `bridge-screen pages set-up`) creates five pages outside the repo, shared by every worktree. Agents read and update them with three tools; your intent page changes only with your words; the decision log only grows. The dashboard shows them read-only under a project's "Project pages".
- **The reporting skill** (`lead-briefing`) is part of the plugin, unchanged, under the repo's licence with a credit line.
- **Release:** a crates.io publish workflow (core first, then the program), a plain Windows message from the plugin launcher, and a first-release checklist.

**In flight:** pull request #18 (merging once its automatic checks pass).

**Open problems:**
- **npm:** the publish asks for a one-time code that a build server can't give; the token's two-factor bypass setting needs fixing, or the first version can be published by hand.
- **"The owner said yes" is the agent's word.** The tool cannot tell a real answer from an invented one; the command you run yourself is the reliable route (decision #30).
- **Windows plugin launcher is not supported** (decision #34); the program is released for Windows but nobody has run it there.
- Earlier items still open: fast-mode work shows "cost unknown".

**Decided today:** keep #25 (link, don't duplicate) and #29 (size cap); the offer also shows in a brand-new repo (#35); the bundled skill stays under the repo's licence with a credit line (#36).

**Needs your decision:**
- Confirm the provisional choices: offer repeats up to three sessions (#31), crates.io skips prerelease tags (#33), launcher message on Windows (#34). Recommended: keep all three.
- Confirm or correct the intent page (`docs/lead/intent.md`).
- Next: sub-project 9 (archive) or the next release (v0.1.1, with the pages and crates.io)?

**Next:** merge #18, fix npm, reinstall and try the offer in a real session; then release v0.1.1.

**How sure:** Checked by running: on the branch, the Rust suite (579 passed, 0 failed), the web tests (163 passed) and clippy; the v0.1.0 release (every build step passed, the Mac program's checksum matched and it reports 0.1.0); the Homebrew formula file exists in the tap; PyPI lists 0.1.0 with its wheels. Relayed from the reviewing agent, not re-run: the browser tests (6 passed), the speed checks and the smoke test in a temporary home. Not checked: `brew install` and `npx` on a real machine, the crates.io workflow (never run), a real Claude session seeing the offer, the new dashboard screen in a browser, anything on Windows.
