# Integrations

One MCP server, one tool set, every MCP-capable agent. There is **no per-agent code** here
and none is planned — these are configuration snippets and prompt text.

The test of whether this layer is thin enough: deleting this directory must cost no
capability. Every tool stays callable; only the shorthand goes away.

| Agent | File | Where it goes |
|---|---|---|
| Claude Code | [`claude-code/.mcp.json`](claude-code/.mcp.json) | your project root |
| Codex | [`codex/config.toml`](codex/config.toml) | append to `~/.codex/config.toml` |
| GitHub Copilot | [`copilot/mcp.json`](copilot/mcp.json) | `.vscode/mcp.json` |
| Anything else | — | spawn `bughunter mcp` and speak MCP over stdio |

## What the agent gets

| Tool | Answers |
|---|---|
| `bughunter_get_project_context` | what kind of project this is, and how far the baseline has drifted |
| `bughunter_scan` | index it and set a baseline |
| `bughunter_rescan` | what changed since the baseline, down to the symbol |
| `bughunter_get_changes` | the changes recorded by the current baseline scan |
| `bughunter_get_impact` | who breaks if this changes — **across the frontend/backend seam** |
| `bughunter_get_symbol` | one symbol's neighbourhood, following renames |
| `bughunter_get_graph` | how much of the dependency graph resolved, so you know what to trust |
| `bughunter_doctor` | what is misconfigured, and the command that fixes it |

Nothing here finds bugs or runs tests yet. The server says so in its own instructions, so an
agent is not left to infer it.

## Notes for whoever writes the prompts

- Call `bughunter_get_project_context` first. It is cheap and it says whether a baseline
  exists.
- `bughunter_get_impact` returns the **edge chain** that produced each result and the weakest
  confidence along it. A result whose `min_confidence` is 0.5 went through a heuristic hop and
  should be treated as a lead, not a fact.
- Results are budgeted to roughly 8k tokens. When one is truncated it says so, with the true
  total and a concrete way to narrow — pass that back rather than guessing.
