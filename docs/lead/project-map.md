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
- **Does:** gives agents tools: claim a task, edit the plan, open/close a detour, say a task is done, verify it, log a decision, leave a handoff note, show progress.
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

### The event log and views (also called the store)
- **Does:** an append-only record of everything that happened; the current picture (views) is rebuilt from it and can always be regenerated.
- **Holds:** all bridge-screen data, in `~/.bridge-screen/`.
- **Lives in:** `crates/bridge-screen-core/src/store/`

### Screens
- **Terminal:** `bridge-screen status` (prints once) and `bridge-screen watch` (live, full-screen). `crates/bridge-screen/src/status.rs`, `watch.rs`
- **Browser dashboard:** `bridge-screen ui` starts a private local web page with the needs-you queue, projects, sessions and drill-downs. `crates/bridge-screen/src/ui/`, `web/`
- **Desktop-app card:** `/bridge-screen` in Claude shows an inline card (text where cards aren't supported). `web/src/card/`

### Plugin package
- **Does:** what Claude Code installs: hook wiring, the agent tools, the skill that teaches agents the protocol, the `/bridge-screen` command, and a launcher that fetches the program.
- **Lives in:** `plugin/`, `.claude-plugin/marketplace.json`

## Where data and secrets live
- `~/.bridge-screen/bridge-screen.db`: everything recorded (no prompt text or full agent messages).
- `~/.bridge-screen/config.toml`: settings.
- `~/.bridge-screen/ui.lock`: the dashboard's private access token (only `bridge-screen ui` prints the link with it).
- `~/.bridge-screen/snapshots/`: records of undone work (the work itself is in git stash).

## Deliberately absent
- No server or cloud; nothing runs when no session or screen is open.
- No AI model calls from bridge-screen.
- Not yet built: automatic plan import and onboarding of existing repos (8), archiving old projects (9).
- No live price lookups: prices come from the config file (defaults taken 2026-10-21).
