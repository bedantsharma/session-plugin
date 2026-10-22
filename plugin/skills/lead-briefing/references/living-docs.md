# Living documents: templates and upkeep

Five one-page documents that let the lead re-orient without asking. Keep them in the project's docs folder (e.g. `docs/lead/`), in plain Markdown, or as an HTML page with a diagram if the lead prefers visuals. Agents keep them current; the lead reads them.

Two rules keep them useful:
- **Short beats complete.** One page each. A long document nobody reads is worse than none, because agents also rely on it.
- **Stale is dangerous.** A wrong map misleads the lead and every agent that reads it. Put only slow-changing facts in the map; put fast-changing facts in the current-state page, which is overwritten and dated.

## 1. Intent (owned by the lead)

Modelled on commander's intent: if the plan breaks, anyone reading this should still know what to aim for.

```
# Intent — <project name>
**Purpose:** <why this exists, for whom, in 2-3 sentences>
**Done looks like:** <the observable end state, e.g. "the whole team uses it daily; every order is tracked from checkout to delivery">
**Not doing:** <non-goals — things people might assume are in scope but aren't>
**Hard constraints:** <rules that must never be broken, e.g. "no secrets in code", "only employees can log in", "a human approves every merge">
**Current focus:** <the one thing that matters most right now, e.g. "first production launch">
Last confirmed by <lead>: <date>
```

Draft it by interviewing the lead (purpose, done, not doing, constraints). Never change it without them.

## 2. Project map

The whole system on one page, from the top. Follows the C4 model's top two levels (context, then parts) and the ARCHITECTURE.md idea (only things that rarely change).

```
# Project map — <project name>
## The whole thing in one paragraph
<what it does, who uses it, what outside systems it talks to>

## Diagram
<boxes and arrows: users, the parts, outside systems; label each arrow with what flows along it>

## The parts
For each part:
### <plain name> (also called `<internal name>`)
- **Does:** <one line>
- **Talks to:** <which parts / outside systems, and why>
- **Holds:** <data, secrets, or nothing>
- **Lives in:** <folder or repo — the only technical line>

## Where data and secrets live
<one short list>

## Deliberately absent
<things a reader might expect but which don't exist on purpose, e.g. "no per-user containers yet">
```

Update when a part is added, removed or changes job. Review monthly.

## 3. Current state

Today's status update, in the status-update shape from SKILL.md. Overwrite it; don't append. Put the date at the top. Link open problem cards.

```
# Current state — <date>
**Bottom line:** ... — <Green|Amber|Red>
**What works today:** ...
**In flight:** ...
**Open problems:** <problem-card headlines with state>
**Needs your decision:** ...
**Next:** ...
**How sure:** ...
```

## 4. Decision log

One entry per choice that someone will later ask "why?" about. Follows Architecture Decision Records. Never delete an entry: mark it "Superseded by #N".

```
## #<N> <decision, as a short statement> — <Proposed|Accepted|Superseded by #M> — <date>
**Context:** <what forced a choice, in plain words>
**Decision:** We will <...>.
**Consequences:** <what gets better, what gets worse or harder>
```

Examples of what deserves an entry: choosing a library, a security trade-off, a "we'll do it later" scope cut, a naming or structure change.

## 5. Glossary

Every term specific to the project (internal names, acronyms, borrowed jargon), one plain line each.

```
- **<term>** (also called <alias>): <what it is, in plain words>. See: <map section>.
```

Add a term the first time it appears in a report. If the lead asks "what is X?", X belongs here.
