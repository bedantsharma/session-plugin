# Project map — bridge-screen

## The whole thing in one paragraph

bridge-screen is one program on the owner's computer that sits next to Claude Code. Every time an agent session does something (starts, gets a prompt, edits a file, stops), Claude Code tells bridge-screen, which records it in a local database. From that record it works out progress, notices when a session drifts off its task, keeps handoff notes for the next session, and shows it all on three screens. Agents also talk to it directly through a small set of tools (claim a task, say it's done, open a detour). It talks to no outside service except git on your machine, and GitHub only to download its own releases.

## Diagram

```
  Owner ──reads/clicks──▶ Screens: terminal (status, watch) · browser dashboard · desktop-app card
                                   │ read + owner actions
                                   ▼
  Claude Code ──events──▶ Hooks ──▶ Event log + views (local database) ◀── Agent tools (MCP server)
  sessions    ◀─short notes─┘            ▲                                   ▲
       └───────────── agents call tools ─┼───────────────────────────────────┘
                                          │
                          git (status, checkpoint commits, stash, squash) on your repos
```

## The parts

### Hooks (also called `bridge-screen hook`)
- **Does:** receives every Claude Code event, records it, and sometimes slips a one-line note into the agent's context (a checkpoint request, "label your last unit", the session briefing).
- **Talks to:** Claude Code (in), the database, git only at a few safe moments.
- **Holds:** nothing itself.
- **Lives in:** `crates/bridge-screen/src/hook/`

### Agent tools (also called the MCP server, `bridge-screen mcp`)
- **Does:** gives agents tools: claim a task, edit the plan, open/close a detour, say a task is done, verify it, log a decision, leave a handoff note, show progress, and read or update the project pages.
- **Talks to:** Claude Code (agents call it), the database, git.
- **Lives in:** `crates/bridge-screen/src/mcp.rs`

### The core (also called `bridge-screen-core`)
- **Does:** all the rules: the plan tree (and each milestone's and the goal's status, worked out from its tasks), sessions and active time, drift scoring (work you asked for never counts), checkpoints and recovery, the verification ladder (including taking a sign-off back), decisions, and the read models the screens show.
- **Lives in:** `crates/bridge-screen-core/`

### Telemetry (cost tracking)
- **Does:** at each stop, reads only the new lines of the agent's transcript and records each model reply's token counts once (never the text), against the task, detour and agent at that moment; prices them when a screen asks.
- **Talks to:** Claude Code's transcript files (read-only), the database, the price table in `config.toml`.
- **Lives in:** `crates/bridge-screen-core/src/telemetry/`

### Forecast
- **Does:** turns how long verified tasks took against their estimates into a finish range (P50/P90 dates) for each project; refuses to give dates on too little data; keeps a history after each verified task.
- **Talks to:** the database only.
- **Lives in:** `crates/bridge-screen-core/src/forecast/`

### Imports and onboarding
- **Does:** reads the repo's plan files (any `*.md` named plan, roadmap, todo or tasks, in any folder; `docs/superpowers/plans/` is just the usual place) at each session start and with `bridge-screen import`, and turns new or changed ones into plan changes (never writing to the files); `bridge-screen init` lists what a half-built repo shows so an agent can draft its plan for you to confirm. When plan files say nothing is finished it asks you once ("Fresh run" or "Some are done") before importing. A finished run is closed when a new plan arrives, so each plan round starts at 0% ("runs").
- **Talks to:** the repo's files (read-only), git (only for `init`, run by hand), the database.
- **Lives in:** `crates/bridge-screen-core/src/import/`, `crates/bridge-screen-core/src/init.rs`, `crates/bridge-screen/src/onboard.rs`

### Waits and links across repos
- **Does:** keeps each session's "waiting on" list, ends a wait when its task is verified and tells the waiting session; lets plan links point at a task in another repo; groups the same branch across repos on the glance.
- **Talks to:** the database only.
- **Lives in:** `crates/bridge-screen-core/src/waits/`, `crates/bridge-screen-core/src/view/groups.rs`

### Project pages
- **Does:** keeps five short pages per repo (intent, project map, current state, decision log, glossary) that every agent session in any worktree can read and keep true. The session briefing offers them once (up to three sessions) and then points at them; agents use `lead_pages`, `lead_page_read` and `lead_page_write`; you use `bridge-screen pages set-up | decline | show | path`; the dashboard shows them read-only ("Project pages" on a project).
- **Talks to:** its own folder only (no database, no git, nothing in the repo).
- **Holds:** the pages, as plain Markdown, plus small marker files for "offered" and "declined".
- **Lives in:** `crates/bridge-screen-core/src/pages/`, `crates/bridge-screen/src/mcp/pages.rs`, `crates/bridge-screen/src/onboard.rs`, `web/src/lib/drill/Pages.svelte`

### The event log and views (also called the store)
- **Does:** an append-only record of everything that happened; the current picture (views) is rebuilt from it and can always be regenerated.
- **Holds:** all bridge-screen data, in `~/.bridge-screen/`.
- **Lives in:** `crates/bridge-screen-core/src/store/`

### Screens
- **Terminal:** `bridge-screen status` (prints once) and `bridge-screen watch` (live, full-screen). `crates/bridge-screen/src/status.rs`, `watch.rs`
- **Browser dashboard:** `bridge-screen ui` starts a private local web page with the needs-you queue, projects, sessions and drill-downs. `crates/bridge-screen/src/ui/`, `web/`
- **Desktop-app card:** `/bridge-screen` in Claude shows an inline card (text where cards aren't supported). `web/src/card/`

### Plugin package
- **Does:** what Claude Code installs: hook wiring, the agent tools, the skill that teaches agents the protocol, the `lead-briefing` skill (how to report to you), the `/bridge-screen` command, and a launcher that fetches the program (not yet on Windows).
- **Lives in:** `plugin/`, `.claude-plugin/marketplace.json`

## Where data and secrets live
- `~/.bridge-screen/bridge-screen.db`: everything recorded (no prompt text or full agent messages).
- `~/.bridge-screen/config.toml`: settings.
- `~/.bridge-screen/ui.lock`: the dashboard's private access token (only `bridge-screen ui` prints the link with it).
- `~/.bridge-screen/snapshots/`: records of undone work (the work itself is in git stash).
- `~/.bridge-screen/lead/<repo-id>/`: each repo's project pages (not in the database; `rebuild` never touches them; not shared between people).

## Deliberately absent
- No server or cloud; nothing runs when no session or screen is open.
- No AI model calls from bridge-screen.
- Not yet built: archiving old projects (9). Plan files are imported in the superpowers format only (found in any folder) (no GitHub issues, Linear or TODO.md).
- No live price lookups: prices come from the config file (defaults taken 2026-10-21).
