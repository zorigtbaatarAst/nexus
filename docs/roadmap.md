# BugHunter — Roadmap

Three releases. Each one ships a claim that is true and useful on its own — no release
depends on the next one to be worth installing.

---

## MVP — v0.1 · "What changed, and what it touches"

**The claim:** *BugHunter tells you exactly which symbols changed since your baseline and
exactly what they affect — provably, incrementally, with no AI involved.*

That is already a product. Nothing else in this milestone requires a model, an API key or a
network connection.

### Scope

**Delivered so far** — `init` `scan` `rescan` `status` `changes` `impact` `graph` `doctor`;
Java and TypeScript analyzers; the dependency graph with four resolution tiers; the GraphQL
seam. Measured on a 880-file Spring + Next.js project: 5,665 symbols and 96 % of in-project
edges resolved in 641 ms.

| Area | In | Out |
|---|---|---|
| Languages | Java (+Spring pack), TypeScript | Python, Rust |
| Resolution | Tiers 0–2 (tree-sitter, heuristic, framework) | LSP sidecar |
| Commands | `init` `scan` `rescan` `status` `changes` `impact` `doctor` `mcp` | `bugs` `verify` `history` |
| Storage | full schema, all 20 tables, migrations | archival, export/import |
| MCP | read tools: `init` `scan` `rescan` `scan_status` `get_project_context` `get_symbol` `get_changes` `get_impact` `get_tests_for` | bug and verify tools |
| Detectors | compiler, test runner, secret scan | AI hunting, Semgrep |
| AI | none — `NullProvider` only | providers, agent write-back |
| Verification | none | the whole engine |
| Output | human + `--json`, exit codes | `--fail-on` |

The bug tables exist in the schema from day one even though nothing writes to them. Adding
them later would mean migrating a populated database for no reason; adding them now costs a
few hundred lines of DDL.

### Definition of done

- All four golden fixtures index correctly; the property test `incremental ≡ full` passes.
- The reformat commit in `spring-payments` produces **zero** symbol changes.
- The rename commit carries symbol identity with no duplicate churn.
- No-op `rescan` under 300 ms at 500 KLOC.
- `bughunter mcp` passes the conformance suite; Claude Code, Codex and Copilot each connect
  with the shipped config snippet.
- `doctor` diagnoses every configuration failure mode with a remedy.

### Deliberately deferred, and why

Bug detection is deferred because *finding* changed symbols correctly is the hard, unglamorous
prerequisite. A bug hunter built on an unreliable change detector reports noise, and nobody
can tell whether the noise came from the model or the index.

---

## V1 — v1.0 · "Found it, and proved it"

**The claim:** *BugHunter finds bugs in the changed region, remembers them across scans
without duplicating them, and proves the real ones by making them happen.*

The full loop. This is the release the product is designed around.

### Scope

**Bug intelligence**
- Fingerprinting with alias resolution and near-duplicate linking.
- The full status machine, including the rule that `FIXED` requires evidence.
- Regression detection across scans; `bug_relations`.
- Semgrep integration for deterministic pattern findings.

**Verification engine**
- Reproduction planning, deterministic templates per `(bug_type, framework)`.
- `SafeWriter` jail; generated tests under `.bughunter/generated-tests/`.
- Docker sandbox with the profile in [security.md](security.md) §4; host opt-in.
- The **baseline-revision run** and the full judgement matrix.
- `verify --promote` to move a reproduction into the project's real test tree.

**AI**
- Agent-as-provider over MCP: evidence bundles out, `record_bug` / `record_fact` back.
- `ContextBuilder` with a hard token budget; candidate rejection without evidence.
- Redaction pass and the audit log.
- `ClaudeProvider` and `OpenAIProvider` behind features, for headless CLI and CI.

**Symptom-driven investigation** — the second entry point, [investigation.md](investigation.md)
- Frontend framework packs: Next.js, React Router, Angular, Vue route tables.
- `ui_strings` + FTS5 over labels, `aria-label`, `data-testid` and i18n values in every locale.
- HTTP call-site extraction (`fetch`, `axios`, generated clients) and the `calls_http` seam.
- Cross-stack contract mismatch detection — deterministic, no model.
- The clarification protocol, surfaced identically over MCP and the CLI.

**Languages** — Python and Rust analyzers, with Django/FastAPI and axum/sqlx packs.

**CLI** — `bugs` `bug` `verify` `history` `hunt` `explain` `ignore` `fact` `export` `import`
`prune`; `--fail-on <severity>` and exit code 3.

**Integrations** — `integrations/claude-code/` with six slash commands and a skill;
config snippets for Codex and Copilot.

### Definition of done

- `spring-payments` commits 3 → 6 → 7 produce `VERIFIED` → `FIXED` → `REGRESSED` with the
  correct commits recorded at each step.
- The rename commit does not duplicate the bug found before it.
- A candidate with fabricated evidence is rejected and counted.
- `policy.execute = "none"` yields `permission_required` over MCP, never an execution.
- Every one of the brief's constraints has a passing test, per the traceability table in
  [architecture.md](architecture.md) §9.
- On a `next-storefront` fixture whose backend renames a DTO field, `investigate` with the
  visible label and the route reaches the contract mismatch **without a model**, and reaches
  it from a Mongolian label through the i18n value index.
- When two components on a route both render the reported label, `investigate` returns a
  question rather than a candidate — asserted, because a template-generated question would
  pass a looser test.

---

## V2 — v1.x · "Fast, precise, and shared"

**The claim:** *BugHunter keeps up with a monorepo, resolves symbols exactly, and a team
shares one bug memory.*

Everything here is an optimization or a scale answer for a problem that must be **measured
first**. Each item names its trigger.

| Feature | Trigger |
|---|---|
| `bughunterd` daemon + filesystem watcher | no-op `rescan` > 2 s, or `impact` p95 > 250 ms |
| LSP sidecars (`jdtls`, `tsserver`, `pyright`, `rust-analyzer`) | measured impact recall < 85 % for a language |
| Direct `GeminiProvider`, `LocalProvider` | user demand for headless non-Claude/OpenAI use |
| Monorepo sharding, per-module databases | full scan > 30 min, or CI write contention |
| CI mode: PR annotations, GitHub/GitLab output | adoption in pipelines |
| Team-shared bug database (server-backed store) | more than one developer maintaining the same findings |
| Cross-repo service graph | microservice estates where the seam crosses repositories |
| GraphQL and gRPC join tiers | a target codebase is GraphQL- or gRPC-first rather than REST |
| Archival of aged ledger rows | database growth becomes a real complaint |
| Additional languages (Go, C#, Kotlin, PHP) | user demand; each is one crate behind `LanguageAnalyzer` |
| HTTP/SSE MCP transport | a hosted or multi-client deployment |

The daemon is listed first because it is the most tempting thing to build early and the
easiest to regret — see [ADR-006](architecture-decisions.md#adr-006-stateless-processes-now-daemon-in-v2).
Its trigger is a number asserted in CI, not an intuition.

---

## Sequencing

```
  MVP                     V1                          V2
  ───────────────────     ─────────────────────       ──────────────────
  store + migrations      fingerprinting              daemon + watcher
  walk + hash + index     lifecycle + regressions     LSP sidecars
  java + ts analyzers     verification engine         sharding
  spring pack             sandbox                     team store
  impact engine           agent-as-provider           cross-repo graph
  cli + json              python + rust
  mcp read tools          full cli + mcp
  doctor                  integrations
  golden fixtures         redaction + audit
```

Read left to right: nothing in V1 requires rewriting anything in MVP, and nothing in V2
requires rewriting anything in V1. That is the return on the module boundaries in
[architecture.md](architecture.md) §4 — if a milestone here would force a rewrite of an
earlier one, the boundary was drawn in the wrong place and the design is wrong, not the plan.

---

## Explicit non-goals

Things BugHunter will not do, in any version, so the scope stays honest:

- **Fix bugs.** It finds and proves them. Applying a fix is the developer's or the agent's
  job, with BugHunter's evidence in hand. A tool that both diagnoses and treats has no
  independent check on itself.
- **Replace linters, type checkers or SAST.** It orchestrates them and reasons about what
  they cannot express.
- **Run arbitrary commands on request.** There is no such tool over MCP, and there will not
  be one. The allowlist is the whole surface.
- **Be a code review tool.** It analyzes changes against a baseline, not diffs against
  a reviewer's taste.
- **Send telemetry.** No usage reporting, no update checks, no crash reporting. Ever.
