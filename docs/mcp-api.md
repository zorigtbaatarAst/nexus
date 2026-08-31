# BugHunter — MCP Server Architecture

MCP is BugHunter's primary integration surface. One server, one tool set, every MCP-capable
agent. There is no Claude-specific code path, no Codex-specific code path, and no plan to
add either. See [ADR-004](architecture-decisions.md#adr-004-mcp-as-the-primary-integration-surface).

```
bughunter mcp        # stdio JSON-RPC — the only transport in V1
```

> **Built today:** the eight read tools — `get_project_context`, `scan`, `rescan`,
> `get_changes`, `get_impact`, `get_symbol`, `get_graph`, `doctor` — with response budgeting
> and structured domain failures. The bug, verification and fact tools land with the engines
> behind them; the server says so in its own instructions rather than leaving an agent to
> infer it.

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
needs a second `Engine` call to do its job, the missing method belongs in `nexus-core`, because
the CLI needs it too.

This is enforced structurally: **`nexus-mcp` does not depend on `nexus-store`, `nexus-lang*` or
`nexus-verify`** — a `cargo metadata` test fails the build otherwise. A handler physically
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
| `bughunter_investigate` | read+ai | `investigate` |
| `bughunter_answer` | read+ai | `answer` |

`bughunter_investigate` is the symptom-driven entry point: the agent reads a screenshot,
passes what it observed, and gets back a cross-stack trace with ranked suspects — or a
question. `bughunter_answer` resumes an investigation by id.
See [investigation.md](investigation.md).

The eleven tools named in the brief, plus seven: `get_symbol` and `get_tests_for` (agents ask
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
  "to_allow": "set execute = \"docker\" in .nexus/policy.toml",
  "test_written_to": ".nexus/generated-tests/BUG-104/"
}
```

The agent can then ask its human in plain language, with the exact command and the exact
config change to hand. The brief's rule — "the MCP server should never silently execute
dangerous commands" — is satisfied on both halves: never silently, and never without a
policy that a human committed to the repository.

Every `execute` and `ai` tool call writes an `audit_events` row naming the MCP client as
actor (`mcp:claude-code`), whether or not it was permitted.

---

## 6. Clarification

BugHunter must ask when a request is under-specified rather than pick a candidate and sound
certain about it. Any tool may return this in place of a result — `bughunter_investigate`
most often, but the variant is general.

```json
{
  "status": "clarification_required",
  "investigation_id": "inv-0142",
  "reason": "the symptom anchors to four components on this route",
  "resolved_so_far": {
    "route": "/checkout",
    "backend_reachable": ["CartController#get", "PricingService#totals"],
    "candidates": ["CartSummary", "CartLineItems", "PromoBanner", "TotalsPanel"]
  },
  "questions": [
    { "id": "which_area",
      "ask": "Which part of the page shows the wrong number — the line items, or the summary panel?",
      "options": ["CartLineItems  src/checkout/CartLineItems.tsx",
                  "TotalsPanel    src/checkout/TotalsPanel.tsx"],
      "why": "Both render a total, and they call different endpoints — /api/cart and /api/pricing.",
      "required": true }
  ],
  "can_proceed_without": true,
  "confidence_if_proceeding": 0.35
}
```

Five rules keep this useful rather than irritating:

- **Questions come from measured ambiguity, never a template.** One candidate means no
  question. Asking something BugHunter already knows is how a tool teaches people to ignore
  its questions.
- **Every question carries `why`**, so the agent can relay what would actually help instead
  of the human guessing at what the tool wants.
- **Options are concrete**, with file paths — an answer is a selection, not an essay.
- **`can_proceed_without` separates two situations**: cannot proceed at all, versus can
  proceed at confidence 0.35. Collapsing them turns every soft ambiguity into a hard block,
  and a tool that blocks constantly gets scripted around.
- **Resolved state comes back with the question**, so no work is discarded and the caller can
  see the tool is not starting from nothing.

The shape deliberately mirrors `permission_required` in §5. Both say the same thing: when
BugHunter must not proceed on its own, it returns a structured description of what it needs —
not an error, and not a guess.

## 7. Errors

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

## 8. Client configuration

The same server, four ways of pointing at it.

**Claude Code** — `.mcp.json` in the project root:
```json
{ "mcpServers": { "nexus": { "command": "nexus", "args": ["mcp"] } } }
```

**Codex** — `~/.codex/config.toml`:
```toml
[mcp_servers.nexus]
command = "nexus"
args    = ["mcp"]
```

**GitHub Copilot** — `.vscode/mcp.json`:
```json
{ "servers": { "nexus": { "command": "nexus", "args": ["mcp"] } } }
```

**Anything else** — spawn `bughunter mcp` and speak MCP over stdio.

No per-client code exists in the repository. `integrations/*/` contains configuration
snippets and prompt text only. If a new agent supports MCP, it is already supported.

---

## 9. A typical agent session

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

## 10. A screenshot session

```
  human → agent   [screenshot] "the cart total shows 0, but there are 3 items"

agent → bughunter_investigate { description, route: "/checkout",
                                visible_text: ["Нийт дүн","0 ₮"],
                                network: [{method:"GET",path:"/api/cart",status:200}] }
      ← clarification_required · 4 candidate components · 2 questions with `why`
        can_proceed_without: true, confidence_if_proceeding 0.35

agent → human    "Which part — the line items, or the summary panel at the bottom?
                  They call different endpoints."
human → agent    "the summary panel"

agent → bughunter_answer { investigation_id: "inv-0142", answers: [...] }
      ← trace   TotalsPanel → useCart → [calls_http GET /api/cart/:p]
                → CartController#get → CartService#totals → CartRepository → cart_items
        suspects  CartService#totals 0.81  (changed yesterday, no covering test)
                  CartDto.totalAmount 0.74  CONTRACT MISMATCH
        finding   BUG-118 · api-contract · detector: contract · confidence 0.90
                  backend serializes `total_amount`; TotalsPanel.tsx:34 reads `totalAmount`
```

The agent read the image; BugHunter never received it. And the finding that explains the
symptom was produced by a join between two indexed sides, with **no model involved** — which
left the agent's reasoning free for the part that genuinely needs judgement.
