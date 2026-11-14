# Current state — 2026-11-14

**Bottom line:** The tracks, hiding, removing, no-test tasks and Back work are all built on the branch `feat/tracks-hide-review` (not merged, not released, not tried in a real Claude session or a browser by a person). — Amber: everything below is checked by the automated tests; nothing was tried by hand.

**What is new on the branch:**
- **No-test tasks:** a task can be checked by review instead of tests; it counts as done once its result is saved ("done — no tests") and you can still mark it broken. You can press ✓ Works / ✗ Broken on any task said done.
- **Tracks:** one folder can hold several tracks, each with its own goal, bar and numbering from 1; agents ask you before starting another; you finish or reopen them.
- **Hiding:** hide a card, a track's bar or a project; it sits in a Hidden drawer and comes back only when something new happens to it. Numbers never change.
- **Remove for good:** remove abandoned tasks, milestones or tracks after a plain question; a plan file can't bring them back.
- **Back works:** the browser's Back button and the swipe move between dashboard screens.
- **Plan import:** step checkboxes are no longer tasks; the old step noise is dropped by the next import.

**Checked by running:** the full Rust test suite, clippy, formatting, the web tests (lint, type check, unit tests, build). See the final report for counts.

**Believed from reading, not run:** the Playwright browser tests (written; they need a browser download), `bridge-screen rebuild` against a copy of a real old database, and any hand use of the new screens.

**Open problems (unchanged):** npm install route unfinished; "the owner said yes" is the agent's word (decision #30); Windows launcher unsupported (#34); fast-mode work shows "cost unknown".

**Not part of this round:** starting a track from the dashboard, un-removing from the dashboard, renaming or merging tracks, tracks across repos.
