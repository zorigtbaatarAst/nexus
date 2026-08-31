# BugHunter — MCP Server Architecture

MCP is BugHunter's primary integration surface. One server, one tool set, every MCP-capable
agent. There is no Claude-specific code path, no Codex-specific code path, and no plan to
add either. See [ADR-004](architecture-decisions.md#adr-004-mcp-as-the-primary-integration-surface).

```
bughunter mcp        # stdio JSON-RPC — the only transport in V1
```

---

## 1. The thinness rule

Every handler is: **deserialize → one `Engine` call → serialize.**

```rust
async fn bughunter_get_impact(&self, p: ImpactParams) -> Result<CallToolResult> {
    let report = self.engine.impact(p.into_query())?;     // the only line that does work
    Ok(budgeted_json(report, p.max_items, p.cursor))
}
```

No branching on results, no assembling data from two calls, no business rules. If a handler
needs a second `Engine` call to do its job, the missing method belongs in `bh-core`, because
the CLI needs it too.

This is enforced structurally: **`bh-mcp` does not depend on `bh-store`, `bh-lang*` or
`bh-verify`** — a `cargo metadata` test fails the build otherwise. A handler physically
cannot reach the database, so it cannot grow logic the CLI lacks. That is the mechanical
form of the brief's "do not implement important functionality only inside MCP handlers".

---

## 2. Tool surface

| Tool | Class | `Engine` method |
|---|---|---|
| `bughunter_init` | write | `init` |
| `bughunter_scan` | write | `scan` |
| `bughunter_rescan` | write | `rescan` |
| `bughunter_scan_status` | read | `scan_status` |
| `bughunter_get_project_context` | read | `project_context` |
| `bughunter_get_symbol` | read | `symbol` |
| `bughunter_get_changes` | read | `changes` |
| `bughunter_get_impact` | read | `impact` |
| `bughunter_get_tests_for` | read | `tests_for` |
| `bughunter_find_bugs` | read+ai | `find_bugs` |
| `bughunter_record_bug` | write | `record_bug` |
| `bughunter_get_bug` | read | `bug` |
| `bughunter_verify_bug` | **execute** | `verify_bug` |
| `bughunter_get_bug_history` | read | `bug_history` |
| `bughunter_get_regressions` | read | `regressions` |
| `bughunter_record_fact` | write | `record_fact` |

The eleven tools named in the brief, plus five: `get_symbol` and `get_tests_for` (agents ask
for these constantly and would otherwise re-derive them from `get_impact`), `scan_status`
(long operations, §4), `record_bug` and `record_fact` (the agent-as-provider write-back path
— without them an agent's reasoning evaporates when the session ends).

### Resources

For pull-style clients that prefer resources over tool calls:

```
bughunter://project/context        the project profile + top facts
bughunter://scan/latest            the most recent scan report
bughunter://bugs/open              non-FIXED, non-IGNORED bugs
bughunter://bug/{uid}              one bug with its full history
```

Resources are read-only views over the same `Engine` methods. No tool has a resource-only
capability, and no resource has a tool-only one.

---

## 3. Response budgeting

An agent's context is the scarcest resource in the system. A tool that returns 2 MB of JSON
has not helped it; it has destroyed its ability to think about anything else.

Every list-returning tool takes and returns:

```jsonc
// request
{ "max_items": 50, "cursor": "eyJvIjo1MH0", "detail": "summary" }   // summary | full

// response
{ "items": [ ... ],
  "truncated": true,
  "total": 412,
  "next_cursor": "eyJvIjoxMDB9",
  "note": "412 affected symbols; showing the 50 highest-impact. Narrow with `min_score`." }
```

Rules:

- **Default budget ≈ 8 000 tokens per response**, enforced by serializing and measuring, not
  by guessing from item counts.
- **`truncated` is never silent.** It carries the true total and a concrete suggestion for
  narrowing. An agent that does not know it got a partial answer will confidently reason
  from it.
- **`detail: "summary"` is the default.** Full source excerpts are opt-in, per item, via
  `bughunter_get_symbol`.
- Source excerpts are capped (default 60 lines) and always carry `file:line` so the agent
  can request more precisely rather than being handed a file.

---

## 4. Long operations

`scan` on a monorepo takes minutes. Blocking an MCP client for that long is an outage from
the user's point of view.

```
bughunter_scan        → { "scan_id": "scan-014", "status": "running", "poll_after_ms": 2000 }
bughunter_scan_status → { "scan_id": "scan-014", "status": "running",
                          "phase": "parse", "progress": {"done": 8120, "total": 42311} }
                      → { "scan_id": "scan-014", "status": "ok", "report": { ... } }
```

The scan runs in a background thread within the MCP process; `scan_status` reads its
progress. Small projects finish inside the initial call and return `status: "ok"` directly,
so the extra round-trip only happens when it is actually needed.

MCP progress notifications are emitted alongside polling for clients that support them.

---

## 5. Permission gating

Tools are classed `read`, `write`, `execute` and `ai`. `execute` — today only
`bughunter_verify_bug` — consults `policy.toml` before doing anything.

When policy forbids it, the tool **returns a structured refusal**, it does not error and it
does not proceed:

```json
{
  "status": "permission_required",
  "action": "execute_tests",
  "reason": "policy.execute = \"none\"",
  "requested_command": ["./gradlew", "test", "--tests", "*BugHunter_BUG104*"],
  "sandbox": "docker",
  "to_allow": "set execute = \"docker\" in .bughunter/policy.toml",
  "test_written_to": ".bughunter/generated-tests/BUG-104/"
}
```

The agent can then ask its human in plain language, with the exact command and the exact
config change to hand. The brief's rule — "the MCP server should never silently execute
dangerous commands" — is satisfied on both halves: never silently, and never without a
policy that a human committed to the repository.

Every `execute` and `ai` tool call writes an `audit_events` row naming the MCP client as
actor (`mcp:claude-code`), whether or not it was permitted.

---

## 6. Errors

Domain failures are results, not protocol errors. An agent can act on a result; a JSON-RPC
error just makes it retry.

```json
{ "status": "error",
  "kind": "no_baseline",
  "message": "No baseline for this project. Run bughunter_scan first.",
  "recoverable": true,
  "next": ["bughunter_scan"] }
```

Kinds: `no_project`, `no_baseline`, `scan_in_progress`, `unknown_symbol`, `unknown_bug`,
`permission_required`, `unsupported_language`, `build_failed`, `sandbox_unavailable`,
`schema_too_new`. Protocol-level JSON-RPC errors are reserved for malformed requests.

---

## 7. Client configuration

The same server, four ways of pointing at it.

**Claude Code** — `.mcp.json` in the project root:
```json
{ "mcpServers": { "bughunter": { "command": "bughunter", "args": ["mcp"] } } }
```

**Codex** — `~/.codex/config.toml`:
```toml
[mcp_servers.bughunter]
command = "bughunter"
args    = ["mcp"]
```

**GitHub Copilot** — `.vscode/mcp.json`:
```json
{ "servers": { "bughunter": { "command": "bughunter", "args": ["mcp"] } } }
```

**Anything else** — spawn `bughunter mcp` and speak MCP over stdio.

No per-client code exists in the repository. `integrations/*/` contains configuration
snippets and prompt text only. If a new agent supports MCP, it is already supported.

---

## 8. A typical agent session

```
agent → bughunter_get_project_context          "what am I looking at"
      ← spring-boot 3.5 · gradle · mongo · 42k symbols · baseline a81f92c

agent → bughunter_rescan                       "what changed"
      ← scan-014 · 4 files · 17 symbols · 2 dependency changes

agent → bughunter_get_impact { since: "scan-013" }
      ← 11 affected symbols with paths · 8 related tests · 1 truncated node

agent → bughunter_get_symbol { fqn: "mn.pay.PaymentService#createPayment", detail: "full" }
      ← signature, annotations, 40-line body, callers, prior bugs in this component

  ... the agent reasons — this is the part BugHunter does not do ...

agent → bughunter_record_bug { fingerprint_input, hypothesis, evidence: [ {file,line}, … ] }
      ← BUG-104 · SUSPECTED → UNVERIFIED (evidence accepted) · confidence 0.71

agent → bughunter_verify_bug { id: "BUG-104" }
      ← reproduced · regression · VERIFIED · confidence 0.97 · introduced a81f92c
```

Note what never happens: no file is uploaded, no repository is traversed by the agent, and
no model is called by BugHunter. The agent brought the reasoning; BugHunter brought the
evidence, the history and the proof.
