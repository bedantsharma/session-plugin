# Message audit — what bridge-screen says to you

Every message bridge-screen shows you, where it appears, what it said before the polish
round and what it says now. The rule: effect first, plain words, say what to do; a
project term only if it is in the [glossary](glossary.md). Codes stay inside the program
(the tools and the data keep `l2`, `drift_red`, `requested`); screens show words.

The words live in one place per language: `crates/bridge-screen/src/words.rs` (status,
watch, report, CLI) and `web/src/lib/words.ts` (dashboard, card). The list of codes that
must never reach a screen is `web/src/lib/jargon.json`; the tests
`no_jargon_on_screens` (Rust) and `tests/jargon.test.ts` (web) check it.

Out of scope: notes only the agent reads (hook context), and the History drill-down,
which is the raw event log on purpose.

| Where it appears | Before | Now |
|---|---|---|
| Session flag (status, watch, dashboard, card) | checkpoint missed | didn't save its work when asked |
| Session flag | checkpoint pending | work not saved yet |
| Session flag | read-back pending | hasn't confirmed it understood the handoff yet |
| Session flag | handoff unstable | started from an unclear handoff |
| Session flag | squash failed | couldn't tidy its commits |
| Session flag | verify pending: 1.2 | said done, not checked yet: 1.2 |
| Needs-you item, title | Verify 1.2 Login form | Check 1.2 Login form |
| Needs-you item, detail | claimed done (L1), nudged twice without a check · does it work? | The agent said it was done 2 days ago (the code is there; tests not run) and nobody has checked it. Check it, or ask the agent to. |
| Needs-you item, title | S ab12cd left 3 uncommitted files | S ab12cd stopped with 3 unsaved files |
| Needs-you item, detail | suggested: resume · in /wt | suggested: resume (carry on with it) · in /wt |
| "Make it a task" fields | parent (milestone or task id) / title | Under (e.g. 10) / Task title |
| Session light word (all screens) | detour (for work you asked for) | asked for (green) |
| Unit label (session drill, watch) | — | ★ Asked for · agent |
| Task and step status (dashboard, watch, MCP status) | claimed / L1 code exists / L2 tests pass / L3 checked / L4 owner-checked / failed / active | said done, not checked yet / code written, tests not run / tests pass / tried end to end / you signed it off / failed a check / being worked on |
| Milestone and goal status | to do (always) | done / in progress / needs attention / to do, from their tasks |
| Task drill, verification row | L2 by rerun (re-run timed_out) · stale | tests pass · by bridge-screen re-ran the tests (they took too long) · out of date: files changed since |
| Task drill, button | — | Take back sign-off / Take back "broken" |
| Session drill | Read-back: pending / done / mismatch | Handoff understood: not confirmed yet / confirmed / misunderstood — check what it said |
| watch, session detail | … · read-back pending | … · handoff understood: not confirmed yet |
| watch, session detail | checkpoint abc12345 — next: … | last saved at abc12345 — next: … |
| status, needs-you line | needs you: verify 1.2 X — bridge-screen verify 1.2 (or --broken "…") | needs you: check 1.2 X — then `bridge-screen verify 1.2` (or --broken "what's wrong") |
| status, project line | 2 unestimated | 2 tasks without an estimate |
| status, project line | leftovers: 3 open (1 report-only) | unsaved work from ended sessions: 3 files (1 of them for information only) |
| Dashboard project card | 2 unestimated | 2 tasks without an estimate |
| Leftover state (session drill) | report only (suggested: resume) | for information only — suggested: resume (carry on with it) |
| CLI `verify` | Verified 1.2 Login form L4 at abc1234. | Signed off 1.2 Login form at abc1234. (other levels: Recorded 1.2 Login form as "tests pass" at abc1234.) |
| CLI `verify --withdraw` | — | Took back your sign-off on 1.2 Login form; it's back to where it was before. |
| CLI `verify --withdraw`, nothing to take back | — | nothing to withdraw on 1.2: you haven't signed it off or marked it broken |
| Any tool, after an upgrade | database schema version 8 is newer than this bridge-screen supports (7); upgrade bridge-screen | bridge-screen was updated while this session was open. Start a new Claude session to keep using it (data format 8; this session's copy understands up to 7). |
| watch, needs-you tag | verify / leftovers | check / unsaved |
| Scope summary (project drill) | +20 min at 10:02 · +15 min at 10:02 · +10 min at 10:02 | 3 additions, +45 min at 10:02 |
| Session light word and chip (all screens; SP8) | — | Waiting (amber), and "· waiting on 2.4 Login form" on the session line |
| Branch groups (dashboard, status, watch; SP8) | — | feat/auth · 2 repos · 45% (each repo's % in the tooltip); "no tasks on this branch yet" |
| Project drill, per task (SP8) | — | from sp7 plan, task 3 · drafted at setup · after api:2.4 · ready/waiting · the branch |
| Session drill, waits (SP8) | — | "Waiting on" list: target — reason · waiting 40m / done @abc1234 / cleared by hand / now a plan link; buttons Clear and Make it a plan link |
| Owner CLI `wait clear` / `wait promote` (SP8) | — | Cleared W2: ab12cd no longer waits on the design doc. / 1.1 now depends on api:2.4; the wait W1 is closed. |
| `bridge-screen import` (SP8) | — | Imported sp7-x: added 6 tasks and steps; 2 ticked steps recorded as claimed (not checked) · Noted … as already planned by hand (not imported) · Skipped …: no "### Task N:" headings · No new or changed plan files. |
| `bridge-screen init` (SP8) | — | the repo's evidence, then "Next steps (for the agent)", or "First: run `bridge-screen import`", or "… already has a plan; nothing to set up" |

Reviewed and kept (already plain, or a glossary term): "far off its task (40 min)",
"asked you a question", "waits on a permission prompt", "waits on your input", "answer in
that session", Continue / Make it a task / Stop, ✓ Works / ✗ Broken, Adopt / Resume /
Undo, "exploring" / "executing" and their tooltips, "stuck · 3 sessions", the forecast
and cost lines, "drift ⚠ 40m", "tuned drift .35–.65, tolerance 25%".
