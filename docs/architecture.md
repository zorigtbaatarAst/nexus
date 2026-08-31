# BugHunter — Architecture

> Status: design, pre-implementation. No code exists yet.
> Companion documents are listed in [Deliverable coverage](#8-deliverable-coverage) at the end.

## 1. What BugHunter is

BugHunter is a **change-aware software intelligence system**. It reads a codebase once,
stores structured knowledge about it locally, and from then on works *incrementally*:
it detects what changed since the last scan, computes the blast radius of those changes,
looks for bugs in the affected region, and tries to **prove** each suspected bug by
generating and running a reproduction test.

It is not a linter, and it is not an AI wrapper.

### The one idea the whole design rests on

**BugHunter owns evidence, history and verification. The AI agent owns reasoning.**

Everything else follows from that split:

- The agent never needs the repository — it needs *the right 4 KB of it*, plus what
  changed, plus what that change touches, plus what already broke here before. BugHunter's
  job is to produce exactly that.
- Because the intelligence is evidence-shaped rather than prompt-shaped, it is reusable by
  any agent. Claude Code, Codex, Copilot and a future local model all consume the same
  MCP tool surface.
- Because the evidence is deterministic, BugHunter is still useful with the AI turned off
  entirely. `scan`, `rescan`, `changes`, `impact` and the deterministic detectors need no
  model and no API key.

### What it answers

After `bughunter init && bughunter scan`, the store can answer without an LLM:

```
What is this project?                     → project_profile
What are its major components?            → symbols where kind in (module, class, service)
How are components connected?             → symbol_edges
Which functions depend on this function?  → reverse BFS over symbol_edges
Which tests cover this function?          → test_coverage
What changed since a81f92c?               → changes for the scans between baselines
What broke here before?                   → bugs + bug_occurrences by component
```

---

## 2. High-level architecture

```
                AI Coding Agents
   Claude Code · Codex · Copilot · other MCP clients
                        │
                        │ MCP (stdio JSON-RPC)
                        ▼
              ┌──────────────────┐
              │  bh-mcp  (thin)  │        ┌──────────────┐
              └────────┬─────────┘        │   bh-cli     │
                       │                  └──────┬───────┘
                       └────────┬────────────────┘
                                ▼
                    ┌───────────────────────┐
                    │   bh-core  (Engine)   │  ← all business logic lives here
                    └───────────┬───────────┘
            ┌───────────┬───────┼────────┬────────────┐
            ▼           ▼       ▼        ▼            ▼
        ┌───────┐  ┌────────┐ ┌─────┐ ┌────────┐ ┌────────┐
        │bh-vcs │  │bh-lang │ │ bh- │ │ bh-ai  │ │bh-store│
        │ git   │  │  AST   │ │verify│ │(trait) │ │ SQLite │
        └───────┘  └───┬────┘ └─────┘ └────────┘ └────────┘
                       │
        bh-lang-java · bh-lang-ts · bh-lang-python · bh-lang-rust
```

Read top to bottom: **agents → adapter → core → capabilities → storage.** No arrow ever
points back up. `bh-core` does not know that MCP exists.

The pipeline through those layers:

```
Git analysis ─┐
Code analysis ─┼──▶ Project memory ──▶ Bug intelligence ──▶ Verification
Test analysis ─┘        (SQLite)         (fingerprints)      (run a test)
```

See [`diagrams/system-architecture.md`](diagrams/system-architecture.md) for the Mermaid
rendering of this, and `scan-flow` / `rescan-flow` / `bug-verification` / `mcp-integration`
for the four runtime flows.

---

## 3. Component architecture

BugHunter is a Rust Cargo workspace producing **one binary**, `bughunter`, which is both
the CLI and (via `bughunter mcp`) the MCP server. Rationale in
[ADR-001](architecture-decisions.md#adr-001-rust-for-bughunter-core).

| Crate | Responsibility | Must not know about |
|---|---|---|
| `bh-types` | IDs, enums, DTOs, error kinds. Serde only. | everything |
| `bh-store` | SQLite access, migrations. **The only crate containing SQL.** | languages, AI, MCP |
| `bh-vcs` | git2: HEAD, dirty state, diffs, blame, file history, detached worktrees | languages, AI, MCP |
| `bh-lang` | `LanguageAnalyzer` + `FrameworkPack` traits, analyzer registry | any specific language |
| `bh-lang-java` | tree-sitter-java, symbol/edge extraction, Spring framework pack | other languages, store |
| `bh-lang-ts` | tree-sitter-typescript/tsx, NestJS/Next.js packs | other languages, store |
| `bh-lang-python` | tree-sitter-python, Django/FastAPI packs | other languages, store |
| `bh-lang-rust` | tree-sitter-rust, axum/sqlx packs | other languages, store |
| `bh-verify` | reproduction planning, test emission, `SafeWriter`, sandbox, judgement | MCP, CLI |
| `bh-ai` | `AiProvider` trait, context budgeting, redaction. Providers behind features. | store, MCP, CLI |
| `bh-core` | `Engine` — the public API of BugHunter | MCP, CLI, concrete providers |
| `bh-mcp` | rmcp server: schema in, `Engine` call, schema out | store, lang, verify |
| `bh-cli` | clap, renderers, composition root | — (it is the top) |

### 3.1 `bh-core::Engine` — the single public API

Every CLI command and every MCP tool is one call into this facade. Nothing else is public.

```rust
impl Engine {
    // lifecycle
    fn init(&self, opts: InitOptions)            -> Result<ProjectProfile>;
    fn scan(&self, opts: ScanOptions)            -> Result<ScanHandle>;
    fn rescan(&self, opts: RescanOptions)        -> Result<ScanHandle>;
    fn scan_status(&self, id: ScanId)            -> Result<ScanReport>;
    fn status(&self)                             -> Result<ProjectStatus>;
    fn doctor(&self)                             -> Result<Vec<Check>>;

    // knowledge
    fn project_context(&self, q: ContextQuery)   -> Result<ProjectContext>;
    fn symbol(&self, sel: SymbolSelector)        -> Result<SymbolDetail>;
    fn changes(&self, q: ChangeQuery)            -> Result<Page<Change>>;
    fn impact(&self, q: ImpactQuery)             -> Result<ImpactReport>;
    fn tests_for(&self, sel: SymbolSelector)     -> Result<Page<TestRef>>;

    // symptom-driven investigation — the second entry point
    fn investigate(&self, r: SymptomReport)      -> Result<Investigation>;
    fn answer(&self, id: InvestigationId, a: Vec<Answer>) -> Result<Investigation>;

    // bugs
    fn find_bugs(&self, q: HuntQuery)            -> Result<HuntResult>;
    fn record_bug(&self, c: BugCandidate)        -> Result<BugRef>;   // agent writes back
    fn bug(&self, id: BugSelector)               -> Result<BugDetail>;
    fn bugs(&self, q: BugQuery)                  -> Result<Page<BugSummary>>;
    fn verify_bug(&self, id: BugSelector, o: VerifyOptions) -> Result<Verification>;
    fn bug_history(&self, id: BugSelector)       -> Result<Vec<BugEvent>>;
    fn regressions(&self, q: RegressionQuery)    -> Result<Page<BugSummary>>;

    // memory
    fn record_fact(&self, f: FactInput)          -> Result<FactRef>;
    fn facts(&self, q: FactQuery)                -> Result<Page<Fact>>;
}
```

Several methods return a `Result<T>` whose `T` may be a `Clarification` rather than a
finished answer — `investigate` most often, but the variant is general. BugHunter refuses
ambiguity by describing what it needs; it never picks a candidate and sounds certain about
it. See [investigation.md](investigation.md) §7 and
[ADR-015](architecture-decisions.md#adr-015-structured-clarification-instead-of-guessing).

`Engine::new(project_root, policy, store, analyzers, ai)` is constructed at the composition
root — `bh-cli::main` or `bh-mcp::serve`. The `ai` argument is
`Arc<dyn AiProvider>` and defaults to `NullProvider`.

### 3.2 Inside `bh-core`

| Module | Job |
|---|---|
| `detect` | language / framework / build system / package manager / DB / container detection |
| `walk` | ignore-aware filesystem traversal, parallel `blake3` hashing |
| `index` | file + symbol upsert, soft-delete, rename carry-over |
| `resolve` | edge resolution: exact → framework → heuristic → unresolved |
| `diff` | the tiered change-detection cascade |
| `impact` | weighted bidirectional BFS + test selection |
| `hunt` | detector orchestration; builds evidence bundles |
| `investigate` | symptom anchoring, cross-stack tracing, suspect ranking |
| `clarify` | measures ambiguity and generates the questions that resolve it |
| `fingerprint` | bug identity, alias resolution, near-duplicate linking |
| `lifecycle` | the bug status machine |
| `memory` | facts: record, supersede, retrieve by relevance |
| `context` | `ContextBuilder` — assembles a token-budgeted evidence bundle |
| `policy` | loads and enforces `policy.toml` |
| `audit` | append-only event log |

### 3.3 Language layer

```rust
pub trait LanguageAnalyzer: Send + Sync {
    fn language(&self) -> Language;
    fn extensions(&self) -> &[&str];
    fn grammar_version(&self) -> &str;                  // feeds cache invalidation

    fn parse(&self, src: &SourceFile) -> Result<ParsedFile>;   // symbols + raw edges
    fn normalize_body(&self, node: &Node, src: &str) -> String; // for body_hash
    fn signature_of(&self, sym: &RawSymbol) -> String;          // for sig_hash
    fn test_hints(&self, parsed: &ParsedFile) -> Vec<TestHint>;
}

pub trait FrameworkPack: Send + Sync {
    fn name(&self) -> &str;
    fn detect(&self, ctx: &DetectContext) -> Option<FrameworkMatch>;
    fn enrich(&self, parsed: &mut ParsedFile);          // routes, entities, beans
    fn expand_impact(&self, seed: &ImpactSeed, g: &GraphView) -> Vec<ImpactEdge>;
}
```

Framework packs are a **separate extension point** from language analyzers
([ADR-012](architecture-decisions.md#adr-012-framework-packs-as-a-separate-extension-point)).
Spring knowledge is not Java knowledge: you can have Java without Spring, and Spring-shaped
DI reasoning recurs in NestJS and in Python DI containers.

Resolution runs in tiers and every edge records which tier produced it:

| Tier | Mechanism | `symbol_edges.resolution` | Typical confidence |
|---|---|---|---|
| 0 | tree-sitter syntax, same file | `exact` | 1.00 |
| 1 | import table + FQN match across the index | `heuristic` | 0.70–0.95 |
| 2 | framework pack (Spring bean wiring, route tables, ORM) | `framework` | 0.80–0.95 |
| 3 | optional LSP sidecar (`jdtls`, `tsserver`, `pyright`, `rust-analyzer`) — V2 | `exact` | 1.00 |
| — | nothing matched; `dst_fqn_hint` retained for later | `unresolved` | 0.00 |

Tier 3 is optional by design: it is expensive and needs a full project import, so BugHunter
must be fully useful without it. See
[ADR-003](architecture-decisions.md#adr-003-tiered-tree-sitter-resolution-with-optional-lsp).

---

## 4. Module boundaries — the rules that are mechanically enforced

These are not conventions. A test walking `cargo metadata` fails the build if any is
violated, which is how the brief's constraints 1, 2 and 3 become structural facts rather
than good intentions.

1. **`bh-core` must not depend on `bh-mcp` or `bh-cli`.**
   *Constraint 2: MCP is an adapter, not the core.*
2. **`bh-core` must not depend on any concrete AI provider** — only on `bh-ai` with
   `default-features = false`, which contains the trait and no HTTP client.
   *Constraints 1 and 3: core does not depend on Claude; AI is optional.*
3. **`bh-mcp` must not depend on `bh-store`, `bh-lang*` or `bh-verify`.** It can only reach
   them through `bh-core`, so a handler physically cannot grow logic that the CLI lacks.
   *Constraint: "do not implement important functionality only inside MCP handlers."*
4. **Only `bh-store` contains SQL.** Schema changes have exactly one blast radius.
5. **`bh-lang-*` must not depend on `bh-store` or `bh-core`.** An analyzer takes source text
   and returns a `ParsedFile`; it never learns about scans or baselines.
   *Constraint 12: no language-specific logic in the core.*
6. **`bh-verify` writes through `SafeWriter` only**, rooted at `.bughunter/generated-tests/`.
   *Constraint 10: never modify production code during verification.*

### On-disk layout

```
<repo>/.bughunter/
├── config.toml          # committed — what to scan, language/framework overrides
├── policy.toml          # committed — permissions, sandbox, redaction rules
├── bughunter.db         # local  — the knowledge store (SQLite, WAL)
├── bughunter.db-wal
├── cache/               # local  — parse caches keyed by content hash
├── generated-tests/     # local  — the only path bh-verify may write to
│   └── BUG-104/
├── audit.log            # local  — append-only JSONL, every exec and AI call
└── .gitignore           # self-managing: ignores db, cache, audit, generated-tests
```

`config.toml` and `policy.toml` are **committed** — they are shared team intent, and a
teammate cloning the repo inherits the same scanning rules and the same execution
permissions. Everything else is local, disposable, and rebuildable from source plus git.

---

## 5. Runtime model

Stateless. Every CLI invocation and every MCP session opens the SQLite file, does its work,
and exits. There is no daemon, nothing to keep in sync, and no staleness class of bugs.

The `Engine` API is deliberately shaped so a V2 `bughunterd` — holding a warm symbol graph
and a filesystem watcher — can be introduced as a *transport* in front of the same methods
without any caller changing.
See [ADR-006](architecture-decisions.md#adr-006-stateless-processes-now-daemon-in-v2).

Concurrency safety with no daemon comes from SQLite: WAL mode, `busy_timeout=5000`, and a
scan-scoped advisory lock row so two simultaneous `rescan` invocations cannot interleave
writes. Readers (`impact`, `bugs`, `status`) never block.

---

## 6. Data flow, end to end

```
bughunter init
   detect() → project_profile          (language, framework, build, DB, containers)
   create .bughunter/, migrate DB

bughunter scan                          [full]
   walk() → hash all files
   parse() → symbols (sig_hash, body_hash)
   resolve() → symbol_edges
   discover tests → tests, test_coverage
   deterministic detectors → bugs (SUSPECTED/UNVERIFIED)
   write scan row + set baseline

bughunter rescan                        [incremental]
   Tier 0 → is anything different at all?
   Tier 1 → changed file set (stat fast path, then blake3)
   Tier 2 → changed symbol set (sig_hash vs body_hash vs annotations)
   Tier 3 → re-resolve affected edges
   impact() → affected symbols + related tests
   hunt() over the affected region only
   fingerprint() → new bug | same bug | regression
   write scan row + advance baseline

bughunter verify <id>
   plan → emit test → run now → run on baseline revision → judge

bughunter investigate                   [symptom-driven — the second entry point]
   SymptomReport from the agent's reading of a screenshot
   anchor() → route · visible text · network · console  →  candidate symbols
   if ambiguous → clarification_required, with what is already resolved
   trace() → forward across the calls_http seam into the backend
   rank()  → on_trace × recency × prior_bugs × coverage_gap × contract_penalty
```

At no point is the repository sent anywhere. The largest thing that ever leaves the machine
is a token-budgeted evidence bundle, and only if AI is enabled and a provider is configured
— which is not the default path
([ADR-005](architecture-decisions.md#adr-005-agent-as-ai-provider-by-default)).

---

## 7. Repository structure

```
bughunter/
├── Cargo.toml                      # workspace
├── Makefile                        # build · test · lint · install · fixtures
├── install.sh
├── README.md                       # Монгол
├── README.en.md                    # English
├── AGENTS.md                       # briefing for an AI agent working in this repo
├── LICENSE
├── crates/
│   ├── bh-types/
│   ├── bh-store/
│   │   └── migrations/             # 0001_init.sql, 0002_*.sql — forward only
│   ├── bh-vcs/
│   ├── bh-lang/
│   ├── bh-lang-java/
│   ├── bh-lang-ts/
│   ├── bh-lang-python/
│   ├── bh-lang-rust/
│   ├── bh-verify/
│   ├── bh-ai/
│   ├── bh-core/
│   ├── bh-mcp/
│   └── bh-cli/                     # produces the `bughunter` binary
├── integrations/
│   ├── claude-code/                # .mcp.json + 6 slash commands + skill
│   ├── codex/                      # config.toml snippet
│   └── copilot/                    # mcp.json snippet
├── tests/
│   ├── fixtures/                   # golden repos with planted bugs
│   │   ├── spring-payments/
│   │   ├── next-storefront/
│   │   ├── fastapi-orders/
│   │   └── cargo-ledger/
│   └── conformance/                # recorded MCP JSON-RPC sessions
└── docs/                           # this directory
```

---

## 8. Deliverable coverage

| # | Deliverable | Where |
|---|---|---|
| 1 | High-level architecture | this document, §2 |
| 2 | Detailed component architecture | this document, §3 |
| 3 | Module boundaries | this document, §4 |
| 4 | Data model | [data-model.md](data-model.md) §1–2 |
| 5 | SQLite schema | [data-model.md](data-model.md) §3–5 |
| 6 | Project-memory model | [memory-model.md](memory-model.md) |
| 7 | Change-detection algorithm | [change-analysis.md](change-analysis.md) §1–4 |
| 8 | Dependency/impact analysis | [change-analysis.md](change-analysis.md) §5–7 |
| 9 | Bug fingerprinting | [change-analysis.md](change-analysis.md) §8–10 |
| 10 | Verification engine | [verification-engine.md](verification-engine.md) |
| 11 | MCP server architecture | [mcp-api.md](mcp-api.md) |
| 12 | AI-provider abstraction | [ai-integration.md](ai-integration.md) |
| 13 | Claude Code integration | [ai-integration.md](ai-integration.md) §7 |
| 14 | CLI architecture | [cli-spec.md](cli-spec.md) |
| 15 | Security model | [security.md](security.md) |
| 16 | Performance / scaling | [performance.md](performance.md) |
| 17 | Error-handling strategy | [testing-strategy.md](testing-strategy.md) §1 |
| 18 | Testing strategy | [testing-strategy.md](testing-strategy.md) §2–7 |
| 19 | Mermaid diagrams | [diagrams/](diagrams/) |
| 20 | ADRs | [architecture-decisions.md](architecture-decisions.md) |
| 21 | Repository structure | this document, §7 |
| 22 | MVP → V1 → V2 roadmap | [roadmap.md](roadmap.md) |
| + | Symptom-driven investigation | [investigation.md](investigation.md) |
| + | Cross-stack tracing and contract mismatch | [investigation.md](investigation.md) §3, §6 |
| + | Clarification protocol | [investigation.md](investigation.md) §7 |

## 9. Constraint traceability

The brief's 15 hard constraints plus the two added later, each mapped to the mechanism
that enforces it.

| # | Constraint | Enforced by |
|---|---|---|
| 1 | Core must not depend on Claude | Boundary rule 2 + `cargo metadata` test |
| 2 | MCP is an adapter, not the core | Boundary rules 1 and 3 |
| 3 | AI optional for deterministic functionality | `NullProvider` default; `bh-ai` default-features off |
| 4 | First scan creates the baseline | `baselines` pointer table written by `scan` |
| 5 | Rescans are incremental | Tiered cascade, [change-analysis.md](change-analysis.md) §2 |
| 6 | Memory persists locally | SQLite at `.bughunter/bughunter.db` |
| 7 | Fingerprints prevent duplicates | `UNIQUE(project_id, fingerprint)` + alias resolution |
| 8 | AI findings verified where possible | [verification-engine.md](verification-engine.md) |
| 9 | Never send the whole repo to an LLM | `ContextBuilder` token budget; agent-as-provider default |
| 10 | Never modify production code while verifying | `SafeWriter` root jail, boundary rule 6 |
| 11 | Designed for Claude Code, Codex, Copilot, future clients | One MCP surface, no per-agent logic |
| 12 | Language analysis behind interfaces | `LanguageAnalyzer` / `FrameworkPack`, boundary rule 5 |
| 13 | SQLite as initial store | [ADR-002](architecture-decisions.md#adr-002-sqlite-as-the-knowledge-store) |
| 14 | Deterministic evidence over AI assumptions | `BugCandidate` without `CodeRef` evidence is rejected at the boundary |
| 15 | Professional DX, not a research prototype | [cli-spec.md](cli-spec.md), `doctor`, `--json`, exit codes |
| 16 | A screenshot plus a description finds bugs across front and back | [investigation.md](investigation.md); the `calls_http` seam |
| 17 | Ask when a task is incomplete or under-specified | `clarification_required`; never a silent guess |
