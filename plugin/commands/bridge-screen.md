---
description: Show bridge-screen progress — needs-you items, project progress and live sessions
allowed-tools: mcp__plugin_bridge-screen_bridge-screen__show_progress, Bash(bridge-screen status), Bash(bridge-screen init:*)
---

Call the `show_progress` tool (bridge-screen's MCP server). In the Claude desktop app it
renders an interactive progress card; elsewhere it returns the text view. Show the text
view to the owner verbatim, inside a plain code block, with no commentary before or after
it. Don't summarise, reorder or reformat it.

If the `show_progress` tool isn't available, run `bridge-screen status` with the Bash tool
instead (or `"${CLAUDE_PLUGIN_ROOT}/scripts/bridge-screen" status` when `bridge-screen`
isn't on the PATH) and show its output the same way.

If the argument is `init` (`/bridge-screen init`), run `bridge-screen init` with the Bash
tool instead and follow the steps it prints: draft the plan, show it to the owner once,
then call `init_plan`.
