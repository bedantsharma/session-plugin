# Project pages and the lead-briefing bundle — design

Date: 2026-10-22. Milestone 11 (Distribution), task 11.1. Builds on spec
`2026-10-13-bridge-screen-design.md` (§6 storage, §8.4 briefing, §11 agent interface, §16 distribution).

## Intent and the why

A friend who installs bridge-screen should get two things the owner has today by hand:

1. **Agents that report to the owner in the lead-briefing style** (effect first, plain words,
   checked vs believed, details last) when asked for a status, a problem or a checklist.
2. **A small set of project pages** (intent, project map, current state, decision log,
   glossary) that every agent session in the repo can read and keep current, so a new session or
   a new worktree does not make the owner re-explain the project.

This repo already has both, committed under `docs/lead/` and in `CLAUDE.md`. A friend's repo has
neither, and nothing in bridge-screen reads, mentions or injects them. Today an agent finds them by
luck.

## Decisions the owner made (in conversation, 2026-10-22)

- The lead-briefing skill ships **inside the plugin bundle** (`plugin/skills/lead-briefing/`), not as
  a separate install. The bridge-screen skill points agents to it.
- bridge-screen writes project pages **only after the owner says yes once**. Never silently.
- The pages are **not** written into the repo and **not** committed. They must still be visible to
  every worktree and session of the repo.
- The agent fills in the current state if it can.

## Decisions I made (owner to check)

- **Where the pages live:** `~/.bridge-screen/lead/<repo-id>/`, plain Markdown files. `repo_id` is the
  existing id from the repo's shared git folder (spec §6.1), so all worktrees resolve to the same
  folder. Chosen over files inside the repo because a worktree or fresh clone does not carry ignored
  files, and over committed files because the owner ruled them out.
- **How agents reach them:** a short pointer in the SessionStart briefing, and MCP tools to read and
  write a page. Not injected in full (keeps the briefing ≤ 40 lines and always current).
- **State of the decision (set up, declined, offered):** marker files in the same folder, not events
  or a new table. No schema change; `bridge-screen rebuild` does not touch the folder.
- **Five pages, not six.** `message-audit.md` is specific to this repo and is not created elsewhere.
- **No `CLAUDE.md` heading.** The briefing does that job. (The owner first suggested a heading;
  after the move out of the repo it is not needed.)
- **Dashboard view is part of this round (owner decision, 2026-10-22).** A read-only "Project pages"
  view in the dashboard: the five pages rendered as text with their last-updated dates, reached from
  the glance screen, in plain words, read through the dashboard's JSON API. No editing from the
  dashboard. The CLI `pages show` also prints the location and content.

## Goals

- A repo with no pages gets **one** offer, through the session briefing. Yes creates the pages from
  built-in templates; no stops all further offers for that repo.
- Every session of a repo with pages sees a one-line pointer in its briefing and can read or update a
  page with a tool, from any worktree.
- The lead-briefing skill and its reference files are in the plugin, and the bridge-screen skill tells
  agents when to use it.
- After setup the agent drafts **current state** from the repo's evidence and a draft **intent** for
  the owner to confirm; unknowns are written as "not known yet", never invented.

## Non-goals and anti-goals

- Writing anything inside a repo, committing, or editing `.gitignore`, `.git/info/exclude`,
  `CLAUDE.md` or `AGENTS.md`.
- Syncing pages between machines or people. (A later owner-run "export into the repo" command is
  possible; not built now.)
- Anti-goal: an agent that rewrites the owner's intent page or deletes decision-log history.
- Anti-goal: a briefing that grows past 40 lines, or a hook that slows past its budget.
- Not in this round: the first public release and its account steps (milestone 11, later tasks).

## Behaviour

### Storage and state

- Folder `~/.bridge-screen/lead/<repo-id>/` holds `intent.md`, `project-map.md`, `current-state.md`,
  `decision-log.md`, `glossary.md`.
- Marker files in that folder: `.offered` (the offer was made), `.declined` (owner said no). The pages
  exist = set up. Writes are atomic (temp file then rename); the previous version is kept as
  `<page>.prev`.
- Tests and the code use the existing home override so nothing touches the real `~/.bridge-screen`.

### The offer and the owner's yes

- "Known repo" = any git repo the SessionStart hook already resolves a repo id for (it does this
  for every session today; SessionStart is already allowed to run git, so this adds no new git
  process).
- At SessionStart, for such a repo with no pages and no `.declined`: the briefing gets one note
  asking the agent to ask the owner, in plain words, whether to set up project pages, and to relay
  the answer. **My call:** the note is repeated for at most three sessions (the hook counts them in
  `.offered`), and stops earlier when any `lead_pages` call or CLI command is made. After three,
  only the CLI remains. The note ranks **last** among briefing notes, is dropped first when the
  briefing is full, and is left out in a session that already carries a plan-import note.
  **Owner decision, 2026-10-22:** a brand-new repo's "no plan yet" note does not block the offer;
  both appear, the offer after the note.
- The agent calls `lead_pages` with `action: "set_up"` and the owner's words (`owner_said`), or
  `"decline"`. Without `owner_said` the call is refused (same pattern as `init_plan` and the L4
  verdict). **My call:** this is a free-text check, as for `init_plan`; it cannot tell a real
  answer from an invented one. Accepted for now; the CLI is the owner's own route.
- **My call:** `set_up` after a `.declined` works through the CLI and through the tool with
  `owner_said`, and clears `.declined`. `set_up` is idempotent: it creates only the missing pages,
  never overwrites an existing one. "Set up" means all five pages exist; a partial set is
  completed by the next `set_up`.
- Owner-run CLI as the other route: `bridge-screen pages set-up | decline | show | path [-C DIR]`.
  `show` prints the folder and each page's last-updated date.
- `set_up` creates the five pages from built-in templates (taken from the lead-briefing skill's
  living-docs templates) and returns the next steps for the agent: draft `current-state.md`, draft
  `intent.md` and show it to the owner once, add known terms to the glossary.

### Reading and writing pages

- `lead_page_read {page}` returns the page text. `lead_pages {action: "status"}` lists pages with
  last-updated dates.
- `lead_page_write {page, content, reason}`:
  - `current-state`, `project-map`, `glossary`: replace the page.
  - `decision-log`: append only. **My call:** the call takes `mode: "append"` and adds an entry under
    an advisory file lock, so two sessions in different worktrees never clash and nothing needs a
    re-read. A replace-style write to this page is refused with a plain message.
  - `intent`: refused unless `owner_said` is given (the owner owns it).
- Page names are a fixed list; any other name is refused. Content over a size cap (64 KiB) is refused.

### Briefing pointer

- Pages exist: one line, e.g. "Project pages: read intent and current state first (`lead_page_read`);
  keep current state true after each merge. Last updated <date>." **My call:** no "stale" check
  against verified tasks (it would need a database read in the hook); the date is enough.
- Pages declined or the offers used up: nothing.
- The briefing stays ≤ 40 lines; the hook adds at most one file stat and, while offering, one small
  file write per session. No new git process.

### The lead-briefing bundle

- Copy the owner's skill (its `SKILL.md` and `references/` — `why-it-works.md` and
  `living-docs.md`) to `plugin/skills/lead-briefing/` unchanged except for plugin-friendly front
  matter if the validator requires it. The only copy on this machine is the installed one under
  `~/Library/Application Support/Claude/local-agent-mode-sessions/skills-plugin/*/*/skills/lead-briefing/`
  (found with `find … -path "*lead-briefing*"`); the owner confirms it is current before the copy.
  The five page templates are taken from `references/living-docs.md`.
- `plugin/skills/bridge-screen/SKILL.md` gets a short section: use lead-briefing whenever the owner asks
  for status, a problem or a checklist; use the page tools for the living docs; when to update each page.
- **Licence and credit (owner decision, 2026-10-22):** the skill stays under this repo's licence
  (PolyForm Strict: people may use it, not modify or redistribute it). A short `NOTICE.md` in the
  skill's folder credits the author and says so.

## Constraints

- Hooks ≤ 50 ms p95 and never fail a session; errors go to the hook error log; exit 0.
- Nothing written in any repo. Writes only under `~/.bridge-screen/`.
- Owner-facing wording follows the lead-briefing rules; new messages are added to
  `docs/lead/message-audit.md` and checked against `web/src/lib/jargon.json`.
- Tests never touch the real home; no real `claude` CLI.
- `plugin.json` version stays equal to the workspace version; `claude plugin validate ./plugin` passes.
- Update spec §6.1 (layout), §8.4 (briefing), §11.2 (tools), the decision log, the glossary
  ("project pages") and the project map.

## Success checks

1. First session in a repo without pages: briefing carries the offer once; the next session does not.
2. After yes: five pages exist; a second worktree of the same repo reads the same pages.
3. After no: no later offer; no folder content created.
4. `intent` write without the owner's words is refused; `decision-log` write that drops old text is
   refused; unknown page name refused.
5. Briefing length and hook latency stay inside their budgets (existing latency test extended).
6. `bridge-screen rebuild` leaves the pages untouched.
7. `claude plugin validate ./plugin` passes with both skills; a test checks the lead-briefing skill files
   are present in the plugin.

## Left to the builder

File layout inside the crates, the exact wording of messages (plain words, audited), template text,
how the CLI subcommand is wired, and test names.

## Open questions for the owner

None. (The licence question is settled; see "The lead-briefing bundle".)

## Working rules for the build (owner, 2026-10-22)

- The plan is written as pseudocode only; the code is written by the implementing agent.
- At most four agents in total are used for this work.
- The owner confirmed the installed copy of the lead-briefing skill is current.
