# Project Pages, Lead-Briefing Bundle and Distribution — Implementation Plan

> **For agentic workers:** Execute task-by-task, test-first. Steps use checkbox (`- [ ]`) syntax. This plan gives interfaces, behaviour rules (pseudocode) and test cases — no full code. Read the cited spec sections and the existing code first and follow its patterns: `Home` paths (`paths.rs`), the hook never-fail wrapper (`hook/mod.rs`), MCP tools with `Args<T>` and `self.caller` (`mcp.rs`, `mcp/onboard.rs`), the owner-words check used by `init_plan`, the `/api/...` routes and `AppState` (`ui/api.rs`), the dashboard's `useModel` + `api.ts` + `types.ts` (`web/src/lib`), and `words.rs` / `words.ts` / `jargon.json` for owner-facing wording. Add dependencies only with `cargo add` / `npm install` (prefer none; file locking is in `std`).

**Goal:** A friend can install bridge-screen, be offered project pages once, and have every agent in every worktree read and keep them true, report in the lead-briefing style, and see the pages in the dashboard; and the release pipeline covers brew, the plugin, PyPI, npm and crates.io, with Windows stated honestly.

**Architecture:** A new core module `pages` keeps five Markdown pages in `~/.bridge-screen/lead/<repo-id>/` (marker files for offer/decline; no events, no schema change). The SessionStart hook appends one pointer or offer line to the briefing. Three MCP tools, one CLI command and one read-only dashboard view sit on the same module. The lead-briefing skill is copied into the plugin. Release work is config, docs and one workflow.

**Tech Stack:** Rust (core + binary crates), axum JSON API, Svelte 5 dashboard, POSIX sh launcher, GitHub Actions + cargo-dist.

**Spec:** `docs/superpowers/specs/2026-10-22-project-pages-and-lead-bundle-design.md` (builds on `2026-10-13-bridge-screen-design.md` §6, §8.4, §10, §11, §16).

**Branch / worktree:** `feat/project-pages-and-distribution` in `/Users/bedantsharma/PycharmProjects/session-plugin-pages` (from `main`). Work only there.

## Global Constraints

- Clippy pedantic `-D warnings`; `cargo fmt`; web `lint`, `check`, `test` pass.
- Hooks ≤ 50 ms p95 and never fail a session (log, exit 0); **no new git process** in any hook.
- Nothing is written inside any repo: pages live only under `~/.bridge-screen/lead/<repo-id>/`. No schema change; `bridge-screen rebuild` must not touch that folder.
- Tests never touch the real `~/.claude` or `~/.bridge-screen`: use the temp-home helpers (`BRIDGE_SCREEN_HOME`) and a fake `claude`.
- Briefing stays ≤ 40 lines. The offer note ranks last and is dropped first.
- Page names are the fixed list `intent`, `project-map`, `current-state`, `decision-log`, `glossary`; size cap 64 KiB per write.
- `intent` is written only with the owner's words; `decision-log` is append-only.
- Owner-facing wording: effect first, plain words, no raw codes; new messages go into `docs/lead/message-audit.md` and pass `web/src/lib/jargon.json`.
- `plugin/.claude-plugin/plugin.json` version equals the workspace version; `claude plugin validate ./plugin` passes.
- Commit per task, ending with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`; never `--no-verify`; chain shell steps with `&&`.

## Review Focus

1. **Two worktrees, one set of pages:** a session in a second worktree of the same repo reads and updates the same pages. Test: `worktrees_share_pages` (Task 3).
2. **Concurrent decision-log appends both land:** two writers appending at once lose nothing and never see a refusal. Test: `concurrent_appends_both_land` (Task 1).
3. **A broken pages folder never breaks a session:** unreadable folder, read-only home, a file where the folder should be → the briefing is produced without the note, hook exits 0. Test: `unreadable_pages_folder_does_not_fail_hook` (Task 2).
4. **A page name can't escape the folder:** `../x`, `intent.md`, `INTENT`, an empty name and an absolute path are all refused. Test: `page_name_is_a_fixed_list` (Task 1).
5. **Empty or whitespace "owner words" don't count as a yes:** set-up, decline and intent writes are refused. Test: `blank_owner_words_refused` (Task 1).

## File Structure (expected)

- `crates/bridge-screen-core/src/pages/mod.rs` — storage, states, rules (Task 1); `templates.rs` — the five templates from the skill's living-docs reference; `view.rs` — the dashboard read model (Task 5)
- `crates/bridge-screen-core/src/paths.rs` — `Home::lead_dir` (Task 1)
- `crates/bridge-screen-core/src/error.rs` — plain pages errors (Task 1)
- `crates/bridge-screen/src/hook/handoff.rs`, `hook/handlers.rs` — the briefing note (Task 2)
- `crates/bridge-screen/src/mcp/pages.rs`, `mcp.rs` — tools (Task 3)
- `crates/bridge-screen/src/cli.rs`, `onboard.rs` — `pages` command (Task 4)
- `crates/bridge-screen/src/ui/api.rs`; `web/src/lib/drill/Pages.svelte`, `Project.svelte`, `api.ts`, `types.ts`, `words.ts` (Task 5)
- `plugin/skills/lead-briefing/**`, `plugin/skills/bridge-screen/SKILL.md`, `crates/bridge-screen/tests/install.rs` (Task 6)
- `crates/bridge-screen-core/Cargo.toml`, `crates/bridge-screen/Cargo.toml`, `.github/workflows/crates.yml`, `docs/RELEASING.md` (Task 7)
- `plugin/scripts/bridge-screen`, `README.md`, `docs/RELEASING.md` (Task 8)
- spec, decision log, glossary, project map, message audit, current state, `CLAUDE.md` layout (Task 9)

---

### Task 1: The pages module

**Files:** Create `pages/mod.rs`, `pages/templates.rs`; modify `paths.rs`, `error.rs`, `lib.rs`; test `pages/tests.rs` (core, temp `Home`).

**Interfaces:**
- Produces (all take `home: &Home, repo_id: &str`):
  - `enum Page { Intent, ProjectMap, CurrentState, DecisionLog, Glossary }` with `Page::parse(&str) -> Option<Page>` (exact lowercase kebab names only), `name()`, `file_name()`.
  - `enum PagesState { NotSetUp { offers_made: u8 }, SetUp, Declined }`; `state(..) -> PagesState`.
  - `set_up(.., owner_said: &str) -> Result<SetUpOutcome { created: Vec<Page>, kept: Vec<Page> }>`; `decline(.., owner_said: &str) -> Result<()>`.
  - `read(.., Page) -> Result<Option<String>>`; `status(..) -> Vec<PageInfo { page, updated: Option<jiff::Timestamp>, bytes: u64 }>`.
  - `enum WriteMode { Replace, Append }`; `write(.., Page, content: &str, mode, owner_said: Option<&str>) -> Result<()>`.
  - `Home::lead_dir(&self, repo_id) -> PathBuf` = `root/lead/<repo_id>`.
- Consumes: `Home`, `jiff`, existing `Error`/`Result`.

Behaviour rules (pseudocode):
```
state: Declined if .declined exists; SetUp if all five page files exist;
       else NotSetUp{offers_made = count read from .offered (0 if absent)}
set_up: owner_said blank -> error; create dir; for each page: if file missing write template
        atomically else keep; remove .declined; remove .offered
decline: owner_said blank -> error; create dir; write .declined
write: reject name not in list (parse), content > 64 KiB, non-UTF-8 never reaches here;
       Intent requires owner_said non-blank; DecisionLog only Append (Replace -> plain error);
       take an exclusive std file lock on <dir>/.lock for the whole write;
       Replace: copy current to <page>.prev, write temp, rename; Append: add content to the end
       with a blank-line separator, same atomic write
```
- [ ] **Step 1: Write the failing tests** (names and assertions):
  - `set_up_creates_five_pages_once`: after `set_up`, `state == SetUp`, five files exist; editing one page then calling `set_up` again leaves the edit and returns it in `kept`.
  - `decline_stops_everything`: `state == Declined`, no page files exist; `set_up` afterwards with words works and clears the decline.
  - `blank_owner_words_refused`: `set_up(" ")`, `decline("")`, `write(Intent, .., None)` and `write(Intent, .., Some("  "))` all `Err`.
  - `page_name_is_a_fixed_list`: `Page::parse` is `None` for `"../x"`, `"intent.md"`, `"INTENT"`, `""`, `"/etc/passwd"`.
  - `intent_needs_owner_words`: `Err` without, `Ok` with `Some("yes, that is the purpose")`.
  - `decision_log_is_append_only`: Replace is `Err` with a plain message; two appends keep both entries in order.
  - `concurrent_appends_both_land`: 8 threads append distinct lines; the file has all 8, none lost.
  - `write_keeps_previous_version`: after a Replace, `<page>.prev` holds the old text.
  - `oversize_write_refused`: 64 KiB + 1 → `Err`, file unchanged.
  - `partial_set_up_is_completed`: delete two pages; `state != SetUp`; `set_up` recreates exactly those two.
  - `rebuild_leaves_pages_alone` (in `crates/bridge-screen/tests/`): the `lead/<id>` folder is byte-identical before and after `bridge-screen rebuild`.
- [ ] **Step 2: Run** `cargo test -p bridge-screen-core pages` — expect FAIL (module missing).
- [ ] **Step 3: Implement** the interfaces above. Atomic write = temp file in the same folder + rename. Templates: read the lead-briefing skill's `references/living-docs.md` and turn its five page templates into `const` strings (`include_str!` of files under `pages/templates/`); each starts with a one-line "what this page is for".
- [ ] **Step 4: Run** `cargo test -p bridge-screen-core pages && cargo clippy --workspace --all-targets -- -D warnings` — expect PASS.
- [ ] **Step 5: Commit** `feat(core): project pages store (five pages, owner-only intent, append-only log)`.

### Task 2: The offer and the pointer in the briefing

**Files:** Modify `pages/mod.rs`, `hook/handoff.rs`, `hook/handlers.rs`; test `crates/bridge-screen/tests/hooks.rs` (or a new `tests/pages_hook.rs`), core `pages/tests.rs`.

**Interfaces:**
- Produces: `pages::briefing_note(home, repo_id, room: usize, other_onboarding_note: bool) -> Result<Option<String>>` where `room` = lines left under the 40-line cap. Changes `start_context(store, input)` to `start_context(store, input, home: &Home)`.
- Consumes: Task 1 `state`, `status`.

Rules:
```
SetUp: if room >= 1 -> one line: "Project pages: read intent and current state first
       (lead_page_read); keep current state true after each merge. Last updated <date>."
NotSetUp{offers_made < 3}: if room >= 2 and not other_onboarding_note ->
       a two-line offer in plain words asking the agent to ask the owner once whether to set up
       project pages, and to relay the answer with lead_pages; then write .offered with
       offers_made + 1 (one tiny file write). Otherwise nothing, and nothing is written.
NotSetUp{offers_made >= 3} or Declined: nothing.
Any error reading/writing the folder -> Ok(None) and log; never propagate.
```
`start_context` computes `room = 40 - b.text.lines().count()` (saturating), and passes `other_onboarding_note = true` when the briefing already carries the no-plan/init note (`NO_PLAN`) or an import note (`with_imports` output). The note is appended after the briefing text; the compaction re-briefing gets the one-line pointer only, never the offer.
- [ ] **Step 1: Write the failing tests:**
  - `offer_is_made_up_to_three_times`: three SessionStarts carry the offer; the fourth does not; `.offered` counts 1,2,3.
  - `offer_stops_after_decline_or_setup`: no note after `decline`; after `set_up` the note is the pointer line.
  - `offer_yields_to_onboarding_note`: in a session whose briefing has `NO_PLAN`, no offer, and `.offered` is not written.
  - `offer_dropped_when_briefing_full`: with `room = 1` no offer (nothing written); with `room = 0` no pointer.
  - `pointer_shows_last_updated_date`: line contains the page's modified date.
  - `unreadable_pages_folder_does_not_fail_hook`: `lead/<id>` is a file → hook exits 0, briefing has no note.
  - `briefing_with_pages_note_stays_within_40_lines`.
  - `compaction_rebriefing_has_pointer_only`.
  - Extend the existing hook latency test: SessionStart with the offer path stays inside its budget (`hook_latency`, ignored test).
- [ ] **Step 2: Run** the new tests — expect FAIL.
- [ ] **Step 3: Implement** `briefing_note`, thread `home` from the SessionStart handler (`handlers.rs:157` already runs inside `run(event, text, home)`), append the note.
- [ ] **Step 4: Run** `cargo test --workspace && cargo test --release -p bridge-screen --test hooks -- --ignored hook_latency --nocapture` — expect PASS, no latency regression.
- [ ] **Step 5: Commit** `feat(hook): offer project pages once, point sessions at them`.

### Task 3: MCP tools

**Files:** Create `mcp/pages.rs`; modify `mcp.rs`; test `tests/mcp.rs`.

**Interfaces:**
- Produces three tools, repo resolved from the caller's session (`self.caller`):
  - `lead_pages { action: "set_up" | "decline" | "status", owner_said?: string }` → plain-text result. `set_up` returns next steps for the agent: draft `current-state` from the repo's evidence and write it; draft `intent` and show the owner once, writing it only with their words; add known terms to `glossary`; use the lead-briefing skill. `decline` confirms nothing more will be offered. `status` lists pages with last-updated dates.
  - `lead_page_read { page }` → text, or a plain "not set up yet" message.
  - `lead_page_write { page, content, mode?: "replace" | "append", reason, owner_said? }` → "Saved." or a plain refusal from Task 1.
- Consumes: Task 1 API; `Args<T>`, `msg`, `Bridge::caller`.
- [ ] **Step 1: Write the failing tests:** `lead_pages_set_up_needs_owner_words`; `set_up_then_read_returns_template`; `write_current_state_then_read_back`; `write_intent_refused_without_owner_words`; `decision_log_append_via_tool`; `unknown_page_refused`; `worktrees_share_pages` (two sessions bound to two worktrees of one repo, created with the existing test helpers, read the same text after one writes).
- [ ] **Step 2: Run** `cargo test -p bridge-screen --test mcp pages` — expect FAIL.
- [ ] **Step 3: Implement** the three tools in `mcp/pages.rs` and register in `mcp.rs`'s tool router with descriptions in plain words.
- [ ] **Step 4: Run** `cargo test --workspace` — expect PASS.
- [ ] **Step 5: Commit** `feat(mcp): lead_pages, lead_page_read, lead_page_write`.

### Task 4: The `pages` command (owner's route)

**Files:** Modify `cli.rs`, `onboard.rs` (or `commands.rs` per the existing pattern); test `tests/cli.rs`.

**Interfaces:**
- Produces: `bridge-screen pages set-up | decline | show | path [-C DIR]`. `set-up` and `decline` count as the owner's words (the command itself is the yes/no) and print one plain line of what happened and where. `show` prints the folder and, per page, title, last-updated date and size; `path` prints the folder only.
- [ ] **Step 1: Write the failing tests:** `cli_set_up_creates_pages_and_says_where`; `cli_decline_then_set_up_works`; `cli_show_lists_five_pages`; `cli_path_prints_folder_for_any_worktree`; each uses `BRIDGE_SCREEN_HOME` temp home.
- [ ] **Step 2: Run** `cargo test -p bridge-screen --test cli pages` — expect FAIL.
- [ ] **Step 3: Implement** the subcommand with `repo::identify(dir)` for the repo id and `Home::resolve()`.
- [ ] **Step 4: Run** `cargo test --workspace` — expect PASS.
- [ ] **Step 5: Commit** `feat(cli): bridge-screen pages set-up|decline|show|path`.

### Task 5: Read-only dashboard view

**Files:** Create `pages/view.rs`, `web/src/lib/drill/Pages.svelte`, `web/tests/pages.test.ts`; modify `ui/api.rs`, `web/src/lib/drill/Project.svelte`, `api.ts`, `types.ts`, `words.ts`, `words.rs` if shared; test `crates/bridge-screen/tests/` (api) and vitest.

**Interfaces:**
- Produces: `pages::view(home, repo_id) -> PagesView { state: "set_up" | "not_set_up" | "declined", pages: Vec<{ title, updated: Option<String>, text }> }` with plain titles ("What this project is for", "How it fits together", "Where things stand", "Why we chose what we chose", "Words we use"); route `GET /api/pages/{repo}` (same token auth as the other routes); TS `PagesView` type and `api.pages(repo)`.
- Behaviour: a "Project pages" link on the project drill-down opens the view; `not_set_up` shows one plain sentence and the command to run; `declined` shows that the owner chose not to; text is shown as wrapped plain text (no markdown library); read-only.
- [ ] **Step 1: Write the failing tests:** Rust `api_pages_returns_five_pages_after_set_up`, `api_pages_requires_token`, `api_pages_not_set_up_has_no_pages`; vitest `pages_view_renders_titles_and_dates`, `pages_view_not_set_up_shows_command`; extend `no_jargon_on_screens` to include the new strings.
- [ ] **Step 2: Run** `cargo test -p bridge-screen api && npm --prefix web run test` — expect FAIL.
- [ ] **Step 3: Implement** the view model, route, component and link. Page text is not interpreted as HTML (render as text).
- [ ] **Step 4: Run** `cargo test --workspace && npm --prefix web run lint && npm --prefix web run check && npm --prefix web run test && npm --prefix web run build` — expect PASS.
- [ ] **Step 5: Commit** `feat(ui): read-only project pages view`.

### Task 6: Bundle the lead-briefing skill

**Files:** Create `plugin/skills/lead-briefing/SKILL.md`, `references/why-it-works.md`, `references/living-docs.md`; modify `plugin/skills/bridge-screen/SKILL.md`; test `tests/install.rs`.

**Interfaces:**
- Copy the owner's skill unchanged from the installed copy (`find ~/Library/Application\ Support/Claude/local-agent-mode-sessions/skills-plugin -path "*skills/lead-briefing*" -type f`); the owner confirmed it is current. Keep its front matter (`name: lead-briefing`).
- In the bridge-screen skill add at most ~12 lines: use `lead-briefing` whenever the owner asks for status, a problem or a checklist; use `lead_page_read`/`lead_page_write` for the project pages; update current-state after each merge or working day, add a decision-log entry for choices someone will later question, glossary for new terms; the offer note and `lead_pages` with the owner's words. Keep the file within any existing length test; compress older wording if needed.
- [ ] **Step 1: Write the failing tests:** `plugin_ships_lead_briefing_skill` (three files exist; front matter name is `lead-briefing`); `bridge_screen_skill_mentions_pages_and_lead_briefing`.
- [ ] **Step 2: Run** `cargo test -p bridge-screen --test install skill` — expect FAIL.
- [ ] **Step 3: Copy** the files and edit the bridge-screen skill.
- [ ] **Step 4: Run** `cargo test -p bridge-screen --test install && claude plugin validate ./plugin` — expect PASS.
- [ ] **Step 5: Commit** `feat(plugin): ship the lead-briefing skill with the plugin`.

### Task 7: crates.io route

**Files:** Modify both `Cargo.toml` files, `docs/RELEASING.md`; create `.github/workflows/crates.yml`.

**Interfaces:**
- `bridge-screen-core` gets the same `description`/`license`/`repository` metadata it has now plus `readme` if needed and is publishable; `bridge-screen` depends on it with `{ version = "<workspace version>", path = "../bridge-screen-core" }` (version kept equal through `version.workspace`).
- Workflow `crates.yml` on tag `v*`: `cargo publish -p bridge-screen-core`, wait for the index, then `cargo publish -p bridge-screen`; secret `CARGO_REGISTRY_TOKEN`. The build needs no web build (the binary falls back to the placeholder page, spec §16.3); say so in the release notes text.
- [ ] **Step 1: Check by running** `cargo publish -p bridge-screen-core --dry-run` and `cargo package -p bridge-screen --list` — record any failure (missing metadata, files excluded). Expect core to pass; the binary dry-run fails until core is on the index, which is expected — verify with `cargo package -p bridge-screen --no-verify`.
- [ ] **Step 2: Add** the metadata, version on the path dependency and the workflow; add a test that the workspace version equals the version in the path dependency (extend the existing plugin-version test).
- [ ] **Step 3: Document** in `RELEASING.md`: one-time `CARGO_REGISTRY_TOKEN`, check the crate names are free, publish order.
- [ ] **Step 4: Run** `cargo test --workspace` and the dry-runs above — expect PASS.
- [ ] **Step 5: Commit** `feat(release): publish to crates.io (core, then binary)`.

### Task 8: Windows stated honestly, release readiness

**Files:** Modify `plugin/scripts/bridge-screen`, `README.md`, `docs/RELEASING.md`; test `crates/bridge-screen/tests/install.rs` (launcher tests, existing style).

**Interfaces:**
- The launcher's `uname -s` case for `MINGW*|MSYS*|CYGWIN*`: from a hook, exit 0 silently after logging to `launcher.log`; from anything else, print "bridge-screen's plugin launcher doesn't support Windows yet. Install the program with the PowerShell installer from the release page and make sure `bridge-screen` is on your PATH, then run `bridge-screen install`." and exit 1. (A matching `bridge-screen` on PATH is still used first, so a Windows user with the program installed and a POSIX shell works.)
- README install table: a Windows row saying "program yes (PowerShell installer, pipx, npx); plugin launcher not yet"; fix the shell-installer row's `<release-url>` to the real URL.
- `RELEASING.md`: a "Before the first tag" checklist: the five one-time setup items (tap repo, tap token, npm token, PyPI trusted publisher + `pypi` environment, crates token), `dist plan` output reviewed, `claude plugin validate ./plugin`, prerelease tag `v0.1.0-rc.1` first as a rehearsal (dist marks it a prerelease and skips brew/npm publish), then the real tag. **Owner decision recorded, not made here:** real Windows plugin support needs a Windows machine to test; left as a follow-up.
- [ ] **Step 1: Write the failing tests:** `launcher_on_windows_hook_exits_zero_and_logs` and `launcher_on_windows_run_explains_install` (fake `uname` on a test PATH returning `MINGW64_NT`); `launcher_still_uses_matching_binary_on_path_on_windows`.
- [ ] **Step 2: Run** `cargo test -p bridge-screen --test install launcher` — expect FAIL.
- [ ] **Step 3: Implement** the launcher case and the doc edits.
- [ ] **Step 4: Run** `cargo test --workspace && sh -n plugin/scripts/bridge-screen && dist plan` — expect PASS and a plan listing archives named `bridge-screen-<arch>-<os>.tar.xz` with `.sha256` files (this is the check that the launcher's expected names match; record the output in the commit message).
- [ ] **Step 5: Commit** `feat(release): Windows launcher message, first-release checklist`.

### Task 9: Docs and the owner's living pages

**Files:** Modify the design spec (§6.1 layout adds `lead/<repo-id>/`; §8.4 briefing pointer/offer; §10 the pages view; §11.2 tools table; §11.3 skill bundle), `docs/lead/decision-log.md`, `glossary.md`, `project-map.md`, `message-audit.md`, `current-state.md`, `CLAUDE.md` (layout list: `pages/`, `mcp/pages.rs`, the `pages` command, the bundled skill).

- [ ] **Step 1: Decision-log entries:** pages stored in bridge-screen's folder, not the repo (why: worktrees, no repo writes; cost: not shared between people); the offer repeats up to three sessions; the lead-briefing skill ships in the bundle; crates.io route and Windows stance. Never delete earlier entries.
- [ ] **Step 2: Glossary** "Project pages"; **project map** new part and where the pages live; **message audit** a row for each new owner-facing message (offer note, pointer line, tool refusals, dashboard sentences, CLI lines, launcher Windows message).
- [ ] **Step 3: Current state** rewritten with the date, what now works and what is not verified.
- [ ] **Step 4: Run** the jargon check and `cargo test --workspace` — expect PASS.
- [ ] **Step 5: Commit** `docs: project pages, lead bundle and release notes`.

---

## Self-review (spec coverage)

Offer once with owner's yes (Tasks 1–2, 4), decline (1, 3, 4), pages outside the repo and shared across worktrees (1, 3), agent tools incl. intent/decision-log rules (1, 3), pointer in briefing within 40 lines (2), agent fills current state (3 `set_up` next steps + 6 skill text), dashboard view (5), skill bundle (6), crates.io and Windows gaps (7, 8), docs and living pages (9). Spec success checks 1–7 map to tests in Tasks 2, 3, 2, 1, 2, 1 (`rebuild_leaves_pages_alone`), 6.

## Execution

Two agents, as the owner prefers (at most four in total for this work; one was already used for the design check): the first implements Tasks 1–9 in order in the worktree above; the second reviews the whole branch against the spec and Review Focus and fixes what it finds. The owner does the one-time release account steps (Task 8's checklist) and pushes the tag.
