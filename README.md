# bridge-screen

bridge-screen is a local, private tracker for projects you build with Claude Code. A
Claude Code plugin (hooks, an MCP server and a skill) records every session in your
repos: which plan task each session is working on, its units of work, and how much
agent-active time they took. Agents claim their task themselves (automatically when the
request clearly matches, after asking you otherwise) and keep the plan up to date.
Everything stays on your machine in `~/.bridge-screen/`. Nothing is ever written into
your repos.

## Handoff and recovery

When a session ends, crashes or is abandoned, the next one picks up where it stopped:

- **Checkpoints.** When a task is claimed done, a detour closes or the task switches, the
  agent is asked once to commit a `wip(bridge):` checkpoint with `Bridge-Task` and
  `Bridge-Next` trailers before it stops.
- **Briefing.** Every session starts with a short I-PASS briefing (stable, watcher or
  unstable) and, unless stable, reads it back before its first edit.
- **Leftovers.** Uncommitted files of a dead session are found, attributed to tasks and
  resolved by adopt (commit), resume (claim the task) or undo (a stash plus a snapshot —
  nothing is ever thrown away). A live session's worktree is never touched, and agents leave a
  crashed session's other worktree alone for 24 h (`crash_takeover_hours`): its owner may
  only be away. Resolve them yourself with `bridge-screen resolve <leftover-id>… adopt|resume|undo`.
- **Squash.** `bridge-screen squash [--onto BASE] [--dry-run]` collapses a branch's WIP
  commits into one (leaving a backup ref), and it happens automatically before
  `gh pr create`.

## Verification and decisions

A task claimed done is checked, not taken on trust:

- **The ladder.** L0 claimed (`claim_done` with the files that show the work) → L1 the
  code exists at HEAD (checked automatically) → L2 the unit tests pass (the agent reports
  the exit code; bridge-screen re-runs your test command, set with
  `bridge-screen config set test-command "cargo test"`, within a time limit) → L3
  end-to-end, as the agent drove it → L4 you (`bridge-screen verify <task>`, or
  `--broken "<note>"`). Every level is tied to a commit; when later commits touch the
  files it depended on, it drops a level at the next session start. Squashing keeps them.
- **Progress.** `bridge-screen status` shows done% (tasks at L2 or above, by estimate) and
  claimed% separately. Unverified work leads the next briefing and is nudged; after two
  ignored nudges it lands in your needs-you list.
- **Decisions.** Agents log what they decided, why and the alternatives
  (`log_decision`) when a task is done, before compaction and at the end of a session; a
  changed decision supersedes the old one. `bridge-screen decisions [--all]` lists them.

## Install

*After the first release:*

| Route | Command |
|---|---|
| Claude Code plugin marketplace | `/plugin marketplace add bedantsharma/session-plugin`, then `/plugin install bridge-screen@bridge-screen` |
| Homebrew | `brew install bedantsharma/tap/bridge-screen`, then `bridge-screen install` |
| PyPI | `pipx install bridge-screen` (or `uv tool install bridge-screen`), then `bridge-screen install` |
| npm | `npx bridge-screen install` |
| Shell installer | `curl -LsSf <release-url>/bridge-screen-installer.sh \| sh`, then `bridge-screen install` |
| crates.io | `cargo install bridge-screen`, then `bridge-screen install` |

`bridge-screen install` creates `~/.bridge-screen/` and registers the plugin through
Claude Code's `claude plugin` CLI. The plugin alone also works: its launcher downloads
the matching binary on first use and checks it against the release's SHA-256.

## Try it from a checkout

```sh
cargo install --path crates/bridge-screen
bridge-screen install --dev   # registers the plugin from this checkout
```

Then start Claude Code in any git repo. `bridge-screen status` (or `/bridge-screen` in
Claude Code) shows the live sessions; `bridge-screen plan show` shows the plan.

## Hook latency

Hooks must stay under 50 ms p95. To check on a database with 100k events:

```sh
cargo test --release -p bridge-screen --test hooks -- --ignored hook_latency --nocapture
```
