# Current state — 2026-10-22

**Bottom line:** Sub-project 8 (imports and onboarding) is built, reviewed and checked on its branch, with a pull request waiting for your merge. Once it's merged, a repo you open gets its plan without hand-seeding. — Green: on track.

**What works today (on main):**
- Sessions are tracked automatically: active task, time, and whether each piece of work was on task, a detour, drift, or **asked for** by you. **New on main:** work cut short by one of your messages is now checked for drift too (it used to be marked on task unchecked).
- Crashed or abandoned sessions are detected; their half-finished changes are offered back, and every new session gets a short briefing.
- "Done" means verified: claimed → code exists → tests pass → end-to-end → you signed it off; you can take a sign-off back.
- Milestones and the goal show done / in progress / needs attention from their tasks.
- Decisions are recorded with their reasons; cost per task, session and project; a finish range per project.
- Three readable screens (terminal, browser dashboard, desktop-app card) in plain words.

**New with sub-project 8 (after the merge):**
- Plan files in `docs/superpowers/plans/` are read by themselves at each session start and kept up to date when they change; the files are never written. A ticked step counts as claimed, not checked. A repo already planned by hand (like this one) has its files noted, not imported — no duplicates.
- `bridge-screen init` for a repo with work but no plan: the agent drafts a plan from the repo's history, you confirm it once, tasks that look done are checked for "the code exists".
- An agent can say "I'm waiting on 2.4" (or on a task in another repo, or on "Sam's API key"); the session shows an amber "waiting", and it hears once when the task's tests pass. You can clear a wait or turn it into a plan link.
- Plan links can point at a task in another repo (`api:2.4`); the same branch in two repos shows as one line with a combined %.

**In flight:** sub-project 8's pull request, waiting for your review and merge. The session-notes wrap fix (PR #14) also waits for merge.

**Open problems:**
- **Friends can't install it yet:** no published release. — Open; needs a first release.
- **An old plan file you edit in a hand-planned repo isn't imported until you link it.** bridge-screen says so in the briefing and in `bridge-screen import`, with the command to link it. — By design, provisional (decision #25); your call.
- **A very long plan file is imported only in part** (first 1000 tasks and steps), silently. — Provisional (decision #29); say if you want a warning.
- **"Asked for" relies on the agent saying so.** — By design.
- **On `status`, a drifting session doesn't say which command answers it.** — Open; one-line fix.
- **Fast-mode work shows "cost unknown".** — Open; your choice.

**Needs your decision:**
- Merge sub-project 8? Recommended: yes.
- Confirm the two provisional choices (#25 link-not-duplicate, #29 size cap). Recommended: keep both.
- Confirm or correct the intent page (`docs/lead/intent.md`).

**Next:** merge, reinstall (rebuild the web dashboard and the program) and start a new session; then sub-project 9 (archive) or the first release.

**How sure:** On the branch, the full Rust suite (525 tests), the web checks and tests, the hook speed checks, the browser tests and the plugin check pass; I re-ran the Rust suite and clippy myself after the review. A reviewer found and fixed five defects (a duplicate-milestone path, a very long plan freezing session start, files past the 20th never imported, a "done" note sent twice, a misplaced comment). On a copy of your real data (never the real folder): the database upgrade kept every row; two session starts in this repo noted its ten plan files and added no milestones. Nobody has clicked through the new dashboard parts by hand yet.
