# Current state — 2026-10-21

**Bottom line:** Sub-project 8 (imports and onboarding) is built and checked on its branch, not merged yet. A repo you open now gets its plan without hand-seeding. — Green: on track.

**What works today:**
- Sessions are tracked automatically: active task, time, and whether each piece of work was on task, a detour, drift, or **asked for** by you.
- Crashed or abandoned sessions are detected; their half-finished changes are offered back, and every new session gets a short briefing.
- "Done" means verified: claimed → code exists → tests pass → end-to-end → you signed it off; you can take a sign-off back.
- Milestones and the goal show done / in progress / needs attention from their tasks.
- Decisions are recorded with their reasons; cost per task, session and project; a finish range per project.
- Three readable screens (terminal, browser dashboard, desktop-app card) in plain words.
- **New (on the branch):** plan files in `docs/superpowers/plans/` are read by themselves at each session start and kept up to date when they change; the files are never written. A ticked step counts as claimed, not checked. A repo already planned by hand (like this one) has its files noted, not imported — no duplicates.
- **New:** `bridge-screen init` for a repo with work but no plan: the agent drafts a plan from the repo's history, you confirm it once, tasks that look done are checked for "the code exists".
- **New:** an agent can say "I'm waiting on 2.4" (or on a task in another repo, or on "Sam's API key"); the session shows an amber "waiting", and it hears once when the task's tests pass. You can clear a wait or turn it into a plan link.
- **New:** plan links can point at a task in another repo (`api:2.4`); the same branch in two repos shows as one line with a combined %.

**In flight:** sub-project 8 waits for review and merge (branch `feat/sp8-imports-onboarding`).

**Open problems:**
- **Friends can't install it yet:** no published release. — Open; needs a first release.
- **A hand-planned file that changes and matches no milestone title becomes a new milestone.** — By design (decision #24); link it with `bridge-screen import <file> --into <milestone>` first.
- **"Asked for" relies on the agent saying so.** — By design.
- **On `status`, a drifting session doesn't say which command answers it.** — Open; one-line fix.
- **Fast-mode work shows "cost unknown".** — Open; your choice.

**Needs your decision:**
- Review and merge sub-project 8? Recommended: yes.
- Confirm or correct the intent page (`docs/lead/intent.md`).

**Next:** merge sub-project 8, reinstall, then sub-project 9 (archive) or the first release.

**How sure:** The full Rust and web test suites, the hook speed checks, the browser tests and the plugin check pass on the branch. By hand, on a copy of your real data (never the real folder): this repo's ten plan files were noted with no duplicate milestones; a scratch repo's plan file was imported and left byte-identical; `bridge-screen init` on a scratch half-built repo listed its evidence and left it untouched. Nobody has clicked through the new dashboard parts by hand yet.
