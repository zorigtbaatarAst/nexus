# BugHunter — Symptom-Driven Investigation

Everything else in BugHunter starts from a **change**: what did this commit break. That is
not how bugs actually arrive. They arrive as a person pointing at a screen and saying
*“this number is wrong.”*

This document specifies the second entry point: a screenshot plus a sentence, traced through
the indexed architecture to a ranked suspect set spanning frontend, backend, and the contract
between them.

```
   change-driven          rescan → impact → hunt the affected region
   symptom-driven         observations → anchor → trace across the seam → rank suspects
```

Both end in the same place — evidence handed to an agent, findings fingerprinted, bugs
verified by reproduction. Only the seed differs. See
[ADR-013](architecture-decisions.md#adr-013-symptom-driven-investigation-as-a-second-entry-point).

---

## 1. BugHunter never sees the screenshot

The agent reads the image. BugHunter receives **observations**, never pixels.

This is not squeamishness; it is the same split the whole product rests on. Vision is
reasoning, and reasoning belongs to the agent that already has a model, a context window and
a paying user. Putting an OCR stack or a vision model inside a Rust binary would add
hundreds of megabytes, a GPU expectation and a vendor relationship, to do worse what the
caller can already do.
See [ADR-016](architecture-decisions.md#adr-016-the-agent-reads-the-image-bughunter-never-receives-it).

```rust
pub struct SymptomReport {
    pub description:     String,               // "cart total shows 0 but 3 items are listed"
    pub expected:        Option<String>,       // "total should be 45,000"
    pub route:           Option<String>,       // "/checkout" — from the URL bar
    pub visible_text:    Vec<String>,          // labels read off the screen
    pub component_hints: Vec<String>,          // from React DevTools, if visible
    pub network:         Vec<NetworkObservation>,
    pub console:         Vec<String>,          // errors visible in the console
    pub screenshot:      Option<EvidenceRef>,  // path + hash, RECORDED, never analyzed
    pub since:           Option<Revision>,     // "worked yesterday" → a commit or a date
}

pub struct NetworkObservation {
    pub method: String, pub path: String,
    pub status: Option<u16>, pub body_excerpt: Option<String>,
}
```

`screenshot` is stored as a path and a hash on the bug record so the report has provenance.
It is never opened by BugHunter. The field is evidence, not input.

---

## 2. Anchoring — observations to symbols

Four independent mechanisms, each deterministic, each producing candidates with a confidence.
They are run in parallel and their results are merged.

| # | Signal | Mechanism | Confidence |
|---|---|---|---|
| A | `route` | frontend framework pack resolves the URL to a component | 0.90 |
| B | `visible_text` | FTS lookup over the indexed string and i18n index | 0.60–0.95 |
| C | `network` | the observed path matches an indexed HTTP route symbol | 0.95 |
| D | `console` | stack frames and file names map to indexed files | 0.85 |

### A · Route → component

The frontend pack already extracts the route table during `scan`: Next.js file-system
routing (`app/checkout/page.tsx`), React Router route arrays, Angular route modules, Vue
Router. `/checkout` resolves to the page component and, through the existing `calls` and
`imports` edges, to the component subtree it renders.

### B · Visible text → component

The most useful signal in practice, and the one that needs a new index. During `scan`, every
user-visible string is recorded — JSX text nodes, `aria-label`, `data-testid`,
`placeholder`, and i18n keys with their values in every locale:

```sql
CREATE VIRTUAL TABLE ui_strings_fts USING fts5(text, content='ui_strings');
```

A label read off the screenshot becomes an FTS query. `"Нийт дүн"` appears in exactly one
component and pins the anchor immediately. This works across locales: the screenshot may be
in Mongolian while the source holds an i18n key, and matching the *value* still reaches the
key, and the key reaches the component.

Confidence scales with specificity: a string appearing in one component scores 0.95; one
appearing in nine scores 0.60 and becomes a clarification candidate rather than an anchor.

### C · Network observation → route

The strongest signal when present. `GET /api/cart 200` matches a `kind='route'` symbol
directly, which pins the **backend** end of the trace without needing the frontend end at
all. A 4xx or 5xx status collapses the search space enormously — which is exactly why the
clarification protocol asks for it when it is missing.

### D · Console error → file

Stack frames, bundle-mapped file names and thrown message text map to indexed files. Weaker
than it looks in production builds, where minification destroys the frames — so it is a
supporting signal, not a primary one.

### Convergence

Anchors that agree reinforce each other. If the route resolves to `CartSummary` and a
visible label also resolves to `CartSummary`, confidence rises above either alone. If they
**disagree**, that is not averaged away — it is a clarification, because two mechanisms
pointing at different components usually means the description is about a part of the page
the route did not predict.

---

## 3. Crossing the seam

The dependency graph stops at each language boundary: a TypeScript `fetch()` and a Java
`@PostMapping` are unrelated symbols. Joining them is what makes cross-stack tracing possible
at all, and it is done at the **HTTP contract**, not by any shared schema.
See [ADR-014](architecture-decisions.md#adr-014-join-the-stack-at-the-http-contract).

Both sides already produce route data through their framework packs. Resolution matches them
on a canonical form:

```
frontend   fetch(`${API}/api/cart/${cartId}/items`)     → GET /api/cart/:p/items
backend    @GetMapping("/api/cart/{cartId}/items")      → GET /api/cart/:p/items
                                                          ────────────┬───────────
                                            symbol_edges edge_type = 'calls_http'
                                                     resolution = 'contract'
```

**Canonicalization rules**

| Input | Canonical |
|---|---|
| `${id}`, `{id}`, `:id`, `<int:id>` | `:p` — path parameters are positional, not named |
| trailing slash | stripped |
| `NEXT_PUBLIC_API_URL + "/api/x"`, axios `baseURL` | prefix resolved from `config.toml` |
| gateway rewrite `/api/* → /*` | applied from `[http.rewrite]` in `config.toml` |
| query string | dropped from the join key, kept as contract metadata |

No new tables are needed. A backend route is already a symbol with `kind='route'`; a frontend
call site emits an edge with `dst_fqn_hint = "GET /api/cart/:p/items"`; and the existing
Tier-3 unresolved-edge sweep resolves it on the next scan using `idx_edges_unresolved`. The
seam reuses machinery that already exists for exactly this shape of problem.

**Unmatched is reported, not hidden.** A frontend call with no matching backend route stays
`unresolved` and is surfaced — it is either a genuine dead endpoint, a rewrite BugHunter was
not told about, or a service outside the repository. All three are worth knowing, and
silently dropping the edge would make the trace lie by omission.

**OpenAPI, when present**, is used as a third source: it supplies path templates, status
codes and response shapes without parsing either side. It is treated as evidence with
`spec_source='openapi'`, not as truth — a spec that has drifted from the handler is itself a
finding.

---

## 4. The trace

Once anchored, the trace is a forward traversal that already works — `calls_http` is just
another edge type in the existing BFS.

```
CartSummary.tsx                          UI anchor          confidence 0.95
  └─ calls ─────────▶ useCart()
       └─ calls_http ─▶ GET /api/cart/:p                    ← the seam
            └─ routes ─▶ CartController#get
                 └─ injects ─▶ CartService#totals
                      └─ injects ─▶ CartRepository#findItems
                           └─ persists ─▶ cart_items
```

Every hop carries its edge type, its resolution tier and its confidence, so the trace can be
shown to a human as a chain of claims rather than an assertion. A hop resolved
`heuristic` at 0.62 is visibly the weak link.

---

## 5. Ranking suspects

The trace says what is *involved*. Ranking says what is *likely*.

```
suspicion(symbol) =
      on_trace_score          -- position on the trace, decayed from the anchor
    × recency_factor          -- changed in the last N scans or since `report.since`
    × prior_bug_density       -- open or historical bugs in this component
    × coverage_gap            -- inverse of test_coverage confidence
    × contract_penalty        -- 3.0 if a contract mismatch was detected on this hop
```

Each factor is deterministic and comes from a table that already exists:

- **`recency_factor`** joins the trace against `changes`. A symptom the user says appeared
  yesterday, on a symbol that changed yesterday, is not a coincidence. When
  `report.since` is given, this becomes the dominant term.
- **`prior_bug_density`** joins against `bugs` and `bug_occurrences` by component. Code that
  has broken before breaks again.
- **`coverage_gap`** joins against `test_coverage`. A symbol on the trace with no covering
  test is a better suspect than one with three.
- **`contract_penalty`** is the multiplier that makes the next section matter.

The output is a ranked list with the path and the reason for each rank — never a single
confident accusation. BugHunter narrows a stack to five candidates with evidence; the agent
reads them and reasons.

---

## 6. Cross-stack contract mismatch — a deterministic detector

The classic screenshot complaint — *“the total shows 0 but the data is there”* — is very
often not a logic bug on either side. It is the two sides disagreeing about the payload. Once
the seam is joined, that disagreement is **computable with no model at all**.

| Mismatch | Detected by | Example |
|---|---|---|
| field name | response DTO fields vs. frontend property accesses | backend `total_amount`, frontend reads `totalAmount` |
| envelope shape | return type vs. destructuring | Spring returns `Page{content:[]}`, frontend maps `response.data[]` |
| type | DTO field type vs. frontend usage | `BigDecimal` serialized as string, frontend does arithmetic on it |
| nullability | `Optional<T>` / nullable column vs. unguarded access | `total` absent, frontend renders `0` |
| status codes | handler's possible responses vs. handled branches | backend can 409, frontend only handles 200 |
| enum values | backend enum constants vs. frontend union type | backend adds `PARTIALLY_REFUNDED`, frontend switch has no case |

These become `api-contract` findings with **`detector = 'contract'` and confidence 0.90** —
deterministic, evidence-backed, no AI involved, and not subject to the 0.75 model clamp
because no model was asked. Exactly the division of labour in
[ai-integration.md](ai-integration.md) §5: if a rule can express it, do not spend a token
guessing at it.

---

## 7. The clarification protocol

BugHunter must **ask when the task is incomplete or under-specified**, rather than pick a
candidate and sound certain about it. Any tool may return this instead of a result.

```json
{
  "status": "clarification_required",
  "reason": "the symptom anchors to four components on this route",
  "resolved_so_far": {
    "route": "/checkout",
    "backend_reachable": ["CartController#get", "PricingService#totals"],
    "candidates": ["CartSummary", "CartLineItems", "PromoBanner", "TotalsPanel"]
  },
  "questions": [
    {
      "id": "which_area",
      "ask": "Which part of the page shows the wrong number — the line items, or the summary panel at the bottom?",
      "options": ["CartLineItems  src/checkout/CartLineItems.tsx",
                  "TotalsPanel    src/checkout/TotalsPanel.tsx"],
      "why": "Both render a total, and they call different endpoints — /api/cart and /api/pricing.",
      "required": true
    },
    {
      "id": "network",
      "ask": "Was there a failing request in the Network tab? Its method, path and status.",
      "why": "A non-200 would pin the backend end of the trace immediately and skip the guesswork.",
      "required": false
    }
  ],
  "can_proceed_without": true,
  "confidence_if_proceeding": 0.35
}
```

The rules that make this useful rather than annoying:

1. **Questions come from measured ambiguity, never from a template.** If anchoring produced
   one candidate, BugHunter does not ask — asking a question whose answer it already has is
   how a tool teaches people to ignore its questions.
2. **Every question carries `why`.** The agent relays it, and the human then knows what
   would actually help instead of guessing at what the tool wants.
3. **Options are concrete**, with file paths, so the answer is a selection rather than an
   essay.
4. **`can_proceed_without` separates two different situations** — *cannot proceed at all*
   from *can proceed, at confidence 0.35*. Collapsing them turns every soft ambiguity into a
   hard block, and a tool that blocks constantly gets scripted around.
5. **What is already resolved is returned with the question.** The work done so far is not
   thrown away, and the human can see the tool is not starting from nothing.
6. **A question is asked at most once per investigation.** Answers are carried in the
   `investigation_id` so a follow-up call resumes rather than re-interrogates.

The shape deliberately mirrors `permission_required` from [mcp-api.md](mcp-api.md) §5. Both
are the same idea: when BugHunter must not proceed on its own, it returns a structured
description of what it needs, not an error and not a guess.
See [ADR-015](architecture-decisions.md#adr-015-structured-clarification-instead-of-guessing).

---

## 8. Worked example

```
agent → bughunter_investigate {
          description:  "cart total shows 0 but 3 items are listed",
          expected:     "total should be 45,000",
          route:        "/checkout",
          visible_text: ["Нийт дүн", "0 ₮", "3 бараа"],
          network:      [{ method:"GET", path:"/api/cart", status:200 }],
          since:        "worked yesterday"
        }

  ← anchor    route /checkout          → CheckoutPage            0.90
              text "Нийт дүн"          → TotalsPanel.tsx:34      0.95   (1 occurrence)
              network GET /api/cart    → CartController#get      0.95
              convergence: TotalsPanel ← CheckoutPage subtree     → anchored, no question

  ← trace     TotalsPanel
                → useCart()
                → calls_http GET /api/cart/:p          resolution contract  0.95
                → CartController#get
                → CartService#totals
                → CartRepository#findItems
                → cart_items

  ← suspects  1. CartService#totals              0.81
                 on trace · changed in scan-013 (yesterday) · no covering test
              2. CartDto.totalAmount             0.74
                 CONTRACT MISMATCH — backend serializes `total_amount`,
                 TotalsPanel.tsx:34 reads `totalAmount` → undefined → renders 0
              3. CartRepository#findItems        0.31
                 on trace, unchanged, covered by 2 tests

  ← finding   BUG-118  api-contract  detector: contract  confidence 0.90
              evidence: CartDto.java:22, TotalsPanel.tsx:34
              introduced: scan-013 / commit 4b21e0a
```

Suspect 2 was found with **no model involved**. The field names disagree, both sides are
indexed, and comparing them is a join. The agent's reasoning is then spent on whether
`CartService#totals` also has a problem — the part that genuinely needs judgement.

---

## 9. Surfaces

**MCP** — `bughunter_investigate`, class `read+ai`. Returns an `InvestigationReport`, or
`clarification_required`. A follow-up call passes `investigation_id` plus `answers` and
resumes from the stored anchors.

**CLI** — `bughunter investigate`, which prompts interactively on a TTY and returns the
structured clarification under `--json`:

```bash
bughunter investigate \
  --description "cart total shows 0 but 3 items are listed" \
  --route /checkout \
  --text "Нийт дүн" --text "0 ₮" \
  --network "GET /api/cart 200" \
  --since yesterday

bughunter investigate --json --answers which_area=TotalsPanel   # non-interactive
```

Interactive mode asks the same questions a human would be asked over MCP — the protocol is
one mechanism with two presentations, so the CLI can never drift into asking something the
MCP path silently assumes.

---

## 10. What this requires that BugHunter does not have yet

Honest dependency list, all landing in V1:

| Needed | Where it lands |
|---|---|
| frontend framework packs — Next.js, React Router, Angular, Vue | `nexus-lang-ts` |
| `ui_strings` table + FTS5 index over labels and i18n | `nexus-store` |
| HTTP call-site extraction — `fetch`, `axios`, generated clients | `nexus-lang-ts` framework packs |
| `calls_http` edge type and its contract resolution tier | `nexus-core::resolve` |
| `[http.rewrite]` and base-URL configuration | `config.toml` |
| DTO shape extraction on both sides for mismatch detection | `nexus-lang-java`, `nexus-lang-ts` |
| the clarification protocol | `nexus-core`, surfaced by `nexus-mcp` and `nexus-cli` |

None of it changes an existing boundary or table classification. The seam is an edge type,
the anchor index is one table, and the clarification protocol is a result variant — which is
the test of whether the original layering was right.
