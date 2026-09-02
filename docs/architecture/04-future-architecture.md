# Future architecture

Three candidates, evaluated, one chosen. The choice is made on practical engineering outcome,
not elegance.

---

## 1. The starting concept, criticised

The brief proposes:

```
                  AI Coding Agent
                        │
                        ▼
                     Nexus
                        │
        ┌───────────────┼───────────────┐
        ▼               ▼               ▼
   Context Engine    Task Intelligence  Verification
        │
        ├── Graphify
        ├── Git
        ├── Memory
        ├── Documentation
        └── Skills
```

It is the right instinct and the wrong decomposition. Six problems, in descending order of how
much damage each would do if built as drawn.

### C1 — Task Intelligence is not a sibling of the Context Engine. It is its first stage.

Understanding intent with no downstream consumer produces nothing. Drawn as peers, they become
two components that both parse the task, drift apart, and eventually disagree about what the
developer asked for — with no way to tell which one is right.

**Correction:** intent is stage 1 of the context pipeline. "Task Intelligence" names a stage,
not a subsystem.

### C2 — Verification is drawn as a peer with no inputs.

Verification needs the changed symbol set (the index), the affected region (the graph), and the
build/test commands (the profile). It is a *consumer* of the same substrate as the Context
Engine, not a third pillar floating beside it.

Drawn as a peer, it grows its own change detection — which is how you end up with two answers to
"what changed" and a very bad afternoon.

### C3 — Graphify is not a peer of Git and Memory.

Git and Memory are first-class, owned, always-present subsystems. Graphify is an **optional
external signal** for languages Nexus has no analyzer for. Listing it alongside them elevates a
crutch to a pillar, and a pillar is a thing you build on — which is precisely the
duplicate-indexer mistake the brief itself forbids.

**Correction:** Graphify is a conditional input to the seed and expand stages, ranked below
tree-sitter-derived edges, and deleted when a native analyzer lands. See §8.

### C4 — Skills are not a context source.

Documentation is a real source: ADRs, `CONTEXT.md`, design docs, all with `file:line` anchors.
**Skills are agent-side workflow** — how the agent should work, not what it should know.

Putting them under the Context Engine confuses the two, and invites Nexus to start shipping
process instructions inside a context package. It should not. Skills belong in
[`07-agent-integration.md`](07-agent-integration.md), on the agent's side of the boundary.

### C5 — The index is missing from the diagram entirely.

Graphify, Git, Memory, Documentation and Skills are named. The **symbol index and the dependency
graph with its cross-stack seam** — the thing that makes Nexus Nexus, and the only component in
the system nothing else can supply — is not.

A diagram that omits the load-bearing component will produce an architecture that under-invests
in it.

### C6 — There is no feedback edge.

Everything flows down. Nothing flows back. But a verification result must update finding status,
and a scan must validate or invalidate facts, or the system cannot learn — it can only serve a
snapshot that gets staler every day.

### The corrected decomposition

```
                  AI Coding Agent
                        │
       ┌────────────────┼────────────────┐
       │ hooks          │ MCP            │ commands
       ▼                ▼                ▼
  ┌─────────────────────────────────────────────┐
  │            CONTEXT ENGINE                   │
  │  intent → seeds → expand → rank → budget    │   ← Task Intelligence is stage 1
  └───────────────────┬─────────────────────────┘
                      │ reads
  ┌───────────────────▼─────────────────────────┐
  │  INDEX + GRAPH  ·  HISTORY  ·  MEMORY        │   ← the substrate. The index is first.
  │  symbols, edges,   commits,    facts,        │
  │  the seam          churn       findings      │
  └───────────────────┬─────────────────────────┘
                      │ reads          ▲
  ┌───────────────────▼──────────┐     │ writes back
  │        VERIFICATION          │─────┘               ← a consumer, and the feedback edge
  │  compile · test · lint       │
  └──────────────────────────────┘

  optional, conditional:  Graphify (unanalysed languages) · Documentation (ADRs, CONTEXT.md)
  outside the boundary:   Skills, workflows — the agent's side
```

Three layers, one direction of read, one feedback edge. Everything else in this document follows
from that shape.

---

## 2. The alternatives

### Architecture A — "More tools"

Keep the tool-per-question MCP surface and extend it: add git tools, verification tools, memory
tools. The agent orchestrates; Nexus answers questions one at a time.

### Architecture B — "Context Engine as the front door"

Insert one assembly pipeline between the Engine and the adapters. A single call takes a task
and returns a ranked, budgeted, explained context package. The existing tools remain as
primitives and as the escape hatch, but the *default* path is one call. Two new deterministic
subsystems — history and verification — feed the ranker. Hooks make invocation automatic.

### Architecture C — "Daemon / agent OS"

A long-running `nexusd` watches the filesystem, keeps a warm index, tracks agent sessions,
streams context proactively, and orchestrates verification in the background.

---

## 3. Evaluation

| Criterion | A — More tools | B — Context Engine | C — Daemon |
|---|---|---|---|
| **Accuracy** | Unchanged. Depends entirely on which tools the agent happens to call | Best. One ranking function, testable, with recorded reasons | Best-case equal to B; adds staleness as a new error class |
| **Token efficiency** | Worst. N calls × N payloads, plus the agent re-reads files anyway | Best. Budget is a selection policy, not a post-hoc trim | Equal to B |
| **Complexity** | Lowest | Medium: one module, two subsystems, a hook set | Highest: process lifecycle, IPC, cache coherence, cross-platform service management |
| **Performance** | Many round trips; each cheap | One round trip; ranking is in-process and bounded | Fastest warm; slowest cold; new failure surface |
| **Maintainability** | Surface area grows without a spine | Adds a spine the surface hangs off | Two execution models to keep in agreement forever |
| **Claude Code integration** | MCP only — model-decided | MCP **+ hooks** — deterministic injection | Hooks + streaming; needs a supervised process the user never asked for |
| **Graphify integration** | Ad-hoc | Clean: an optional structural-signal source behind the same seed/expand interface | Same as B |
| **Memory design** | Unchanged; rot persists | Lifecycle + invalidation become part of the retrieval path | Same as B |
| **Verification** | One more tool the agent may forget | A gate on the `Stop` hook — fires whether or not the agent remembers | Continuous; strongest, and most intrusive |
| **Future agent support** | Per-agent tool wiring | CLI verb is the contract; adapters are shells | Same as B plus a daemon protocol to port |
| **Failure modes** | Agent doesn't call, calls in the wrong order, or truncates blindly. *This is already today's failure mode* | Ranker is wrong → confidently wrong context. Visible via the ledger | Stale daemon, port conflicts, zombie processes, index/daemon disagreement — all silent |
| **Migration difficulty** | Trivial | Additive. Nothing existing is rewritten | Hard. Changes the process model everything else assumes |

---

## 4. The choice: **Architecture B**

### Why not A

A optimises feature count, which is not the stated goal. It leaves the primary optimisation
target — useful information per token — completely untouched, and it makes the existing failure
mode worse by adding more things the agent can fail to call. Sixteen tools that must be
orchestrated correctly by a model is not an intelligence layer; it is an API with good
documentation.

### Why not C

Three independent reasons, any one sufficient:

1. **The project's own trigger has not fired.** [ADR-006](../architecture-decisions.md) defers
   the daemon until `rescan > 2 s` or `impact p95 > 250 ms`. Measured today: 641 ms for a *full*
   scan of 880 files. Building it now is deciding by enthusiasm.
2. **It violates "avoid unnecessary technology"** — the brief's own constraint.
3. **It adds a silent failure class.** Every other failure in this system is loud. A daemon
   serving a stale index is not, and a wrong answer that looks right is the most expensive kind.

C stays on the roadmap as *Future*, gated on its number.

### Why B

- It is the only candidate that attacks the primary optimisation goal directly.
- It is **additive** to a codebase whose boundaries are sound (`current-state.md` §2): one new
  module in `nexus-core`, one new crate, extensions to two existing ones. No load-bearing wall
  moves.
- It converts the two largest gaps — nothing assembles context (P1), nothing invokes it
  automatically (P8) — into a single mechanism, so they are fixed by the same work.
- It makes the ranker's mistakes **visible** (the inclusion ledger) rather than silent, which
  is the difference between a system that can be improved and one that can only be trusted.
- It defers everything expensive behind a written trigger.

---

## 5. Component responsibilities

```
                        ┌──────────────────────────────────────┐
  adapters              │  nexus-cli   nexus-mcp   hooks/*.sh  │
  (thin, no logic)      └──────────────┬───────────────────────┘
                                       │ one call
                        ┌──────────────▼───────────────────────┐
  assembly              │  nexus-core::context                 │
                        │  intent · seeds · expand · rank ·    │
                        │  budget · package · ledger           │
                        └──────────────┬───────────────────────┘
                                       │
  ┌────────────────┬───────────────────┼─────────────────┬──────────────────┐
  │                │                   │                 │                  │
┌─▼────────┐  ┌────▼──────┐   ┌────────▼───────┐  ┌──────▼──────┐  ┌────────▼───────┐
│ index    │  │ graph     │   │ history        │  │ memory      │  │ verification   │
│ symbols  │  │ impact.rs │   │ core::history  │  │ facts       │  │ nexus-verify   │
│ files    │  │ seam      │   │ commits/churn  │  │ findings    │  │ compile/test   │
└─┬────────┘  └────┬──────┘   └────────┬───────┘  └──────┬──────┘  └────────┬───────┘
  │                │                   │                 │                  │
  └────────────────┴───────────────────┴─────────────────┴──────────────────┘
                                       │
                        ┌──────────────▼───────────────────────┐
  storage               │  nexus-store   (the only SQL)        │
                        └──────────────────────────────────────┘
```

### Existing crates — what changes

| Crate | Change |
|---|---|
| `nexus-types` | New DTOs: `ContextPackage`, `ContextItem`, `InclusionLedger`, `Verdict`, `Intent` |
| `nexus-store` | New ranked-retrieval queries; populate `commits`, `test_runs`, `test_coverage`, `finding_verifications`; **drop** `bugs*`; write `facts.invalidated_at` |
| `nexus-vcs` | Grow read-only history primitives: `log_since`, `numstat`, `blame_lines`. Still knows nothing of storage or languages |
| `nexus-lang` | Unchanged. Already correct |
| `nexus-core` | **Split `engine.rs`.** New modules: `context`, `history`, `graph`. `Rule` trait hoisted here from the three capabilities |
| `cap-*` | Delete their private `Rule`/`Detector` traits and `Graph`; implement `nexus_core::capability::Rule`. Rules themselves unchanged |
| `nexus-mcp` | Add `nexus_get_context`, `nexus_what_next`, `nexus_verify`. `budget::fit` demoted to a last-resort guard |
| `nexus-cli` | Add `context`, `next`, `verify`. `ask.rs` orchestration moves into `nexus-core` |

### New

| Component | Responsibility | Explicitly not |
|---|---|---|
| `nexus-core::context` | Task → ranked, budgeted, explained package | Never runs a model. Never reads whole files. Never talks to an agent |
| `nexus-core::history` | Persist commits; derive churn, recency, co-change, blame→symbol | Not a git wrapper — `nexus-vcs` stays that |
| `nexus-verify` (crate) | Plan and run compile/test/lint from the allowlist; return a `Verdict` | Never touches the store. Never generates tests (Phase 3). No `sh -c`, ever |
| `hooks/` | Shell shims: SessionStart, UserPromptSubmit, PostToolUse, Stop | No logic. Each is `nexus <verb>` with a timeout and `exit 0` on failure |

### Boundaries added to `tests/boundaries.rs`

- `nexus-verify` must not depend on `nexus-store` — it returns outcomes, the Engine persists them.
- `nexus-core::context` must not depend on any `cap-*` — capabilities feed it findings; it does not know their rules.
- `nexus-lang-*` remains forbidden from `nexus-store`/`nexus-core` (unchanged).
- **New, fixing P3:** `nexus-core` must not depend on any `nexus-lang-*` once the registry is
  injected at the composition root (Phase 3 — the test lands with the change, not before).

---

## 6. Data flow

### Cold: first contact with a project

```
SessionStart hook
  → nexus context --session
    → Engine::open_or_init
    → scan if no baseline        (detect profile, index, build graph, run Architect)
    → context::session_package   (profile · open findings · durable facts · scope warnings)
  → ~800 tokens injected into the agent's first turn
```

### Warm: the developer asks for something

```
UserPromptSubmit hook  ("fix the payment idempotency bug")
  → nexus context --task "<prompt>" --budget 4000
    → intent      : {verb: fix, kind: defect, targets: ["payment", "idempotency"]}
    → seeds       : fqn/path/text match → PaymentService, PaymentController, 2 facts
    → expand      : impact BFS from seeds, bounded            (reuses impact.rs)
    → signals     : churn · recency · coverage · prior findings · facts · profile
    → rank        : one scoring function, per-term recorded
    → budget      : greedy by score/token density + diversity guard
    → package     : items as file:line + 3-line window, never whole files
  → injected, with the ledger available on request
```

### After an edit

```
PostToolUse(Edit|Write) → nexus rescan --quiet     (no-op fast path; keeps the index warm)
Stop                    → nexus verify --changed
                            → changed set from rescan
                            → nexus-verify: compile · test · lint (allowlist, argv only)
                            → diff + impact cross-check
                            → Verdict {Verified | Failed{what} | Inconclusive{why}}
                          non-zero on Failed; the agent sees why, in its own turn
```

### The invariant across all three

Every arrow above is deterministic. No model is invoked anywhere in this diagram. The agent is
the only probabilistic component, and it sits outside the box.

---

## 7. Where the ten required capabilities live

| # | Capability | Home | Status |
|---|---|---|---|
| 1 | Task Intelligence | `core::context::intent` | New — deterministic classifier, no LLM |
| 2 | Context Engine | `core::context` | New — the spine |
| 3 | Graphify integration | `core::context::seeds` (optional source) | New, optional; see §8 |
| 4 | Engineering Memory | `facts` + `findings` + `core::context::retrieve` | Exists; needs lifecycle + invalidation |
| 5 | Skill / Workflow | `commands/`, `skills/`, `hooks/` | Exists; hooks are new |
| 6 | Agent Integration | `nexus-mcp`, `nexus-cli`, `hooks/` | Exists; hooks are new |
| 7 | Verification | `nexus-verify` | New crate |
| 8 | Git Intelligence | `nexus-vcs` + `core::history` | Primitives exist; derivation new |
| 9 | Architecture / Decisions | `facts` (`decision.*`, `arch.*`) + `cap-architect` | Exists; needs the markdown export |
| 10 | Learning / Feedback | `finding_occurrences` + fact lifecycle + `context.feedback` | Ledgers exist; the loop is new |

## 8. Graphify's place

Graphify is a **structural signal source**, not a second index and not a dependency.

It earns a place for exactly one reason: it parses languages Nexus does not, which today
includes the language Nexus is written in. Where `graphify-out/graph.json` exists, the seed and
expand stages may consult it for files whose language has no `LanguageAnalyzer`, marked with
`resolution = "external-graph"` and a confidence ceiling below tree-sitter-derived edges.

It is never required, never invoked automatically, and never mixed into the resolution
denominator. If Nexus gains a Rust analyzer (Phase 5.2), this path is dead weight and gets
deleted — which is the test of whether it was ever an integration or just a crutch.
