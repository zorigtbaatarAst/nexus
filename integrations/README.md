# Integrations

## Claude Code, as a plugin

```
/plugin marketplace add zorigtbaatarAst/nexus
/plugin install nexus@nexus
```

That brings the MCP server, seven slash commands and a skill that tells the agent when to
reach for Nexus unprompted. The manifest lives in [`.claude-plugin/`](../.claude-plugin/) at
the repository root, with the commands in [`commands/`](../commands/) and the skill in
[`skills/nexus/`](../skills/nexus/).

Update it with `/plugin marketplace update nexus`, which is separate from updating the
binary — the plugin carries prompts, the binary carries the intelligence.



One MCP server, one tool set, every MCP-capable agent. There is **no per-agent code** here
and none is planned — these are configuration snippets and prompt text.

The test of whether this layer is thin enough: deleting this directory must cost no
capability. Every tool stays callable; only the shorthand goes away.

| Agent | How |
|---|---|
| **Claude Code** | install the plugin — see below — or `claude mcp add --scope user nexus -- nexus mcp` |
| Codex | append [`codex/config.toml`](codex/config.toml) to `~/.codex/config.toml` |
| GitHub Copilot | copy [`copilot/mcp.json`](copilot/mcp.json) to `.vscode/mcp.json` |
| Anything else | spawn `nexus mcp` and speak MCP over stdio |

## What the agent gets

| Tool | Answers |
|---|---|
| `nexus_get_project_context` | what kind of project this is, and how far the baseline has drifted |
| `nexus_scan` | index it and set a baseline |
| `nexus_rescan` | what changed since the baseline, down to the symbol |
| `nexus_get_changes` | the changes recorded by the current baseline scan |
| `nexus_get_impact` | who breaks if this changes — **across the frontend/backend seam** |
| `nexus_get_known` | what is already known about this code: findings and facts |
| `nexus_record_finding` | contribute a finding you reasoned out — it gets the same identity and history a rule's does |
| `nexus_record_fact` | remember something for the next session |
| `bughunter_analyze` | run BugHunter's deterministic rules |
| `nexus_get_symbol` | one symbol's neighbourhood, following renames |
| `nexus_get_graph` | how much of the dependency graph exists — coverage, not accuracy |
| `nexus_doctor` | what is misconfigured, and the command that fixes it |

Nothing here runs tests, so no finding is verified by reproduction. The server says so in its
own instructions, so an agent is not left to infer it.

## Notes for whoever writes the prompts

- Call `nexus_get_project_context` first, and `nexus_get_known` before changing anything —
  the answer is what a previous session already worked out.
- Call `nexus_get_project_context` first. It is cheap and it says whether a baseline
  exists.
- `nexus_get_impact` returns the **edge chain** that produced each result and the weakest
  confidence along it. A result whose `min_confidence` is 0.5 went through a heuristic hop and
  should be treated as a lead, not a fact.
- Results are budgeted to roughly 8k tokens. When one is truncated it says so, with the true
  total and a concrete way to narrow — pass that back rather than guessing.
