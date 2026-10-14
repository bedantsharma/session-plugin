# Dashboard layout, type and liveness rework — Implementation Plan

> **For agentic workers:** One implementer does all tasks in order, then a reviewer checks and fixes. This plan is a design spec, not code: sizes, tokens, anatomy and behaviour. Follow existing patterns in `web/src` (Svelte 5 runes, Tailwind 4 `@theme` tokens in `app.css`, shadcn-svelte components, LayerChart).

**Goal:** The owner said the dashboard is "way too close together", the font is "very small", "the shapes and sizes and proportions are not" right, and "nothing blinks or glows… it doesn't feel alive". The colours are good and stay. Rework type, spacing, layout, component proportions and motion in the browser dashboard and the desktop card, per the research spec below.

**Branch:** `feat/dashboard-layout` in the worktree `/Users/bedantsharma/PycharmProjects/session-plugin-ui` (from `main` at the SP6 merge). Work only there.

**Scope:**
- **In:** `web/` (dashboard and card). Text wording in `status`, `watch` and MCP text output **only** for the hill rename and the human unit labels, so all screens match.
- **Out:** data, model and API changes; colour tokens (other than adding tint mixes of existing tokens); needs-you rules.
- **Known data bugs, not in scope:** the stale session branch, and milestone status not rolling up.

**Source:** the research report (owner-requested design research pass 2, 2026-10-14). Its principles and sources are summarised in §1; §2–§5 are the spec to implement.

## Global constraints

- Keep every colour token in `app.css` and the contrast tests (`web/tests/contrast.test.ts`). New tints are `color-mix(in oklab, var(--token) N%, transparent | var(--surface))` of existing tokens; add each new text-on-tint pair to the contrast test.
- Colour is never the only signal: every chip and state carries text plus an icon shape.
- Needs-you rules are unchanged:
  - only items with an owner response;
  - rank, then oldest;
  - buttons from `actions`;
  - no optimistic removal;
  - no reorder while the pointer or focus is inside;
  - a 4-item cap with "show all".
- Motion rules (§5):
  - meaning only;
  - every loop stoppable by the header "Motion" toggle and by `prefers-reduced-motion`;
  - animate only transform, opacity and background colour, except the bar width (row 7).
- Tests:
  - update vitest tests and the Playwright smoke/layout tests deliberately;
  - the old "no vertical scroll at 1280×720" layout test is **replaced**: the glance may scroll; the header and needs-you queue must be fully visible above the fold at 1280×720 with 4 queue items;
  - the motion test changes from "nothing longer than 5 s" to "every looping animation is disabled under `[data-motion=off]` and under reduced motion".
- Commits: small and logical, each ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`. Add dependencies only via `npm install`.
- Checks that must pass:
  - `web/`: `npm run lint`, `npm run check`, `npm run format:check`, `npm run build`, `npm run test`, `npm run e2e`;
  - Rust: `cargo test --workspace` and clippy, since `status`/`watch` text changes.

## 1. Principles (with sources)

1. **Body text on a productive UI is 14 px.** 12/16 is for labels and captions only, not body copy ([Carbon type sets](https://carbondesignsystem.com/elements/typography/type-sets/)).
2. **Web body text is 15–25 px.** 15 px row text is the floor ([Practical Typography](https://practicaltypography.com/point-size.html)).
3. **Spacing comes from one scale:** 2/4/8/12/16/24/32/40/48/64. No off-scale values ([Carbon spacing](https://carbondesignsystem.com/elements/spacing/overview/)).
4. **Row height is a density setting.** Carbon uses 32/40/48 for one line and 64 only for two lines. Right-align numeric columns ([Carbon data table](https://carbondesignsystem.com/components/data-table/usage/), [Primer DataTable](https://primer.style/components/data-table/)).
5. **No cramped containers that force wrapping or truncation;** use fixed column widths ([Carbon data table](https://carbondesignsystem.com/components/data-table/usage/)).
6. **Typography details:** don't use colour alone; line length ≈ 80 characters; line heights on a 4 px grid ([Primer typography](https://primer.style/foundations/typography/)).
7. **Motion:** it communicates and is optional. Carbon duration tokens are 70/110/150/240/400 ms, always with a static alternative ([Carbon motion](https://carbondesignsystem.com/elements/motion/overview/), [Apple HIG motion](https://developer.apple.com/design/human-interface-guidelines/motion)).
8. **Reduced motion means "reduce", not "remove":** opacity and colour fades stay ([MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/@media/prefers-reduced-motion)).
9. **Every status has a text label** (the Vercel Geist StatusDot pattern, [Geist](https://vercel.com/geist/status-dot)).
10. **WCAG 2.2.2:** looping or auto-updating content beyond 5 s needs a pause/stop control or must be essential. The header Motion toggle is that control ([W3C 2.2.2](https://www.w3.org/WAI/WCAG22/Understanding/pause-stop-hide.html)).

## 2. Spec

### Type scale

`html` 16 px; system UI font stack (unchanged). Add the tokens to `@theme`.

| Role | Size / line height | Weight | Class / token |
|---|---|---|---|
| Page title | 20/28 | 600 | `text-xl font-semibold` |
| Section heading | 16/24 | 600 | `text-base font-semibold` |
| Row primary | 15/22 | 500 for titles, else 400 | `--text-row: 0.9375rem`, `--text-row--line-height: 1.375rem` (`text-row`) |
| Metadata | 13/20 | 400 | `--text-meta: 0.8125rem`, `--text-meta--line-height: 1.25rem` (`text-meta text-muted`) |
| Numbers | as the row text | 500 | `tabular-nums` |
| Big number (done %) | 28/32 | 600 | `text-[1.75rem] leading-8` |
| Mono ids | 13/20 | 400 | `font-mono text-meta` |
| Button | 14/20 | 500 | `text-sm font-medium` |
| Chip | 12/16 | 500 | `--text-chip: 0.75rem`, `--text-chip--line-height: 1rem` (`text-chip`) |

- Nothing goes below 12 px, and only chips are 12 px.
- No weight 300.
- Keep `antialiased`.
- Replace every `text-xs` used for non-chip text.

### Spacing (4 px base)

| Element | Value |
|---|---|
| Page padding | 32 px at ≥1280, 24 px at ≥1024, 16 px below |
| Header | 64 px tall, 1 px `--line` bottom border |
| Gap between sections | 32 px |
| Heading to content | 12 px |
| Card padding | 20 px (16 px in the desktop card) |
| Card radius | 12 px |
| Inner row radius | 8 px |
| Gap between cards | 16 px |
| List row minimum height | 44 px |
| Table row height | 36 px |
| Gaps inside a row | 12 px |
| Gap from primary line to metadata line | 2–4 px |

Elevation:
- The page is `--bg`.
- Cards are `--surface` with a 1 px `--line` border and no shadow.
- Row hover is `color-mix(in oklab, var(--fg) 4%, var(--surface))`.

### Layout

- **Container:** `mx-auto w-full max-w-[1280px]` with the page padding.
- **≥1280 px:**
  - the needs-you queue is full width at the top, only when it has items;
  - below it, a grid `grid-cols-[minmax(0,1fr)_400px]` with a 32 px gap;
  - left: project cards, 16 px apart;
  - right: one Sessions card.
- **1024–1279 px:** one column — needs-you, projects, sessions.
- **<1024 px:** one column — needs-you, sessions, projects; tables collapse to a primary line plus a metadata line.
- Cards for projects, sessions and needs-you items. Flat sections (heading plus divider) for drill-down tables and trees.

### Components

**Header**
- 64 px tall.
- Left: the clay ✱, then "bridge-screen" (20/28), then "1 session · 1 project" (13/20 muted).
- Right: the Live indicator (§5, row 3), then the Motion toggle (32 px icon button, `aria-pressed`, tooltip "Motion on/off"), then the count pill.
- **Count pill:** 32 px tall, `px-3.5`, fully rounded, 1.5 px border in the status token, 14/20 medium.
  - "● All clear" uses `--ok`.
  - "■ N need you" uses `--bad` on a 10% tint.

**Needs-you item card**
- 3 px `--bad` left border, 20 px padding.
- Row 1:
  - the kind chip, e.g. "■ DRIFT": `h-6`, 12/16 uppercase, `tracking-wide`, `--bad` on a 10% tint;
  - the title at 15/22 weight 500, which opens the session;
  - the age at 13/20 muted, pushed right.
- Row 2: the detail at 13/20 muted, at most 2 lines.
- File lists use mono 13/20 with `line-clamp-2`.
- Buttons go in a bottom row, 8 px gaps, wrapping.
- The optional Continue reason input is `h-9 w-64 text-sm`.
- Status and reply text are 13/20.

**Buttons**
- `h-9 px-4 text-sm font-medium rounded-lg` (36 px).
- Compact: `h-8 px-3`, used in the card.
- Minimum hit target 32 px.
- Existing tones.

**Chips**
- `h-6 px-2.5 rounded-full text-chip font-medium`.
- 1 px border in the tone token, text in the tone token, fill at 10% tint.
- Always an icon or text, never colour alone.

**Project card**
- **Top row:**
  - the project name at 16/24 weight 600, as a link;
  - the goal title at 13/20 muted, truncating;
  - on the right, the done % at 28/32 plus "+N% claimed" at 13/20 muted;
  - the whole card is clickable.
- **Progress bar:**
  - `h-2.5`, fully rounded track in `--line`, fill in `--clay`;
  - the claimed segment is clay at 40% alpha (it must still pass the 3:1 test against the track — adjust if needed);
  - a caption above the bar: "Done N% · M of K milestones complete" at 13/20 muted.
- **Milestone list:**
  - starts 12 px below the bar; one row per milestone;
  - each row is 36 px, a grid `grid-cols-[2.5rem_minmax(0,1fr)_7rem_3rem_auto]`:
    1. the id in mono 13 muted;
    2. the title at 15/22, truncating;
    3. a mini bar `h-1.5`;
    4. the % right-aligned, `tabular-nums` 13/20;
    5. chips (hill, stuck).
  - Show 6 rows, then a "show all N" text button.
- **Footer:** leftovers and unestimated counts at 13/20 muted, above a 1 px rule with `pt-3`.

**Hill chip — rename (owner-facing wording; one word change if the owner prefers other words)**
- Replace "uphill" / "downhill" everywhere — dashboard, card, `status`, `watch`, MCP text — with:
  - **"exploring"**, `--warn`, icon `?`, tooltip "Unknowns remain — this % may still change";
  - **"executing"**, `--ok`, icon `→`, tooltip "Path is clear — this % is reliable".
- Drop the ▲/▼ arrows.
- Hide the chip on milestones at 100%.
- Keep the muted-% treatment for exploring milestones; it is now explained by the tooltip.
- The model and API keep the values `uphill`/`downhill`; this is display wording only.

**Session row (glance Sessions card)**
- Minimum height 52 px.
- Line 1: the status chip with its word ("● Working" `--ok`, "◆ Idle" `--warn`, "✓ Done" `--ok`, "■ Needs you" `--bad`), the mono short id, then the title at 15/22, truncating.
- Line 2: the branch in mono 13 muted.
- Right side: the active time in `tabular-nums` 13/20, e.g. "185 min".

**Session drill**
- **Header block:** the mono id at 20/28, the status chip, the branch, the active time.
- **"Time by label":** a captioned, stacked 3-segment bar (`h-2.5`) with a legend under it (12 px dot plus "On task 186 min" in 13/20).
- **Timeline table:**
  - fixed columns `grid-cols-[3rem_4.5rem_4.5rem_11rem_4rem_minmax(0,1fr)]`:
    1. `#` in mono 13 muted;
    2. time, 13 muted, tabular;
    3. duration, e.g. "6 min", "<1 min", right-aligned;
    4. the label chip;
    5. the task id;
    6. files/score, e.g. "1 file", "4 files", "no files", score "0.40".
  - Rows are 36 px with 1 px dividers.
  - Sticky header row, 36 px, 12/16 uppercase muted.
  - Labels never wrap (`whitespace-nowrap`).
- **Human labels** (dashboard, card and `watch`):

  | Raw | Display |
  |---|---|
  | on_task (agent) | "✓ On task · agent" |
  | on_task (heuristic) | "✓ On task · auto" |
  | on_task (owner) | "✓ On task · owner" |
  | detour | "↷ Detour · …" (with the same source suffix) |
  | drift | "▲ Drift · …" (with the same source suffix) |
  | none | "Unlabelled" |

  Tones: on task `--ok`, detour `--warn`, drift `--bad`, unlabelled grey.

**Plan tree (project drill)**
- 40 px rows.
- 20 px indent per level with a 1 px `--line` guide.
- A `size-6` chevron, the status icon, and the title at 15/22 (weight 500 for parents, 400 for leaves).
- On the right: a mini bar (`h-1.5 w-24`) and the %.

**Task, History and Tuning drills**
- **Task:** title at 20/28; details as a definition list (13/20 muted labels, 15/22 values).
- **History:** a 36 px table with a 13 px tabular timestamp, event and detail; grouped by day under 16/24 headings with 24 px above each group.
- **Tuning:** a single card with 20 px padding; 44 px label/value rows with a 13/20 muted description; numbers right-aligned and tabular; `h-9` buttons.

### Desktop card (600–700 px wide)

The same scale, one step down:
- **Text:** rows 14/20; metadata 12/16 (the only place 12 px metadata is allowed).
- **Spacing and controls:** row height 40 px; card padding 16 px; `h-8` buttons; section gap 12 px, inner gap 8 px.
- **Layout:** single column — needs-you, then one compact line per project (name, `h-2` bar, %), then sessions. Milestones go behind "show milestones".

## 3. Screen changes (summary)

- **Glance:**
  - the 64 px header with Live indicator, Motion toggle and a larger pill;
  - the 1280 container with the two-column grid;
  - project cards with milestone rows and the captioned bar;
  - the Sessions card with status chips;
  - rows 44–52 px.
- **Project:** a 20/28 heading, the goal and the big %; a milestone card; a 40 px plan tree; History and Tuning as 36 px buttons.
- **Session:** the header block, the time-by-label bar with legend, and the timeline table with human labels. It may scroll; the table header is sticky.
- **Task, History, Tuning:** as specified above.

## 4. Keep

- Colour tokens and contrast tests.
- Icons plus text on every state.
- Reduced-motion behaviour (extended below).
- The needs-you rules.
- The "Disconnected" state, which also replaces the Live indicator with a static `--warn` "Disconnected" chip.

## 5. Liveness (motion spec)

### Gating

- Loops run only under `@media (prefers-reduced-motion: no-preference)` **and** `:root:not([data-motion='off'])`.
- The header Motion toggle sets `data-motion` on `<html>` and persists it in `localStorage` (wrapped in try/catch).
- The default is on, unless reduced motion is set, in which case it is off.
- With motion off, the 1 s timers tick once a minute.
- Data polling is unaffected.

### Animated elements

| # | Element | Trigger | Animates | Duration / easing | Range | Loop | Stopped by |
|---|---|---|---|---|---|---|---|
| 1 | Working-session dot (8 px) with an expanding ring | Session working | Pseudo-element scale + opacity | 2 s, `cubic-bezier(0,0,.2,1)` | Scale 1→2.4, opacity 0.5→0, in `--ok` at 45% | Yes | Toggle or reduced motion → static 2 px ring at 30% |
| 2 | Working-session row | Session working | Static: 2 px `--ok` left bar + 6% tint | — | — | No | — |
| 3 | Header "● Live" dot | Connected | Opacity | 2.4 s ease-in-out | 1→0.45 | Yes | Toggle or reduced motion → solid dot |
| 4 | Heartbeat on the Live dot | A poll returned changed data | Scale | 300 ms, `cubic-bezier(.2,0,0,1)` | 1→1.5→1 | Once | Reduced motion → opacity flash |
| 5 | Row changed flash | A row's values changed after first load | Background, `--clay` 14% → transparent | 1.2 s ease-out | — | Once | Toggle → none; reduced motion → 600 ms |
| 6 | Number change | A displayed number changed | Opacity 0.4→1, translateY 4 px→0 | 150 ms, `cubic-bezier(.2,0,0,1)` | — | Once | Reduced motion → no translate |
| 7 | Progress bar width | Value changed | Width (or scaleX) | 600 ms, `cubic-bezier(.2,0,0,1)` | — | Once | Toggle or reduced motion → instant |
| 8 | Sheen on an active milestone bar | A working session is on that milestone | Gradient translateX, white ≤12% | 2.4 s linear sweep, then a pause | — | Yes | Toggle or reduced motion → none |
| 9 | New needs-you item | Item appears | Opacity 0→1, translateY 4→0, plus a one-time `--bad` ring 35%→0 | 240 ms; ring 1.2 s ease-out | — | Once | Reduced motion → fade only |
| 10 | Count pill | Count increased | Scale | 200 ms | 1→1.08→1 | Once | Reduced motion → none |
| 11 | Elapsed timers ("working 42 min", "updated 3 s ago") | A live view is open | Text | 1 s ticks (per minute when off) | — | Continuous | The toggle slows it |
| 12 | Pending action spinner (16 px) | A button is waiting | Rotate | 800 ms linear | — | While pending | Reduced motion → static "…" |
| 13 | Hover and press | Pointer | Colours | 110–150 ms | — | No | Kept under reduced motion |

Rules:
- Rows 5 and 6 skip the initial load: compare against the previous value, keyed by row id. Use a Svelte `flash` action that adds a class on change and removes it on `animationend`, and `{#key}` for number changes.
- Needs-you items never loop. Their urgency is the red border, the ■ icon and the pill.
- Idle and done sessions are static.

### Desktop card

- Keep: row 1 as opacity-only (1→0.5, 2.4 s, no scale) and row 5 at 600 ms.
- Drop: rows 4, 6, 7 (instant), 8 and 10.
- It honours reduced motion and `data-motion`, with a small Motion icon button in the card header.

## Tasks

1. **Tokens and type scale:** the `@theme` tokens, spacing, elevation and tints; contrast-test additions; replace `text-xs` body uses. Commit `web: type scale, spacing and surface tokens`.
2. **Glance:** header (Live, Motion toggle, pill), layout grid, project card with milestone rows and the hill rename, Sessions card. Commit `web: glance layout, project cards and session list`.
3. **Drills:** session (header, time bar, timeline table, human labels), project (plan tree), task, history and tuning. Commit `web: drill-down layouts and readable labels`.
4. **Motion:** gating and toggle, rows 1–13, the flash action, tests. Commit `web: live motion with a pause control`.
5. **Desktop card** at the card scale and motion. Commit `web: card layout and motion`.
6. **Matching wording elsewhere:** hill words and human unit labels in `status`, `watch` and MCP text; update Rust tests deliberately. Commit `cli: exploring/executing and readable unit labels`.
7. **Tests and screenshots:**
   - update the vitest and Playwright tests per the global constraints;
   - add a Playwright screenshot step that saves glance, project and session screenshots at 1440×900 to `web/test-results/screens/` for the owner to review (not committed);
   - commit `test: layout, motion and screenshot checks`.
