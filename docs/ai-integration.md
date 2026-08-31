# BugHunter — AI Integration

Two rules shape this entire document:

1. **AI is optional.** Every deterministic capability works with no provider, no API key and
   no network. `bughunter scan` on an air-gapped machine is a fully functional product.
2. **AI proposes, evidence disposes.** A model's output is a *candidate* until something
   deterministic — a compiler, a test, a graph lookup — agrees with it.

---

## 1. Two ways AI enters the system

```
  A. AGENT-AS-PROVIDER  (default)          B. DIRECT PROVIDER  (opt-in)

  Claude Code / Codex / Copilot            bughunter hunt --provider claude
        │                                        │
        │ MCP                                    │ Engine::find_bugs
        ▼                                        ▼
   BugHunter returns an evidence bundle    ContextBuilder → AiProvider → HTTPS
        │                                        │
   the agent reasons                        the model reasons
        │                                        │
   bughunter_record_bug  ──────────────────▶  record_bug
```

**A is the default and the primary path.** Under MCP, BugHunter calls no model at all. It
hands the calling agent a structured evidence bundle; the agent — which already has a model,
a context window and a paying user — does the reasoning and writes findings back. Zero API
keys, zero cost inside BugHunter, and identical behaviour for every MCP client.
See [ADR-005](architecture-decisions.md#adr-005-agent-as-ai-provider-by-default).

**B exists for headless use**: CI, cron, `bughunter hunt` in a terminal with no agent
attached. It requires a configured provider and an explicit `ai = "provider"` in policy.

---

## 2. The abstraction

```rust
pub trait AiProvider: Send + Sync {
    fn id(&self) -> &str;                       // "claude" | "openai" | "gemini" | "local"
    fn capabilities(&self) -> Capabilities;     // context window, JSON mode, tool use, cost tier
    async fn analyze(&self, req: AnalysisRequest) -> Result<AnalysisResponse, AiError>;
}

pub struct Capabilities {
    pub context_tokens:  u32,
    pub structured_output: bool,
    pub cost_tier:       CostTier,   // Free | Low | High  — drives budget decisions
    pub offline:         bool,       // true for LocalProvider; gates the redaction requirement
}
```

```
AiProvider
├── NullProvider     — the default. Returns an empty candidate set. Never errors.
├── AgentProvider    — not an HTTP client: marks that reasoning happens out-of-process (MCP)
├── ClaudeProvider   ┐
├── OpenAIProvider   ├─ behind cargo features; each is one file, ~200 lines
├── GeminiProvider   │
└── LocalProvider    ┘  Ollama / llama.cpp / any OpenAI-compatible local endpoint
```

`bh-core` depends on `bh-ai` with `default-features = false`, which compiles the trait, the
request/response types, `NullProvider` and `AgentProvider` — **and no HTTP client at all**.
A deterministic build has no `reqwest` in its dependency tree. That is constraint 1 ("core
must not depend on Claude") and constraint 3 ("AI is optional") expressed as a build fact
rather than a promise, and it is checked by the `cargo metadata` boundary test.

---

## 3. The request: a bundle, never a repository

```rust
pub struct AnalysisRequest {
    pub task:            AnalysisTask,     // FindBugs | ExplainImpact | PlanReproduction | Summarize
    pub project:         ProjectDigest,    // language, framework, build, ~200 tokens
    pub focus:           Vec<SymbolSlice>, // changed symbols + capped source excerpts
    pub impact:          Vec<ImpactPath>,  // fqn → fqn chains, with min_confidence
    pub facts:           Vec<Fact>,        // top-K relevant, from memory-model.md §3
    pub prior_bugs:      Vec<BugDigest>,   // same component: fingerprint, type, status
    pub tests:           Vec<TestRef>,     // names only, no bodies
    pub budget:          TokenBudget,
}
```

`ContextBuilder` assembles it under a hard token budget (default 24 k, configurable):

```
1. project digest                                        ~200 tokens   always
2. changed symbols, highest impact first, bodies capped at 60 lines   ~60 %
3. impact paths, one line each                                        ~10 %
4. relevant facts, top-12 by the relevance function                   ~10 %
5. prior bugs in the same component                                   ~10 %
6. test names                                                          ~5 %
7. fill remaining budget with the next-highest-impact symbols
```

Anything that does not fit is dropped by rank, and the request records what was dropped so
the response can be judged accordingly. **The repository is never a fallback.** There is no
code path that widens the context to "just include the whole file" — constraint 9 is a
property of `ContextBuilder`, not a guideline for prompt authors.

---

## 4. The response: evidence or rejection

```rust
pub struct AnalysisResponse { pub candidates: Vec<BugCandidate>, pub facts: Vec<FactInput>, … }

pub struct BugCandidate {
    pub title:            String,
    pub bug_type:         BugType,
    pub component:        String,
    pub anchor:           SymbolRef,
    pub hypothesis:       String,
    pub severity:         Severity,
    pub confidence:       f32,
    pub structural_key:   String,        // feeds the fingerprint
    pub evidence:         Vec<CodeRef>,  // MUST be non-empty
}
```

**A `BugCandidate` with an empty `evidence` vector is rejected at the boundary and never
reaches the store.** Not down-ranked — rejected. Every `CodeRef` is additionally validated:
the file must exist in the index, the line must be within the symbol's range, and the
excerpt hash must match what is on disk. A model that describes a plausible bug in a method
that does not exist produces zero rows.

This is the mechanical form of constraint 14, "prefer deterministic evidence over AI
assumptions". Rejections are counted and reported (`3 candidates rejected: unverifiable
evidence`), because a silently discarded finding is indistinguishable from a model that
found nothing.

Confidence from a model is **clamped at 0.75**. Only the verification engine can push a bug
above that, and only by reproducing it. A model is not permitted to grade its own work.

---

## 5. What AI is and is not asked to do

| Deterministic — never sent to a model | AI — genuinely needs judgement |
|---|---|
| syntax errors, type errors | business-logic errors |
| unused variables, dead code | edge cases the tests do not cover |
| formatting, style, lint rules | race conditions and interleaving |
| known CVEs in dependencies | transaction boundary problems |
| Semgrep pattern matches | data-consistency violations across services |
| null dereference on a known-null path | incorrect assumptions about callers |
| test pass/fail | subtle security issues in business flows |
| call-graph reachability | error-handling gaps that matter |
| coverage gaps | whether a change is a behavioural regression |

The right column has one thing in common: a compiler cannot express the property being
violated. The left column is cheaper, faster and more reliable without a model — spending
tokens there is spending money to become less accurate.

---

## 6. Redaction and data flow

Before any bundle leaves the process — path B only; path A never leaves the machine at all —
it passes `bh-ai::redact`:

| Detector | Action |
|---|---|
| path deny-list (`.env`, `*.pem`, `*.key`, `secrets/**`, `credentials*`) | excluded from context entirely, upstream |
| AWS/GCP/Azure key shapes, GitHub/Slack tokens, JWTs, PEM blocks | replaced with `«REDACTED:aws_key»` |
| connection strings with credentials | user:password replaced, host kept |
| assignments to `password`/`secret`/`token`/`api_key` | value replaced |
| high-entropy string literals ≥ 32 chars | replaced, flagged for review |

Redactions also **create deterministic `security`-type bug candidates** — a hardcoded key is
a finding, not just a redaction. Finding it while protecting it costs nothing extra.

Every outbound request writes an `audit_events` row with the provider, task, token count,
redaction count and a **hash of the payload — never the payload**. Storing the prompt would
recreate the exact leak the redactor exists to prevent, inside BugHunter's own database.

`bughunter doctor --ai` prints the full data-flow statement: which provider, which endpoint,
what leaves the machine, what is redacted, what is logged.

---

## 7. Claude Code integration

Thin by contract. `integrations/claude-code/` contains **no intelligence** — it is
configuration and six short prompt files that say which MCP tools to call in what order.

```
integrations/claude-code/
├── .mcp.json                     { "bughunter": { "command": "bughunter", "args": ["mcp"] } }
├── commands/bughunter/
│   ├── scan.md      /bughunter:scan      first-time index; report the profile
│   ├── rescan.md    /bughunter:rescan    changes + impact since baseline
│   ├── hunt.md      /bughunter:hunt      rescan → impact → reason → record_bug
│   ├── verify.md    /bughunter:verify    verify_bug, report the judgement honestly
│   ├── status.md    /bughunter:status    baseline, open bugs, regressions
│   └── explain.md   /bughunter:explain   project_context + facts for a symbol or module
└── skills/bughunter/SKILL.md     when to reach for BugHunter unprompted
```

A command file is ~15 lines. `hunt.md`, the longest, in full outline:

```markdown
1. Call bughunter_rescan. If no changes, say so and stop.
2. Call bughunter_get_impact for the changed symbols.
3. For the highest-impact symbols, call bughunter_get_symbol with detail: "full".
4. Reason about business-logic errors, races, transactions, data consistency.
   Do NOT report anything a compiler or linter would catch — BugHunter already did.
5. For each finding, call bughunter_record_bug with concrete file:line evidence.
   A finding you cannot cite is not a finding.
6. Offer to run /bughunter:verify on anything above medium severity.
```

The test of whether this layer is thin enough: **deleting `integrations/claude-code/`
entirely must cost no capability** — every tool remains callable, just without the shorthand.
The same applies to `integrations/codex/` and `integrations/copilot/`, which are one
configuration snippet each against the identical tool surface.

---

## 8. When AI is off

`policy.ai = "off"`, or no provider configured, or `--no-ai` on any command:

```
✓ init · scan · rescan · status · changes · impact · history · doctor
✓ deterministic detectors: compiler, test runner, linter, Semgrep, secret scan,
  dependency CVEs, graph-derived findings (unreachable code, missing null checks
  on a known-null path, transaction boundary violations by annotation analysis)
✓ verification of mechanical bug classes via deterministic templates
✗ business-logic, subtle-race and data-consistency hunting
```

The banner reads `ai: disabled` and every command exits 0. Requiring a model to run at all
would make BugHunter useless in exactly the environments — regulated, air-gapped, offline —
where a change-aware bug tool is worth the most.
