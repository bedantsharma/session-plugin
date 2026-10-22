---
name: lead-briefing
description: How to report to the person who leads a project but does not read the code — status updates, blockers, findings, reviews, checklists, handoffs, "what's going on?" answers — in a shape they understand on first read (bottom line first, why it matters, the idea before the detail, cause, state, the ask, confidence, details last). Also sets up and maintains the project's living docs (intent, project map, current state, decision log, glossary) and runs catch-up sessions and backbriefs where the lead explains the system back. Use this whenever you report progress or problems to the user, write a checklist or plan for them to act on, summarise what agents or subagents did, answer "where are we?", "what's the status?", "what do I need to do?", "explain this to me", or when the user says they've lost track of their project, don't understand an update, or wants to be trained to follow it — even if they never mention reporting or briefing.
---

# Lead briefing

The reader is the project's lead. They decide what gets built and what ships, but they don't write or read the code, and work is moving faster than they can follow. Accurate text full of file names and config keys fails them: they can read every word and still not know what the problem is. Your job is to give them the picture, not the evidence. The evidence goes at the bottom, for whoever does the work.

This shape comes from fields that brief decision-makers under pressure: military "bottom line up front" and commander's intent, hospital handoffs (SBAR, I-PASS), intelligence writing standards (facts kept apart from judgment, confidence stated), decision records, and plain-language rules. They all do the same few things. See `references/why-it-works.md` if you need the sources.

## The rules

1. **Lead with the effect, in plain words.** The first sentence says what is true or wrong in terms the lead cares about: users, security, money, time, going live. "Anyone who logs in could run commands on our server", not "the middleware has no route allowlist". No file names, config keys or code terms in that sentence.
2. **Say why it matters, and whether it blocks anything.** What happens if nobody acts?
3. **Explain the idea before the detail.** In 2-4 sentences, give the concept a newcomer needs. Assume they know what a server, a login, a database and a deploy are, but not this project's internal names. Define any project term or jargon the first time it appears ("the gateway — the front door that handles login").
4. **Give the cause.** Why did this happen? Name the systemic cause if there is one (we copied a tool built for something else, nobody owned X). No blame.
5. **State where it stands now.** Use one of: Open / Being fixed / Fix ready, waiting for merge / Merged, not deployed / Live and checked. Say what is still exposed meanwhile.
6. **Ask for something specific, or say you don't need anything.** A decision comes with options, your recommendation and the cost of waiting.
7. **Keep facts apart from judgment.** Mark what you *checked by running it* separately from what you *believe from reading code or docs*, and what you relayed from another agent without checking. This distinction is more useful to the lead than "high/medium/low confidence", because model confidence is not well calibrated.
8. **Details go last, under a "Details" heading.** File paths, line numbers, env vars, commands and merge request numbers belong there and nowhere above it.
9. **Never round up to green.** If you know of a risk, or haven't verified something, say so. "Done" means checked, not written.
10. **Keep it short.** Aim for under 300 words above "Details" unless the lead asks for depth. If several items need reporting, give a one-line overview first, then the items.

Before sending, re-read your first paragraph as if you had never seen the code. If it only makes sense to someone who has, rewrite it.

## Shapes

Use the same shape every time, so the lead always knows where to look.

### Problem card (blocker, bug, risk, finding)

```
**Headline:** <one plain sentence: the effect>
**Why it matters:** <consequence if nobody acts; blocks go-live? yes/no>
**What's going on:** <the idea a newcomer needs, 2-4 sentences, terms defined>
**Why it happened:** <cause, systemic cause if any>
**State:** <Open | Being fixed | Fix ready, waiting for merge | Merged, not deployed | Live and checked> — <what is exposed meanwhile>
**What I need from you:** <decision with options + recommendation, or an action, or "Nothing">
**How sure:** <checked by running X | believed from reading Y | relayed, not checked>
**Details:** <files, config, links — for whoever does the work>
```

Example:

> **Headline:** Anyone who can log into our admin dashboard could run commands on our server and read the file that holds our passwords and keys.
> **Why it matters:** One curious employee, or one stolen login, would be enough to take over the server. This blocks going live.
> **What's going on:** The dashboard started as a copy of an open-source app built for one person on their own laptop. It came with a built-in terminal and a file editor. On a laptop only the owner can use them; on a shared server, everyone who gets past the login can. Our login checks who you are, not what you may do afterwards.
> **Why it happened:** We built our screens on top of the general tool and never listed what else it could do.
> **State:** Fix ready, waiting for merge. Until it is deployed, any running dashboard is exposed.
> **What I need from you:** Merge the fix. After deploy I'll confirm the terminal and editor can't be reached.
> **How sure:** Checked by calling the terminal endpoint as a normal user on a local build (blocked). Not yet checked on the real server.
> **Details:** route allowlist in `dashboard/middleware.ts`, `ENABLE_HOST_TOOLS` flag, MR !42.

### Status update

```
**Bottom line:** <one sentence> — <Green: on track | Amber: at risk, being handled | Red: needs you or outside help>
**What changed:** <3-5 items, each: what + its effect>
**Blocked:** <what is stuck, on what — or "Nothing">
**Needs your decision:** <each: options, recommendation, by when — or "None">
**Next:** <what happens before the next update>
**How sure:** <what was checked by running, what wasn't>
**Details:** <links, MRs, files>
```

### Checklist or plan the lead must act on

Checklists are where this goes wrong most often: each item becomes a dense line of config. Give each item a plain action, why it matters, and how the lead will know it's done. Put the technical how-to under Details. Order the items by "blocks going live" first. Group them under plain headings ("Passwords and keys", "Who can log in"), not by component name.

```
- [ ] **<plain action>** — <why: what goes wrong without it>. Done when: <observable result>.
  Details: <commands, settings, files>
```

### Answering "what is X?" or "explain this"

Start with what it's for and where it sits in the whole (one sentence each), then how it works in plain steps, then what can go wrong. Use one concrete example. Offer a diagram for anything with more than three moving parts.

## Living documents

Keep the lead's picture current without them having to ask. Each is one page. Agents maintain them; the lead reads them. Templates and upkeep rules are in `references/living-docs.md`. Read it when setting these up or when the project has none.

| Document | What it holds | Changes |
|---|---|---|
| Intent | Purpose, what "done" looks like, non-goals, hard constraints | Rarely; the lead owns it |
| Project map | Each part in plain words, how they connect, where data and secrets live, what is deliberately absent | When parts are added or removed |
| Current state | Today's status update, dated, overwritten (not appended) | After each merge or working day |
| Decision log | One paragraph per "why did we do it this way?" choice | Whenever such a choice is made |
| Glossary | Every project term in one plain line, plus "also called" | Whenever a new term appears |

When you make a choice someone will later ask "why?" about, add a decision-log entry. When you introduce a project term, add it to the glossary. When you report something that changes the picture, update the current-state page.

## Catch-up and backbriefs

Reading alone doesn't rebuild understanding. Saying it back does: hospitals and the military both make the receiver explain the plan back, and that is the step that catches gaps. When the lead has lost track, or asks to be trained, run this:

1. **Intent.** Confirm or draft the intent page with them (ask what the product is for, what done means, what's out of scope).
2. **Map.** Show the project map at the top level first (the whole system, its users, the outside systems it talks to), then one level down (its parts). Visual first.
3. **Backbrief.** Ask the lead to explain the system back in their own words, for example how a request becomes a finished result. Correct what's wrong, name what's missing, and ask them to try again on the corrected part. Repeat until there's nothing to correct. Be specific and encouraging; they're rebuilding a model, not taking a test.
4. **Walkthroughs.** Take one part at a time, riskiest first. Explain it plainly with real snippets from the code, or an interactive explainer for anything mechanical. A fresh agent that didn't write the code gives a more honest walkthrough.
5. **State and decisions.** Read the current-state page, then the decision-log titles; open only the decisions that surprise the lead.
6. **Weekly upkeep.** A status update, a five-minute backbrief on whatever changed most, and "what did an agent decide this week that you'd want to know about?"

When the lead explains something back, the check runs both ways: if your own explanation can't survive their questions, it wasn't grounded. Point to evidence.

## Working with subagents

Subagents don't see this skill unless they load it. When you delegate work whose result goes to the lead, put the core rules in the subagent's prompt: the effect first, the problem-card shape, checked vs believed, details last. Then verify what the subagent reports before you relay it. An unchecked agent claim is the most common way a lead gets told something false.
