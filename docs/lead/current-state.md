# Current state — 2026-10-22

**Bottom line:** The project pages, the bundled reporting skill and the crates.io and Windows release work are built and checked by tests on the branch `feat/project-pages-and-distribution`; nothing is merged, pushed or released yet. — Amber: not yet reviewed, and never tried in a real Claude session or a browser.

**What works today (on main):**
- Sessions are tracked automatically: active task, time, and whether each piece of work was on task, a detour, drift, or asked for by you; crashed sessions are detected and every new session gets a short briefing.
- "Done" means verified (claimed → code exists → tests pass → end-to-end → you signed it off); decisions, cost per task and a finish range per project are recorded.
- Plan files are read by themselves; agents can say what they wait on; three readable screens in plain words.

**New on the branch (not merged):**
- **Project pages:** a repo with none gets one offer in the session briefing (up to three sessions); your yes (or `bridge-screen pages set-up`) creates five pages outside the repo, shared by every worktree. Agents read and update them with three tools; your intent page changes only with your words; the decision log only grows. The dashboard shows them read-only under a project's "Project pages".
- **The reporting skill** (`lead-briefing`) is now part of the plugin, and the bridge-screen skill says when to use it and how to keep the pages current.
- **Release:** a crates.io publish workflow (core first, then the program), a plain Windows message from the plugin launcher, and a first-release checklist in `docs/RELEASING.md`.

**In flight:** a review of this branch against the design is next. Sub-project 8's pull request and the session-notes wrap fix (PR #14) are on main already.

**Open problems:**
- **Friends can't install it yet:** no published release; the checklist is written, the one-time account steps are yours.
- **"The owner said yes" is the agent's word.** The tool cannot tell a real answer from an invented one; the command you run yourself is the reliable route (decision #30).
- **The licence and credit for the bundled lead-briefing skill are undecided** (this repo is PolyForm Strict). Blocks the release, not the build.
- **Windows plugin launcher is not supported** (decision #34); the program itself is released for Windows but nobody has run it there.
- Earlier items still open: an edited old plan file in a hand-planned repo isn't imported until linked (#25); a very long plan file is imported in part (#29); fast-mode work shows "cost unknown".

**Needs your decision:**
- Confirm the three provisional choices: offer repeats up to three sessions (#31), crates.io skips prerelease tags (#33), launcher message on Windows (#34). Recommended: keep all three.
- Licence and credit for the bundled skill (blocks release).
- Confirm or correct the intent page (`docs/lead/intent.md`).

**Next:** review and merge this branch, reinstall, try the offer in a real session; then the rehearsal tag (`v0.1.1-rc.1`) and the first crates.io publish with `v0.1.1` (v0.1.0 is already live without the crates).

**How sure:** Checked by running, on the branch: the Rust suite (577 passed, 0 failed, 10 slow speed checks skipped), the web tests (163 passed), clippy, the web lint and type checks and build; the SessionStart speed check with the offer and with the pointer (p95 29 ms and 22 ms, budget 150 ms); `cargo publish --dry-run` for the core library and `cargo package --workspace` for both crates; `dist plan` (its archive names match what the launcher downloads); the plugin manifest check. Believed from reading only: the new GitHub workflow (its YAML parses; it has never run), the Windows launcher message (tested with a pretend Windows only). Not checked: a real Claude session seeing the offer, the dashboard screen in a browser, the browser end-to-end tests, anything on Windows.
