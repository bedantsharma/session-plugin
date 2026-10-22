# bridge-screen

Local, private tracker for Claude Code projects: progress toward the goal, tangent detection, and session handoff. The design spec is `docs/superpowers/specs/2026-10-13-bridge-screen-design.md`; read it before changing behaviour.

## Layout

- `crates/bridge-screen-core/` — core library (store, plan, session, navigator, …; spec §4.2);
  `plan/rollup.rs` — a milestone's and the goal's status from their tasks, derived when read
  - `tangent/` — detours, suspended items and resume cues, unit labels, away time and the
    tolerance band (§7.1–7.4); `drift.rs` — the pure drift-score heuristic (§7.2);
    `navigator.rs` — note candidates, priority, one-note-per-unit and repeat limits, and the
    Stop/PostToolUse/UserPromptSubmit steps (§7.5); `tuning.rs` — per-repo threshold
    auto-tuning with guard rails (§7.6)
  - `git/` — every git operation (status, commit, stash, squash, push), always the `git`
    command with a scrubbed environment (§8); `checkpoint.rs` — boundaries, the Stop
    request and commit detection (§8.1); `recovery.rs` — end classification, crash scan,
    leftover analysis and adopt/resume/undo (§8.2–8.3); `handoff/` — the handoff model
    (schema v4 views) plus the I-PASS briefing and read-back (§8.4); `squash/` — WIP
    squash, by hand and before a PR (§8.1)
  - `view/` — the read models every screen renders: `glance` (projects, the needs-you
    queue, live sessions; ordering, lights and escalation live here) and `drill` (project,
    session, task, history, tuning) (§10.1–10.2); `owner.rs` — every owner action
    (drift answer with optional reason, label, leftovers, verify ✓/✗, reset tuning) with
    the stale-click check; the CLI, dashboard, `watch` and the card all call `owner::act`
  - `verify/` — the verification ladder (§8.5): L1 at HEAD (`l1.rs`), the `verify` tool's
    L2–L4 and the owner's verdict (`ladder.rs`), the bounded L2 test re-run with
    process-group kill (`rerun.rs`, never from a hook), staleness at SessionStart and its
    briefing note (`stale.rs`), nudges and owner escalation (`nudge.rs`), status
    derivation and done%/claimed% (`mod.rs`), schema v5 views (`project.rs`), the owner
    taking a verdict back (`withdraw.rs`)
  - `decision/` — decisions with supersede chains, duplicate merging and the capture ask
    (§8.6)
  - `telemetry/` — transcript usage (§9.2): the bounded reader (`read.rs`: cursor, whole
    lines, 512 KiB per hook, dedupe by message id), the `usage` projector (`project.rs`:
    `usage_replies` attributed by timestamp to unit/task/detour/agent, `transcript_cursors`),
    totals per scope and cost at read time from `config.toml` prices (`mod.rs`)
  - `forecast/` — calibration ratios, the seeded Monte Carlo (`rng.rs`, no `rand`), hours/day
    and dates, refusal below 3 finished tasks, `forecast_ran` history (`project.rs`) (§9.3)
  - `import/` — superpowers plan files (§12.1): the pure parser (`parse.rs`), the pure diff to
    plan ops with predicted ids (`diff.rs`), `plan_imported`/`plan_file_seen` → `plan_sources`,
    `plan_source_items` (`project.rs`), `scan` (SessionStart budget: 20 files, 1 MiB, stat
    first; baseline for a hand-planned repo) and `import_path` (`mod.rs`); files are only read
  - `init.rs` — `bridge-screen init` evidence (read-only) and `init_plan`'s `apply` (§12.2)
  - `pages/` — project pages (spec 2026-10-22): five Markdown pages per repo in
    `~/.bridge-screen/lead/<repo-id>/` (`mod.rs` storage, owner-only intent, append-only log under a
    file lock, offer and pointer note `briefing_note`; `templates.rs` + `templates/*.md` from the
    lead-briefing skill; `view.rs` the dashboard read model); marker files, no events, no schema
  - `waits/` — `waiting_on`/`wait_cleared`, the `waits` view, `clear_for` (same transaction as
    any L2+ `verified`), promote to a plan link, the wait note (§9.1)
  - `view/groups.rs` — branch groups on the glance (§10.1)
- `crates/bridge-screen/` — the single binary: CLI, `hook`, `mcp`, `status`, `ui`, `watch`
  - `src/onboard.rs` — `bridge-screen import`, `init`, `wait list|clear|promote` and
    `pages set-up|decline|show|path`
  - `src/ui/` — `bridge-screen ui`: lock/token/single instance (`mod.rs`), axum router with
    the JSON API and embedded SPA (`api.rs`); `build.rs` embeds `web/dist` and
    `web/dist-card` or a placeholder page when they aren't built
  - `src/watch.rs` — the ratatui live view; `src/status.rs` — the text glance;
    `src/report.rs` — the opt-in HTML progress report
  - `src/hook/` — `bridge-screen hook <event>`: payload parsing, per-event handlers, injected context
  - `src/hook/nav.rs` — the navigator's part of a hook (label at Stop, tolerance band, notes)
  - `src/hook/question.rs` — did the turn end with a question to the owner (pure string
    checks; no text stored) and the `AskUserQuestion` tool name
  - `src/hook/handoff.rs` — checkpoint request/detection, SessionEnd classification,
    SessionStart recovery and briefing, read-back nudge, squash before a PR
  - `src/mcp.rs` — `bridge-screen mcp`: stdio MCP server (`claim_task`, `plan_edit`, `status`,
    `start_detour`, `close_detour`, `resume`, `promote_detour`, `label_unit`, `claim_done`,
    `handoff_readback`, `handoff_note`, `resolve_leftover`, `verify`, `log_decision`,
    `show_progress` + the `ui://bridge-screen/progress` card resource, `owner_action`
    (card-only: needs the card's nonce), `progress_report`, `report_published`, and in
    `src/mcp/`: `init_plan` (`onboard.rs`), `waiting_on`, `clear_wait`, `promote_wait`
    (`waits.rs`), `lead_pages`, `lead_page_read`, `lead_page_write` (`pages.rs`))
  - `tests/fixtures/hooks/` — one recorded-style hook payload per event
- `plugin/` — the Claude Code plugin: `.claude-plugin/plugin.json`, `hooks/hooks.json`,
  `.mcp.json`, `skills/bridge-screen/SKILL.md`, `skills/lead-briefing/` (the owner's reporting
  skill, copied unchanged), `commands/bridge-screen.md`, and the
  `scripts/bridge-screen` launcher (POSIX sh; uses a matching PATH binary or downloads one)
- `.claude-plugin/marketplace.json` — marketplace `bridge-screen` listing `./plugin`
- `web/` — Svelte 5 + Vite + Tailwind + shadcn-svelte + LayerChart, embedded into the binary
  at build time: the dashboard (`src/App.svelte`, `src/lib/glance`, `src/lib/drill`,
  `src/lib/api.ts`) and the single-file desktop card (`src/card`, `vite.card.config.ts`);
  palette tokens once in `src/app.css`; `tests/` (vitest), `e2e/` (Playwright)
- `docs/superpowers/` — specs and implementation plans
- `docs/lead/` — the owner's living docs: intent, project map, current state, decision log,
  glossary (see "Reporting to the owner"), and `message-audit.md` (every owner-facing message,
  before and after; codes that must never reach a screen are in `web/src/lib/jargon.json`)

## Commands

- First-time setup: `scripts/setup.sh` (enables `.githooks`, installs web deps)
- Rust: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`
- Web (in `web/`): `npm run dev`, `npm run build` (dashboard + card), `npm run lint`,
  `npm run check`, `npm run format`, `npm run test` (vitest), `npm run e2e` (Playwright,
  chromium; needs `cargo build --release` first)
- Screens: `bridge-screen ui [--port N] [--no-open]`, `bridge-screen watch`
- Hooks: `bridge-screen hook <session-start|user-prompt-submit|pre-tool-use|post-tool-use|stop|subagent-stop|pre-compact|notification|session-end>` (hook JSON on stdin; always exits 0, errors go to `~/.bridge-screen/hook-errors.log`)
- MCP server: `bridge-screen mcp` (stdio; Claude Code starts it through the plugin)
- Status: `bridge-screen status`; plugin registration: `bridge-screen install [--dev | --no-plugin]`
- Owner answers: `bridge-screen drift <session-prefix> continue|make-task|stop [--reason R] [--parent P --title T]`,
  `bridge-screen label <session-prefix> <unit_no> on-task|detour|drift|requested` (`requested`,
  alias `asked-for`: work the owner asked for — never drift)
- Plans: `bridge-screen import [-C DIR] [FILE] [--into NODE] [--dry-run]` (plan files; read
  only), `bridge-screen init [-C DIR] [--goal TEXT]` (a half-built repo's evidence and the
  agent's steps), `bridge-screen wait list|clear <id>|promote <id> [-C DIR]` (owner)
- Project pages: `bridge-screen pages set-up | decline | show | path [-C DIR]` (owner; the pages
  live in `~/.bridge-screen/lead/<repo-id>/`, never in the repo)
- Recovery: `bridge-screen resolve <leftover-id|prefix>… adopt|resume|undo [-C DIR]`;
  `bridge-screen squash [-C DIR] [--onto BASE] [--dry-run]`
- Config: `bridge-screen config [-C DIR]` (shows the repo's tuned drift thresholds and test
  settings), `bridge-screen config reset-drift [-C DIR]`,
  `bridge-screen config set test-command "<cmd>" | test-timeout <secs> | publish-reports never|ask [-C DIR]`,
  `bridge-screen config unset test-command|test-timeout|publish-reports [-C DIR]` (owner only)
- Verification and decisions: `bridge-screen verify <task> [--broken "<note>"] [--level N] [-C DIR]`
  (the owner's verdict), `bridge-screen verify <task> --withdraw [-C DIR]` (take back the owner's
  newest sign-off or "broken"; owner only — the MCP refuses it), `bridge-screen decisions [-C DIR] [--task ID] [--all]`
- Cost and forecast: `bridge-screen usage [-C DIR] [--session PREFIX] [--task ID] [--by model|task|session|agent]`,
  `bridge-screen usage --catch-up [-C DIR]` (reads every known transcript of the repo to the end;
  owner-run, never a hook), `bridge-screen forecast [-C DIR]`; prices under `[prices."<model>"]`
  in `config.toml` (`bridge-screen config` shows them and which are defaults)
- Plugin check: `claude plugin validate ./plugin` (read-only)
- Hook latency (release, on demand; PostToolUse, the Stop scoring path, a prompt scoring a unit cut short, Stop and SubagentStop with a 512 KiB transcript backlog (Stop also finishing 512 KiB of a subagent's), UserPromptSubmit with verification and dependency notes, UserPromptSubmit with 20 open waits and 5 links into another repo, and — budget 150 ms — PostToolUse after `git commit` with a checkpoint pending, SessionStart's staleness scan over 50 verifications, and SessionStart with 20 unchanged plan files or one changed 300 KiB plan file): `cargo test --release -p bridge-screen --test hooks -- --ignored hook_latency --nocapture`
- Releases: see `docs/RELEASING.md` (`dist plan` to preview); keep `plugin/.claude-plugin/plugin.json`'s `version` equal to the workspace version (a test checks it)

## Rules

- Rust toolchain is pinned in `rust-toolchain.toml`; clippy pedantic warnings fail the build.
- The pre-commit hook runs fmt/clippy/web lint; pre-push runs all tests and the web build. Don't bypass them with `--no-verify`.
- Never write bridge-screen state inside a repo; runtime data lives in `~/.bridge-screen/` (spec §6).
- Hooks must stay fast (≤ 50 ms p95) and must never fail a Claude session: log errors and exit 0. A known session's hooks run `git` only when a checkpoint is pending (Stop), after a Bash `git commit`, at SessionStart/SessionEnd, and before `gh pr create`.
- Transcripts: read only ids, model names, token counts and timestamps — never message
  text, tool inputs/outputs or prompts; at most 512 KiB per file per hook. Money is never
  stored, only computed from prices when read; an unpriced model's cost is "unknown", never $0.
- No fake forecasts: no dates below 3 finished tasks or without recent activity (say why);
  the forecast runs after a verification at L2+ (MCP `verify`, owner verdict), never in a hook.
- Never run tests from a hook: the L2 re-run happens only in the `verify` MCP call. Only the
  owner sets the test command (CLI); every verification is tied to a commit SHA.
- Tangents are never blocked. Stop continuations (checkpoint request first, then the label request) go out at most once per unit, never when `stop_hook_active`.
- Never lose work: undo is a `git stash push` plus a snapshot record; squash leaves a backup ref; never `reset --hard`, `clean` or `checkout --` on unstashed files; never touch a live session's worktree.
- Projectors use event timestamps only; "now" (live away time) is for hook decisions and never written into views.
- Tests never touch the real `~/.claude` or `~/.bridge-screen` and never run the real `claude` CLI: use temp homes and a fake `claude` on a test-controlled PATH (`bridge-screen install --no-plugin` otherwise).

## Reporting to the owner

The owner leads this project but doesn't read the code. Every status update, finding, review result, checklist or "what's going on?" answer follows the lead-briefing shape (the `lead-briefing` skill, where available, has the full version and templates):

1. **Effect first, in plain words.** The first sentence says what is true or wrong for the owner (their time, their data, what works, what ships). No file names, config keys or code terms in it.
2. **Why it matters,** and whether it blocks anything.
3. **The idea before the detail:** 2–4 sentences a newcomer would need; define any project term the first time (see `docs/lead/glossary.md`).
4. **The cause,** systemic if there is one. No blame.
5. **State:** Open / Being fixed / Fix ready, waiting for merge / Merged, not deployed / Live and checked — and what's exposed meanwhile.
6. **The ask:** a specific decision (options, recommendation, cost of waiting) or "Nothing".
7. **Checked vs believed:** say what you checked by running it, what you believe from reading, and what you relayed from another agent without checking.
8. **Details last,** under a "Details" heading: paths, commands, commit and PR numbers.
9. **Never round up to green.** "Done" means checked, not written.
10. **Short:** under ~300 words above Details; several items → a one-line overview first.

Shapes: a problem card (headline, why it matters, what's going on, why it happened, state, what I need from you, how sure, details) for blockers and findings; a status update (bottom line with Green/Amber/Red, what changed, blocked, needs your decision, next, how sure, details); checklists with a plain action, why, and "done when" per item, ordered by what blocks shipping.

**Subagents:** any subagent whose result reaches the owner gets these rules in its prompt (effect first, checked vs believed, details last). Verify what it reports before relaying it, and say which parts you re-checked.

**Living docs** in `docs/lead/` (one page each; agents keep them current, the owner reads them):
- `intent.md` — owned by the owner; never change it without them.
- `project-map.md` — update when a part is added, removed or changes job.
- `current-state.md` — overwrite (don't append), dated, after each merge or working day.
- `decision-log.md` — add an entry for every choice someone will later ask "why?" about; never delete, mark "Superseded by #N".
- `glossary.md` — add every new project term the first time it appears in a report.
- `message-audit.md` — add a row when a new owner-facing message appears or changes.

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **session-plugin** (2418 symbols, 6996 relationships, 211 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `npx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/session-plugin/context` | Codebase overview, check index freshness |
| `gitnexus://repo/session-plugin/clusters` | All functional areas |
| `gitnexus://repo/session-plugin/processes` | All execution flows |
| `gitnexus://repo/session-plugin/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->
