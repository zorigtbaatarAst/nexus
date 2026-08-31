# Nexus — Architecture

> Status: design, pre-implementation. No code exists yet.
> Companion documents are listed in [Deliverable coverage](#8-deliverable-coverage) at the end.

## 1. What Nexus is

Nexus is a **platform for persistent code intelligence**. It reads a codebase once, stores
structured knowledge about it locally, and from then on works incrementally: it detects what
changed, computes the blast radius, and runs targeted analysis over the affected region only.

> **Nexus understands the project; capabilities use that understanding.**

Capabilities are what turn that understanding into findings. **BugHunter is the first**, and
the shape every other one takes: read the index, return findings, get identity, lifecycle and
history for free. See [capabilities.md](capabilities.md).

```
                        NEXUS
                          │
              ┌───────────┴───────────┐
              │                       │
       Project intelligence      Capabilities
              │                       │
              │              ┌────────┴────────┐
              │              │        │        │
              │          BugHunter  Review  Security
              │                     (later) (later)
              ├── code understanding      nexus-lang*
              ├── git and change          nexus-vcs, the tiered cascade
              ├── dependency and impact   symbol_edges, the weighted BFS
              ├── persistent knowledge    nexus-store, facts, findings
              └── agent context           nexus-mcp
```

Historically this was BugHunter, a **change-aware software intelligence system**. It reads a codebase once,
stores structured knowledge about it locally, and from then on works *incrementally*:
it detects what changed since the last scan, computes the blast radius of those changes,
looks for bugs in the affected region, and tries to **prove** each suspected bug by
generating and running a reproduction test.

It is not a linter, and it is not an AI wrapper.

### The one idea the whole design rests on

**Nexus owns evidence, history and verification. The AI agent owns reasoning.**

Everything else follows from that split:

- The agent never needs the repository — it needs *the right 4 KB of it*, plus what
  changed, plus what that change touches, plus what already broke here before. BugHunter's
  job is to produce exactly that.
- Because the intelligence is evidence-shaped rather than prompt-shaped, it is reusable by
  any agent — and by any capability. Claude Code, Codex, Copilot and a future local model all consume the same
  MCP tool surface.
- Because the evidence is deterministic, Nexus is still useful with the AI turned off
  entirely. `scan`, `rescan`, `changes`, `impact`, `analyze` and `ask` need no model and no
  API key.
- Because an agent can *record* a finding as well as read one, no model is foundational: any
  model is a provider, and Nexus contains no provider-specific code.
  [ADR-020](architecture-decisions.md#adr-020--llm-independence-is-the-write-back-path).

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
              ┌──────────────────┐        ┌──────────────┐
              │ nexus-mcp (thin) │        │  nexus-cli   │
              └────────┬─────────┘        └──────┬───────┘
                       └────────┬────────────────┘
                                ▼
                  ┌───────────────────────────┐
                  │  nexus-core  (Engine)     │  ← the platform
                  │  index · graph · change   │
                  │  impact · findings · facts│
                  │  capability registry      │
                  └───────────┬───────────────┘
            ┌───────────┬─────┴─────┬──────────────┐
            ▼           ▼           ▼              ▼
      ┌──────────┐ ┌──────────┐ ┌────────────┐ ┌──────────────┐
      │nexus-vcs │ │nexus-lang│ │nexus-store │ │ capabilities │
      │   git    │ │   AST    │ │  SQLite    │ │cap-bughunter │
      └──────────┘ └────┬─────┘ └────────────┘ │  (Review)    │
                        │                      │  (Security)  │
   nexus-lang-java · nexus-lang-ts             └──────────────┘
   nexus-lang-graphql                          registered, never
                                               compiled in
```

Read top to bottom: **agents → adapter → platform → understanding.** No arrow ever points
back up: `nexus-core` does not know that MCP exists, and it does not know that BugHunter
exists either. Capabilities are handed to it by the composition root, which is the only place
that knows both.

The pipeline through those layers:

```
Git analysis  ─┐
Code analysis ─┼──▶ Project understanding ──▶ Capability ──▶ Findings ──▶ Verification
Change/impact ─┘        (persistent)          (BugHunter…)   (lifecycle)   (later)
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
| `nexus-types` | IDs, enums, DTOs, error kinds. Serde only. | everything |
| `nexus-store` | SQLite access, migrations. **The only crate containing SQL.** | languages, capabilities, MCP |
| `nexus-vcs` | git2: HEAD, dirty state, diffs, blame, file history, detached worktrees | languages, capabilities, MCP |
| `nexus-lang` | `LanguageAnalyzer` + `FrameworkPack` traits, analyzer registry | any specific language |
| `nexus-lang-java` | tree-sitter-java, symbol/edge extraction, Spring pack | other languages, store |
| `nexus-lang-ts` | tree-sitter-typescript/tsx, `gql` documents, the frontend seam | other languages, store |
| `nexus-lang-graphql` | `.graphqls` schema — the contract both sides are generated from | other languages, store |
| `nexus-core` | **The platform.** Index, graph, change detection, impact, findings lifecycle, facts, the capability registry | adapters, capabilities, AI providers |
| `cap-bughunter` | **The first capability.** Deterministic rules over the index | adapters, store |
| `nexus-mcp` | rmcp server: schema in, `Engine` call, schema out | store, lang, capabilities' internals |
| `nexus-cli` | clap, renderers, composition root. Produces `nexus` and `bughunter` | — (it is the top) |

### 3.1 `nexus-core::Engine` — the single public API

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

    // capabilities and findings
    fn analyze(&mut self, capability: &str, scope: Scope) -> Result<AnalyzeReport>;
    fn capability_list(&self)                    -> Vec<CapabilityInfo>;
    fn record_finding(&mut self, cap: &str, f: Finding) -> Result<RecordedFinding>;
    fn findings(&self, cap: Option<&str>, status: Option<&str>, sev: Option<&str>)
                                                 -> Result<Vec<FindingSummary>>;
    fn finding(&self, uid: &str)                 -> Result<Option<FindingDetail>>;
    fn findings_for(&self, target: &str)         -> Result<Vec<FindingSummary>>;
    fn ignore_finding(&self, uid: &str)          -> Result<bool>;

    // memory
    fn record_fact(&mut self, f: FactInput)      -> Result<()>;
    fn facts(&self, subject: Option<&str>)       -> Result<Vec<Fact>>;
}
```

Several methods return a `Result<T>` whose `T` may be a `Clarification` rather than a
finished answer — `investigate` most often, but the variant is general. BugHunter refuses
ambiguity by describing what it needs; it never picks a candidate and sounds certain about
it. See [investigation.md](investigation.md) §7 and
[ADR-015](architecture-decisions.md#adr-015-structured-clarification-instead-of-guessing).

`Engine::new(project_root, policy, store, analyzers, ai)` is constructed at the composition
root — `nexus-cli::main` or `nexus-mcp::serve`. The `ai` argument is
`Arc<dyn AiProvider>` and defaults to `NullProvider`.

### 3.2 Inside `nexus-core`

| Module | Job |
|---|---|
| `detect` | language / framework / build system / package manager / DB / container detection |
| `walk` | ignore-aware filesystem traversal, parallel `blake3` hashing |
| `index` | file + symbol upsert, soft-delete, rename carry-over |
| `resolve` | edge resolution: exact → framework → heuristic → unresolved |
| `diff` | the tiered change-detection cascade |
| `impact` | weighted bidirectional BFS + test selection |
| `capability` | the `Capability` trait, `Scope`, and the registry |
| `project` | `ProjectContext` — the snapshot a capability reads |
| `findings` | identity, the lifecycle rules, the evidence requirement |
| `investigate` | symptom anchoring, cross-stack tracing, suspect ranking (designed, not built) |
| `clarify` | measures ambiguity and generates the questions that resolve it (designed, not built) |
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

1. **`nexus-core` must not depend on `nexus-mcp` or `nexus-cli`.**
   *Constraint 2: MCP is an adapter, not the core.*
2. **`nexus-core` must not depend on any concrete AI provider** — only on `nexus-ai` with
   `default-features = false`, which contains the trait and no HTTP client.
   *Constraints 1 and 3: core does not depend on Claude; AI is optional.*
3. **`nexus-mcp` must not depend on `nexus-store`, `nexus-lang*` or `nexus-verify`.** It can only reach
   them through `nexus-core`, so a handler physically cannot grow logic that the CLI lacks.
   *Constraint: "do not implement important functionality only inside MCP handlers."*
4. **Only `nexus-store` contains SQL.** Schema changes have exactly one blast radius.
5. **`nexus-lang-*` must not depend on `nexus-store` or `nexus-core`.** An analyzer takes source text
   and returns a `ParsedFile`; it never learns about scans or baselines.
   *Constraint 12: no language-specific logic in the core.*
6. **`nexus-verify` writes through `SafeWriter` only**, rooted at `.nexus/generated-tests/`.
   *Constraint 10: never modify production code during verification.*
7. **`cap-*` must not depend on `nexus-cli`, `nexus-mcp` or `nexus-store`.**
   *A capability that could reach a UI would drag one with it wherever it went — this is the
   concrete meaning of "BugHunter is usable independently".*
8. **`nexus-core` must not depend on any `cap-*`.**
   *Capabilities are registered into the platform, never compiled into it. The reverse
   dependency would make "add Code Review later" a core change.*

### On-disk layout

```
<repo>/.nexus/
├── config.toml          # committed — what to scan, language/framework overrides
├── policy.toml          # committed — permissions, sandbox, redaction rules
├── nexus.db         # local  — the knowledge store (SQLite, WAL)
├── nexus.db-wal
├── cache/               # local  — parse caches keyed by content hash
├── generated-tests/     # local  — the only path nexus-verify may write to
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
   create .nexus/, migrate DB

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
nexus/
├── Cargo.toml                      # workspace
├── Makefile                        # build · test · lint · install · smoke · demo
├── install.sh                      # installs both binaries
├── README.md  README.en.md  AGENTS.md  LICENSE
├── crates/
│   ├── nexus-types/                # vocabulary
│   ├── nexus-store/                # SQLite; the only crate with SQL
│   │   └── migrations/             # forward-only
│   ├── nexus-vcs/                  # git
│   ├── nexus-lang/                 # LanguageAnalyzer, FrameworkPack
│   ├── nexus-lang-{java,ts,graphql}/
│   ├── nexus-core/                 # the platform
│   │   ├── capability.rs           #   the trait, Scope, the registry
│   │   ├── project.rs              #   ProjectContext — what a capability reads
│   │   ├── findings.rs             #   identity and lifecycle
│   │   ├── engine.rs impact.rs walk.rs detect.rs report.rs
│   ├── cap-bughunter/              # the first capability
│   │   └── src/detectors/          #   spring · graphql · secrets
│   ├── nexus-mcp/                  # agent surface
│   └── nexus-cli/                  # produces `nexus` and `bughunter`
├── integrations/                   # config snippets and prompts — no logic
├── tests/fixtures/
└── docs/
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
| + | Capability contract | [capabilities.md](capabilities.md) |
| + | Platform shape and forbidden edges | [diagrams/nexus-platform.md](diagrams/nexus-platform.md) |
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
| 3 | AI optional for deterministic functionality | `NullProvider` default; `nexus-ai` default-features off |
| 4 | First scan creates the baseline | `baselines` pointer table written by `scan` |
| 5 | Rescans are incremental | Tiered cascade, [change-analysis.md](change-analysis.md) §2 |
| 6 | Memory persists locally | SQLite at `.nexus/nexus.db` |
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
