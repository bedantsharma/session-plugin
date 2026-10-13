# bridge-screen

Local, private tracker for Claude Code projects: progress toward the goal, tangent detection, and session handoff. The design spec is `docs/superpowers/specs/2026-10-13-bridge-screen-design.md`; read it before changing behaviour.

## Layout

- `crates/bridge-screen-core/` — core library (store, plan, session, navigator, …; spec §4.2)
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
- `crates/bridge-screen/` — the single binary: CLI, `hook`, `mcp`, `status`, `ui`
  - `src/hook/` — `bridge-screen hook <event>`: payload parsing, per-event handlers, injected context
  - `src/hook/nav.rs` — the navigator's part of a hook (label at Stop, tolerance band, notes)
  - `src/hook/handoff.rs` — checkpoint request/detection, SessionEnd classification,
    SessionStart recovery and briefing, read-back nudge, squash before a PR
  - `src/mcp.rs` — `bridge-screen mcp`: stdio MCP server (`claim_task`, `plan_edit`, `status`,
    `start_detour`, `close_detour`, `resume`, `promote_detour`, `label_unit`, `claim_done`,
    `handoff_readback`, `handoff_note`, `resolve_leftover`)
  - `tests/fixtures/hooks/` — one recorded-style hook payload per event
- `plugin/` — the Claude Code plugin: `.claude-plugin/plugin.json`, `hooks/hooks.json`,
  `.mcp.json`, `skills/bridge-screen/SKILL.md`, `commands/bridge-screen.md`, and the
  `scripts/bridge-screen` launcher (POSIX sh; uses a matching PATH binary or downloads one)
- `.claude-plugin/marketplace.json` — marketplace `bridge-screen` listing `./plugin`
- `web/` — Svelte 5 + Vite + Tailwind dashboard, embedded into the binary at build time
- `docs/superpowers/` — specs and implementation plans

## Commands

- First-time setup: `scripts/setup.sh` (enables `.githooks`, installs web deps)
- Rust: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`
- Web (in `web/`): `npm run dev`, `npm run build`, `npm run lint`, `npm run check`, `npm run format`
- Hooks: `bridge-screen hook <session-start|user-prompt-submit|pre-tool-use|post-tool-use|stop|subagent-stop|pre-compact|notification|session-end>` (hook JSON on stdin; always exits 0, errors go to `~/.bridge-screen/hook-errors.log`)
- MCP server: `bridge-screen mcp` (stdio; Claude Code starts it through the plugin)
- Status: `bridge-screen status`; plugin registration: `bridge-screen install [--dev | --no-plugin]`
- Owner answers: `bridge-screen drift <session-prefix> continue|make-task|stop [--parent P --title T]`,
  `bridge-screen label <session-prefix> <unit_no> on-task|detour|drift`
- Recovery: `bridge-screen resolve <leftover-id|prefix>… adopt|resume|undo [-C DIR]`;
  `bridge-screen squash [-C DIR] [--onto BASE] [--dry-run]`
- Config: `bridge-screen config [-C DIR]` (shows the repo's tuned drift thresholds),
  `bridge-screen config reset-drift [-C DIR]`
- Plugin check: `claude plugin validate ./plugin` (read-only)
- Hook latency (release, on demand; PostToolUse, the Stop scoring path, and PostToolUse after `git commit` with a checkpoint pending — budget 150 ms): `cargo test --release -p bridge-screen --test hooks -- --ignored hook_latency --nocapture`
- Releases: see `docs/RELEASING.md` (`dist plan` to preview); keep `plugin/.claude-plugin/plugin.json`'s `version` equal to the workspace version (a test checks it)

## Rules

- Rust toolchain is pinned in `rust-toolchain.toml`; clippy pedantic warnings fail the build.
- The pre-commit hook runs fmt/clippy/web lint; pre-push runs all tests and the web build. Don't bypass them with `--no-verify`.
- Never write bridge-screen state inside a repo; runtime data lives in `~/.bridge-screen/` (spec §6).
- Hooks must stay fast (≤ 50 ms p95) and must never fail a Claude session: log errors and exit 0. A known session's hooks run `git` only when a checkpoint is pending (Stop), after a Bash `git commit`, at SessionStart/SessionEnd, and before `gh pr create`.
- Tangents are never blocked. Stop continuations (checkpoint request first, then the label request) go out at most once per unit, never when `stop_hook_active`.
- Never lose work: undo is a `git stash push` plus a snapshot record; squash leaves a backup ref; never `reset --hard`, `clean` or `checkout --` on unstashed files; never touch a live session's worktree.
- Projectors use event timestamps only; "now" (live away time) is for hook decisions and never written into views.
- Tests never touch the real `~/.claude` or `~/.bridge-screen` and never run the real `claude` CLI: use temp homes and a fake `claude` on a test-controlled PATH (`bridge-screen install --no-plugin` otherwise).

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
