# Agent integration

Nexus is agent-agnostic. The developer's workflow does not change.

```bash
cd project
claude          # unchanged
```

---

## 1. The shape

```
Developer
  → Claude Code
    → Nexus capabilities when useful
```

Never:

```
Developer
  → Nexus
    → Nexus launches another agent      ✗
```

Nexus does not orchestrate agents, spawn agents, or contain an agent. It has no model client on
its critical path and, in the deterministic build, no HTTP stack in its dependency tree at all —
a `cargo metadata` test asserts this. That is not a limitation to lift later; it is what makes
Nexus something an agent can trust, because it is structurally incapable of guessing.

---

## 2. Three tiers, in order of preference

### Tier 1 — Hooks (deterministic invocation)

**The largest single gap in the product today.** There is no `hooks.json` anywhere in the
plugin surface, so every path into Nexus requires the model to *choose* to call it, and
`skills/nexus/SKILL.md` is a well-argued plea to remember.

Hooks remove the choice. They fire whether or not the model thinks of it.

| Hook | Command | Budget | Purpose |
|---|---|---|---|
| `SessionStart` | `nexus context --session --budget 800` | 800 tok / 400 ms | The agent starts knowing what the project *is* |
| `UserPromptSubmit` | `nexus context --task "$PROMPT" --budget 4000` | 4000 tok / 150 ms | The context package for this task. **This is the product.** |
| `PostToolUse` (Edit\|Write) | `nexus rescan --quiet` | 0 tok / 200 ms | Keep the index warm. No-op rescan is the fast path |
| `Stop` | `nexus verify --changed` | ~300 tok / 5 s | "Done" gets checked before the turn ends |

**Every hook is fail-open and time-bounded.** Hard timeout, `exit 0` on any failure, nothing
printed on failure. Principle 9: a tool that occasionally hangs a developer's session is
uninstalled once and never reinstalled. The budgets above are assertions, not hopes — each is
a test.

Hooks ship **off by default**, enabled by `nexus init --hooks` after the latency numbers have
been measured on the actual project. Turning on a per-prompt hook uninvited is exactly the kind
of change to how the developer works that the mission forbids.

### Tier 2 — MCP tools (the pull path, and the escape hatch)

The existing 16 tools stay. They are the primitives, and they are what an agent uses when it
wants something the package did not include. Three are added:

| Tool | Answers |
|---|---|
| `nexus_get_context` | "Give me what I need for this task" — the package, budgeted |
| `nexus_what_next` | "What should I look at?" — ranked changed symbols. Exists in the CLI as `ask next`, has never been reachable by an agent |
| `nexus_verify` | "Does this actually build and pass?" |

Two existing handlers break the repo's own rule that a handler is *deserialize → one Engine call
→ serialize*: `nexus_get_known` makes two, and `ask` orchestrates in the CLI. Both move into
`nexus-core`, which is where the rule says the missing method belongs.

### Tier 3 — Skills and slash commands (the explicit path)

Unchanged. `skills/nexus/SKILL.md` and the eight `commands/*.md` stay, for when a developer
wants to ask directly.

---

## 3. Why the CLI verb is the agent-agnostic contract

Every tier above is a shell over the same CLI verbs:

```
hooks/          →  nexus context | rescan | verify
nexus-mcp       →  Engine methods (same code path as the CLI)
commands/       →  nexus <verb>
```

Adding an agent means adding a shim, never touching `crates/`.

| Agent | Today | Added |
|---|---|---|
| Claude Code | MCP + skill + commands | hooks |
| Codex | MCP (`integrations/codex/config.toml`) | hook shims when Codex exposes them |
| Copilot | MCP (`integrations/copilot/mcp.json`) | as above |
| Anything else | `nexus context --task "…" --json` | — |

The last row is the real portability guarantee: a JSON-emitting CLI works for an agent that
does not exist yet and has never heard of MCP.

**No agent-specific logic ever enters the binary.** `if claude_code { … }` anywhere in
`crates/` is the smell the boundary tests exist to catch.

---

## 4. What the agent gets, concretely

`SessionStart`, ~800 tokens, before the agent has read a single file:

```
Nexus · nexus (Rust workspace, 13 crates)
  Java 21 + Spring Boot 3.2 · TypeScript/Next.js 14 · GraphQL · SQLite
  Build: cargo · Tests: cargo test (150) · CI: .github/workflows/ci.yml

Open findings (4)
  REV-003  REGRESSED  PaymentService#pay  no test reaches this  (broke, fixed, broke again)
  BUG-011  VERIFIED   OrderService#place  @Transactional on a non-public method
  ...

Durable knowledge (3)
  invariant.payment.status-transitions  PENDING→PAID→SETTLED only; no path back  [human]
  arch.payment.idempotency              enforced at the controller via Idempotency-Key,
                                        not in the service  [ai, validated, scan 38]

Scope warning
  This scan covers 1 of 6 modules. Impact answers here are understated.
```

Every line is a query result. Nothing was inferred, nothing was generated, no token was spent
producing it. Compare with the agent achieving the same understanding by reading files: a dozen
`Read` calls, most of the content discarded, and no history at all — because history is not in
the files.

---

## 5. What Nexus refuses to do for an agent

- **Run arbitrary commands.** There is no such tool over MCP and there will not be one. The
  allowlist is the entire execution surface.
- **Fix code.** It reports and proves; applying the fix is the agent's job with the evidence in
  hand. A tool that both diagnoses and treats has no independent check on itself.
- **Store conversations.** Rows, not transcripts.
- **Send telemetry.** No usage reporting, no update checks, no crash reporting. Ever.
- **Launch or coordinate agents.** Out of scope, permanently.

---

## 6. The five moments, end to end


The five moments Nexus serves. Each one is a sequence of deterministic calls; the agent is the
only probabilistic component and it sits outside every diagram here.

---

### 6.1 Arriving in an unfamiliar project

**Moment:** `cd project && claude`, first turn.

```
SessionStart hook
  → nexus context --session --budget 800
      no baseline?  → scan   (detect · index · graph · Architect)
      baseline?     → rescan (no-op fast path)
  → profile · open findings · durable facts · scope warnings
```

The agent's first turn already knows the languages, frameworks, build system, datastores, what
is currently broken, what previous sessions worked out, and whether the scan covers the whole
project or one module of something larger.

**What this replaces:** a dozen `Read` calls whose content is mostly discarded, producing an
understanding with no history in it — because history is not in the files.

**Capability at this moment:** Architect — *what is this, and what does working in it lack?*

---

### 6.2 Being asked to do something

**Moment:** the developer types a prompt.

```
UserPromptSubmit hook
  → nexus context --task "$PROMPT" --budget 4000
      intent → seeds → expand → signals → rank → budget → package
```

Worked example — *"fix the payment idempotency bug"*:

| Stage | Result |
|---|---|
| intent | `Debug` — findings and history weighted up, forward impact down |
| seeds | `PaymentController`, `PaymentService` (name match); fact `arch.payment.idempotency` (subject match) |
| expand | reverse impact from both seeds, depth 5, bounded |
| signals | `PaymentService#pay` churned 9× in 30 days; `REV-003 REGRESSED` sits on it; no test reaches it |
| rank | 61 candidates scored |
| budget | 12 items, 3,780 of 4,000 tokens, ≤ 3 per component |
| package | anchors + 3-line windows + the ledger |

The agent starts with the controller that actually enforces idempotency, the fact saying so, the
regression that already happened there, and the knowledge that nothing tests it.

**What this replaces:** the agent grepping for "idempotency", reading four files, and missing
that the enforcement point is the controller — which is exactly the fact a previous session paid
to discover and Nexus stored.

---

### 6.3 Finishing an edit

**Moment:** the agent has written code and is about to say "done".

```
PostToolUse(Edit|Write) → nexus rescan --quiet        keeps the index warm
Stop                    → nexus verify --changed
                            compile · test · lint · diff · impact
                            → Verdict
                          (analyze review over the changed scope)
```

Review reports what the diff cannot show: a change no test reaches, a contract change that
reaches frontend code nobody touched, a signature whose callers did not move with it.

**Capability at this moment:** Review — *what does this change reach, and what covers it?*

**What this replaces:** trust.

---

### 6.4 Chasing a defect

**Moment:** something is broken and the cause is unknown.

```
nexus context --task "<symptom>" --purpose debug
  → seeds from: a stack frame · a symbol name · a user-visible label (ui_strings, any locale)
  → forward impact from the seeds
  → prior findings on the reached set, weighted by status
  → facts about the modules involved

bughunter analyze --scope <reached set>
  → deterministic rules only: Spring proxy mistakes, orphaned GraphQL fields, committed secrets

agent reasons over the package
  → nexus record finding    (evidence required; model confidence clamped at 0.75)
```

The seam matters most here. A symptom visible in the UI reaches the backend method that serves
it, because the `.graphqls` schema is indexed as the contract. Nothing in the source text
connects `fetch('/api/x')` to `@QueryMapping`; Nexus connects them.

**Capability at this moment:** BugHunter — *where is it, and what proves it?*

---

### 6.5 The learning loop

**Moment:** continuous, and entirely deterministic.

```
     ┌──────────────────────────────────────────────────────┐
     │                                                      │
  scan/rescan ──▶ findings ──▶ agent reasons ──▶ record ────┤
     │               │                             │        │
     │               ▼                             ▼        │
     │          occurrences                      facts      │
     │          (append-only)                      │        │
     │               │                             ▼        │
     │               │                     validated by     │
     │               │                     the next scan    │
     │               ▼                             │        │
     └──────── verification ◀──────────────────────┘        │
                     │                                      │
                     └── status transitions ────────────────┘
                         VERIFIED · FIXED · REGRESSED
```

What each pass buys:

| Pass | Learned |
|---|---|
| scan 1 | index · profile · 3 facts · 2 findings `UNVERIFIED` |
| scan 2 | 4 files → 17 symbols → 11 affected. Analysis costs 11, not 5,665. Fingerprints match: occurrences appended, no duplicates |
| scan 3 | verification passes → `FIXED`, commit recorded |
| scan 9 | the same check fails → `REGRESSED`, with both the original introduction and the fix attached |
| scan 12 | a fact's evidence symbol changed → `invalidated_at` set; it stops being retrieved rather than quietly misleading |

Scan 9's conclusion is only possible because scans 1, 3 and 9 all still exist, unedited. That is
the append-only doctrine paying for itself.

**No model appears anywhere in this loop.** Learning here means ledger rows and status
transitions — which is why it is reliable, and why it costs nothing.

---

### 6.6 What the developer does differently

Nothing.

```bash
cd project
claude
```

Hooks fire, or they do not (off by default until measured on the project). Tools are called, or
they are not. Slash commands exist for when someone wants to ask directly. At no point is the
developer asked to run Nexus, configure Nexus, or change how they work — which is the entire
point of the mission statement, and the constraint most easily lost while adding ten
capabilities.
