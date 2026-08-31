# BugHunter — Architecture Decision Records

Twelve decisions that would be expensive to reverse. Each states why it was needed, what else
was considered, what it costs, and — the part most ADRs omit — **the signal that should make
you change it**.

Status of all records: **Accepted (design phase, 2026-08-31)**. Nothing is implemented yet,
so every one of these is still cheap to revisit.

---

## ADR-001 — Rust for BugHunter Core

### Why it is needed
BugHunter must hash and parse millions of lines fast enough that a no-op rescan is
instantaneous, ship as something an MCP client can spawn without a runtime, and parse five
languages through one parser technology.

### Decision
Rust. A Cargo workspace producing a single static binary, `bughunter`, that is both the CLI
and the MCP server. `rusqlite`, native `tree-sitter`, `rayon`, `blake3`, `git2`, `clap`, `rmcp`.

### Alternatives considered

**Java 21 + GraalVM.** The strongest option on the primary target language: JavaParser and
Spoon give real Java and Spring semantics with no sidecar, and it is the language this
project's author writes best. Rejected because the *other* four languages have no credible
tree-sitter story on the JVM, native-image adds real build complexity, and a JVM start per
CLI invocation fights the sub-100 ms `rescan` budget that makes the tool pleasant.

**Go.** Single binary, simplest concurrency story, official MCP SDK, and a sibling tool
(`portman`) already uses it. Rejected because tree-sitter requires cgo, which forfeits
pure-Go cross-compilation, and the pure-Go parser alternatives are materially weaker.

**TypeScript/Node.** The most mature MCP SDK and the easiest distribution (`npx`), with
uniform WASM grammars for all five languages. Rejected on the scaling requirement: WASM
tree-sitter is roughly 5–8× slower than native, which converts the 8-minute 5 MLOC scan
into an hour.

**Python.** Excellent ecosystem, worst throughput, and a runtime dependency in a tool meant
to run inside other people's CI.

### Advantages
Single ~12 MB binary — `scp` it, or `command: "bughunter"` in any MCP config, with no
runtime. Native tree-sitter for all five languages through one API. `rayon` + `blake3` make
the hashing tier essentially free. No GC pauses in a latency-sensitive interactive path.
Consistent with `launchpad` and `pitcrew-cli`.

### Disadvantages
Slowest to write of the four. Async ergonomics in the MCP layer are more friction than in
Go or TypeScript. Deep Java semantics need a sidecar (ADR-003) that a JVM implementation
would get natively. The `rmcp` SDK is less mature than the TypeScript one.

### When to change it
If the Java/Spring analyzer plateaus at unacceptable precision *and* the LSP sidecar tier
proves too heavy in practice, extracting the Java analyzer into a JVM sidecar speaking a
small IPC protocol is the correct retreat — `bh-lang-java` becomes a client and nothing else
moves. That is a contained change precisely because of boundary rule 5.

---

## ADR-002 — SQLite as the knowledge store

### Why it is needed
Project memory must survive scans, sessions and machines; be queryable with joins, indexes
and transactions; and add zero operational burden to a developer tool.

### Decision
One SQLite file per project at `.nexus/nexus.db`, WAL mode. `bh-store` is the only
crate containing SQL.

### Alternatives considered

**An embedded graph database (CozoDB, SurrealDB embedded).** Attractive because the
dependency graph is the core data structure and recursive traversal is native. Rejected:
recursive CTEs plus a good index on `dst_symbol_id` cover the traversals actually performed,
and every one of these engines is a younger dependency with a smaller debugging community
than SQLite, in a tool whose whole value proposition is trustworthiness.

**JSON or Parquet files on disk.** No dependency, trivially inspectable, and hopeless the
moment two processes touch it or a query needs a join.

**A key-value store (sled, RocksDB).** Fast, and it would require hand-writing every index
and every join — reimplementing a relational engine, badly.

**A server database (Postgres).** Correct for a future team-shared deployment; absurd as a
prerequisite for `bughunter init` on a laptop.

### Advantages
Zero operations. Debuggable with `sqlite3` over SSH at 2 a.m., which for this author is a
decisive property. Real transactions, so a scan is atomic. Real indexes, so reverse traversal
is a seek. WAL lets reads proceed during a scan. Portable single file.

### Disadvantages
One writer. Deep graph traversal costs more than a native graph engine. Very large monorepos
will produce multi-gigabyte files. No built-in full-text or vector search (SQLite FTS5 covers
the former if needed).

### When to change it
Move to a server-backed store when a *team* shares one knowledge base rather than each
developer holding a local one, or when CI write contention becomes real. Add a graph engine
only if profiling shows traversal — not parsing — dominating, which
[performance.md](performance.md) §2 suggests it will not.

---

## ADR-003 — Tiered tree-sitter resolution with optional LSP

### Why it is needed
"Which functions depend on this function" needs type resolution. In Java with Spring it also
needs DI wiring: `@Autowired PaymentRepository` is an interface whose real implementation is
chosen at runtime. tree-sitter gives syntax, not types.

### Decision
Four tiers, with every edge recording which tier produced it and how confident it is:

| Tier | Mechanism | `resolution` | Confidence |
|---|---|---|---|
| 0 | tree-sitter, same file | `exact` | 1.00 |
| 1 | import table + FQN match across the index | `heuristic` | 0.70–0.95 |
| 2 | framework pack (bean wiring, routes, ORM) | `framework` | 0.80–0.95 |
| 3 | LSP sidecar — `jdtls`, `tsserver`, `pyright`, `rust-analyzer` (V2, optional) | `exact` | 1.00 |

### Alternatives considered

**LSP-only.** Highest precision. Rejected as a *requirement*: `jdtls` needs a full Gradle or
Maven import, takes 30+ seconds and gigabytes on first run, and fails outright on a project
that does not currently build. A bug-finding tool that only works on a healthy project is
useless exactly when it is needed.

**Compiler-based (javac plugin, tsc API, rustc).** Perfect resolution, and one bespoke
integration per language with a per-version maintenance burden, plus the same "must build"
constraint.

**tree-sitter only.** Fast and universal, and it cannot resolve an interface to its
implementation — which is most of what matters in a Spring codebase.

### Advantages
Always works, including on a project that does not compile. Degrades gracefully rather than
failing. Confidence is *reported*, so a three-hop heuristic chain is visibly a guess rather
than silently equal to a compiler fact. Precision is opt-in where it is worth the cost.

### Disadvantages
Tier 1 will produce false edges on heavy overloading, reflection and dynamic dispatch.
Recall on dynamic Python and TypeScript is materially worse than a real type checker's. Two
resolution paths mean two sets of bugs.

### When to change it
Promote LSP from optional to default per-language once measured impact recall on the golden
fixtures falls below roughly 85 % for that language, and the sidecar's startup cost can be
amortized — which realistically means once the V2 daemon exists to keep it warm.

---

## ADR-004 — MCP as the primary integration surface

### Why it is needed
The brief requires Claude Code, Codex, Copilot and future agents to use BugHunter with no
per-agent implementation.

### Decision
One MCP server over stdio (`bughunter mcp`), a thin adapter over `Engine`. Per-agent
directories contain configuration snippets and prompt text only, never logic.

### Alternatives considered

**A plugin per agent.** Best-integrated UX per agent, and N implementations to keep in step —
exactly the outcome the brief forbids.

**A local HTTP API.** Language-agnostic and universal, but every agent then needs a custom
client, plus port management, lifecycle and an auth surface where stdio has none.

**A CLI that agents shell out to.** Works today with any agent, and gives up structured
schemas, typed errors, pagination and progress. Retained as a *secondary* path — the CLI is
first-class for humans and CI regardless.

### Advantages
One implementation for every current and future MCP client. Schemas and typed errors are part
of the protocol. Stdio needs no ports, no auth and no daemon. New agent supporting MCP
means zero work.

### Disadvantages
Bound to MCP's evolution. Stdio is one client per process, so no shared warm cache. Agents
that do not speak MCP fall back to the CLI.

### When to change it
Add an HTTP/SSE transport when a *hosted* or multi-client deployment appears — a shared team
server, or CI agents talking to one BugHunter. That is a transport addition inside `bh-mcp`,
not an architectural change, which is the point of keeping the adapter thin.

---

## ADR-005 — Agent-as-AI-provider by default

### Why it is needed
BugHunter needs AI reasoning for the bug classes deterministic tools cannot express, without
requiring API keys, incurring cost, or coupling to a vendor.

### Decision
Under MCP — the primary path — BugHunter calls **no model**. It returns a structured evidence
bundle; the calling agent reasons and writes findings back via `bughunter_record_bug`. Direct
providers exist behind cargo features for headless CLI and CI use, and are opt-in.

### Alternatives considered

**BugHunter owns an API key and calls the model itself.** The obvious design, and it means
every user configures billing for a second model when they are already paying for the agent
in front of them, tokens are spent twice on the same reasoning, and the tool inherits a
vendor relationship. Kept as the opt-in path for headless use, where there is no agent.

**Bundled local model.** Removes the key and adds hundreds of megabytes, GPU expectations and
materially worse reasoning. `LocalProvider` supports it for those who want it; bundling it is
not the default.

**No AI at all.** Deterministic analysis alone is a real product — and it cannot find a
business-logic error, which is where the value is.

### Advantages
Zero API keys, zero cost, zero vendor coupling on the default path. The user's own agent —
whose context window and billing already exist — does the reasoning. Identical behaviour
across Claude Code, Codex and Copilot. Nothing leaves the BugHunter process. It makes
constraints 1 and 3 nearly free rather than a constant effort.

### Disadvantages
Reasoning quality varies with whichever agent is connected, and BugHunter cannot control or
reproduce it. Headless CI needs the opt-in provider path, so both paths must be maintained.
The agent may ignore the evidence bundle and do something else — mitigated by rejecting
candidates without verifiable evidence.

### When to change it
If measurement shows agent-produced findings are materially worse than a directly-prompted
model — most likely because agents skim the bundle — move to a hybrid where BugHunter runs
a small, tightly-prompted direct call for triage and hands only the survivors to the agent.

---

## ADR-006 — Stateless processes now, daemon in V2

### Why it is needed
A warm in-memory graph would make interactive impact queries faster on a monorepo. It would
also add a lifecycle, a staleness problem and an IPC failure mode.

### Decision
Every CLI invocation and MCP session opens SQLite, works, exits. `Engine` is shaped so a V2
`bughunterd` can wrap the same methods as a transport, with no caller changing.

### Alternatives considered

**Daemon from day one.** Fastest repeat queries, and it front-loads every hard problem —
lifecycle, restart, filesystem watching, cache invalidation, IPC — before there is evidence
any of it is needed. `pitcrew`'s "there is nothing to keep in sync because nothing is kept"
is the principle being borrowed.

**Never a daemon.** Simplest possible operational story, and it puts a hard floor under
interactive latency on very large repositories.

### Advantages
No staleness class of bugs at all — the database is the only state, and SQLite already
handles concurrent access. Nothing to start, restart, supervise or debug. A crash loses
nothing. Trivially usable from CI, cron, a script or an agent.

### Disadvantages
Every process rebuilds its in-memory FQN map. Repeat `impact` queries pay SQLite open and
page-cache warm-up each time. No filesystem watcher, so a no-op `rescan` still costs a
`git status`.

### When to change it
When the no-op `rescan` budget (2 s at 5 MLOC) or the `impact` p95 budget (250 ms) is
missed on a real repository. Both are asserted in CI precisely so the trigger is a
measurement rather than an opinion.

---

## ADR-007 — Composite hash fingerprint for bug identity

### Why it is needed
Constraint 7: the same bug seen in a later scan must be recognized, not re-reported. Identity
must survive reformatting, line drift, parameter renames and file moves, while distinguishing
two genuinely different bugs in the same class.

### Decision
`blake3(bug_type ‖ component ‖ anchor_symbol_fqn_shape ‖ detector_family ‖ structural_key)`,
truncated to 128 bits, unique per project. Path, line numbers, commit and wording are
excluded. A human-readable `slug` is stored separately for display.

### Alternatives considered

**File path + line number.** Trivial, and invalidated by adding an import above the bug.

**A hash of the code snippet.** Survives line drift, and changes on any reformat or rename
inside the method — the two most common no-op edits.

**Let the AI assign an identifier.** Non-deterministic across runs and providers. It would
make bug identity a property of a model's mood.

**Semantic similarity over embeddings.** Handles rephrasing well and gives probabilistic
identity, a vector index, and no explanation for why two findings merged. Retained only as
the near-duplicate *hint* (§9 of [change-analysis.md](change-analysis.md)), never as identity.

### Advantages
Deterministic, cheap, explainable — you can point at each input and say why two findings are
or are not the same. Stable across formatting, renames and moves via `symbol_aliases`.
Enforced by `UNIQUE(project_id, fingerprint)`, so deduplication is a database guarantee
rather than application discipline.

### Disadvantages
A method genuinely moving to another class produces a new fingerprint and looks like a new
bug — arguably correct, occasionally annoying. `structural_key` quality is the detector's
responsibility, so a sloppy detector produces sloppy identity. Two truly distinct bugs with
identical structural keys would collide.

### When to change it
If fixture measurement shows either a duplicate rate above ~5 % or a merge of genuinely
distinct findings, revise `structural_key` per detector first — it is the tunable input.
Change the hash composition only if that fails.

---

## ADR-008 — Verification by generated tests, with a baseline-revision run

### Why it is needed
The difference between "an AI thinks there might be a bug" and "here is the bug happening."
Everything else in the product is in service of this.

### Decision
Generate a reproduction test into an isolated directory, run it on the current revision, run
**the same test on the baseline revision** in a detached read-only git worktree, and judge
from the pair.

### Alternatives considered

**Run the existing test suite and look for new failures.** Cheap, and it only finds bugs the
suite already covers — which by definition are not the interesting ones.

**Static proof / symbolic execution.** Sound and beautiful; does not scale to a Spring
application, and cannot express most of the properties in question.

**Run the generated test on the current revision only.** Half the cost, and it cannot
distinguish "this change introduced a bug" from "this suite was already red". That
distinction is the difference between an actionable report and a wild goose chase — and
between an honest 97 % and a fabricated one.

**Ask the AI whether the bug is real.** Asking the same system that produced a guess to grade
its own guess. Explicitly rejected: constraint 14.

### Advantages
Verified bugs come with a runnable reproduction, which is the single most useful artifact a
bug report can carry. Regression versus pre-existing is classified automatically. Confidence
becomes evidence-backed rather than model-asserted. The test can be promoted into the real
suite with one command.

### Disadvantages
Requires a working build and test infrastructure. Slow — minutes per verification. The
baseline run doubles the cost. Generated tests can be wrong, and a wrong test that fails
looks like a reproduction (mitigated by requiring `expected_failure` to match, not just a
non-zero exit). Some bug classes are not testable at all in a reasonable amount of time.

### When to change it
Add lighter verification tiers — assertion injection, targeted property tests, a runtime
invariant check — if the full generate-and-run cycle proves too slow to be used routinely.
The judgement matrix stays; only the evidence-production step changes.

---

## ADR-009 — Docker-preferred sandbox with explicit host opt-in

### Why it is needed
Verification executes code that a model wrote, against a repository BugHunter does not own,
using build tools with arbitrary plugins.

### Decision
`policy.execute` ∈ `{docker, host, none}`, defaulting to `none`. Docker mounts the repository
read-only with a writable overlay for generated tests, `--network=none`, resource caps, a
non-root user, and a wall-clock timeout. Host execution is permitted only by a committed
policy change and is always audit-logged.

### Alternatives considered

**Container required, always.** Maximum safety and reproducibility, and it blocks
verification entirely for testcontainers-based suites, GPU tests, licensed toolchains and
every machine without a Docker daemon — which is most CI runners on some platforms. It would
make the flagship feature unavailable to a large fraction of real projects.

**Host only, with an allowlist.** Simplest and fastest, with no isolation from a generated
test that decides to write files.

**Generate but never execute.** Zero risk, and it makes `VERIFIED` unreachable — deleting
the product's main claim.

**A VM or microVM (Firecracker).** Stronger isolation, far heavier, and a hard dependency
most developers do not have.

### Advantages
Safe by default and honest about the trade-off. Reproducible runs when containerized.
Available to projects that genuinely cannot containerize, without silently pretending they
are sandboxed — `test_runs.sandbox` records which was used, and the report shows it.

### Disadvantages
Two execution paths to maintain and test. Containerized builds are slower without a warm
cache. Host mode's isolation rests on the allowlist and the `SafeWriter` jail alone. Docker
availability is one more thing `doctor` must diagnose.

### When to change it
If a real escape or a damaging incident occurs in host mode, remove it and require
containers. If containerization proves impractical for most target projects, invest in a
lighter isolation primitive (`bubblewrap`, `nsjail`) as a middle tier rather than widening
host mode.

---

## ADR-010 — Two hashes per symbol

### Why it is needed
With one hash per symbol, every edit is an "it changed" and must ripple to the entire
reverse-reachable set. On a real codebase that is hundreds of symbols for a one-line change,
which makes impact analysis noise and makes minimal AI context impossible.

### Decision
`sig_hash` over signature and annotations; `body_hash` over the normalized body. The change
kind derived from which one moved selects which edge types the ripple follows.

### Alternatives considered

**One hash per symbol.** Simpler, and it destroys precision as described above.

**A full AST diff per symbol.** Maximum precision — which statement changed, and whether it
touched shared state — at a much higher cost, with per-language complexity in the core.
A plausible future refinement, not a starting point.

**File-level hashing only.** Cheapest, and it hands an LLM the whole class for a one-method
edit, violating constraint 9 by construction.

### Advantages
A body-only edit ripples only through data and effect edges, typically a handful of symbols
instead of hundreds. Annotation changes get their own kind (`CONTRACT_CHANGED`), which
matters enormously in Spring where `@Transactional` carries more meaning than most
signatures. Reformatting produces zero symbol changes, so `spotlessApply` does not trigger a
bug hunt. Two 16-byte columns.

### Disadvantages
Two hashes to compute and store. `normalize_body` is per-language and is the piece most
likely to have subtle bugs — a normalizer that strips too much hides real changes, which is
the worst failure mode in the system. Guarded by the fixture assertion that a reformat
commit produces exactly zero symbol changes and a literal change always produces one.

### When to change it
Add a third, finer signal — a statement-level or effect-level hash — if measurement shows
`BODY_CHANGED` ripples are still too wide on real repositories. The schema accommodates it
without migration pain because edges are derived.

---

## ADR-011 — Immutable evidence ledger versus mutable current state

### Why it is needed
"BUG-104 was introduced in a81f92c, fixed in c72aa11, and regressed in f0091ab" is only
sayable if the sightings at each of those scans still exist, unedited.

### Decision
Three table classes, documented in [data-model.md](data-model.md) §2. Ledger tables
(`scans`, `changes`, `bug_occurrences`, `bug_verifications`, `test_runs`, `audit_events`) are
append-only. Current-state tables are upserted and soft-deleted, never hard-deleted. Derived
tables (`symbol_edges`, `test_coverage`) are droppable. `facts` are superseded, never edited.

### Alternatives considered

**Mutate everything in place.** Smallest database, simplest writes, and no history — which
removes regression detection, the strongest thing the product does.

**Event sourcing throughout.** Perfect history, and every read becomes a fold over events,
requiring projections and a rebuild path. Far more machinery than a developer tool needs.

**Full temporal tables (validity intervals on every row).** Complete and correct; makes every
query carry an as-of clause, for history that is only needed on a few entities.

### Advantages
History is a fact on disk, not a reconstruction. Regression detection is a query. Audit is
inherent. Derived tables can be dropped and rebuilt after an analyzer upgrade with no data
migration at all. The classification tells every future contributor which tables they may
`UPDATE` — the rule is legible in the schema.

### Disadvantages
The database only grows; retention must be explicit (`bughunter prune`). Soft-deletes mean
almost every query needs `WHERE deleted = 0` and forgetting it is a silent bug — mitigated
by exposing only filtered views from `bh-store`. Three classes is more to explain than one.

### When to change it
If ledger growth becomes a genuine problem on a busy monorepo, add archival — move ledger
rows older than the retention window into a compressed sidecar database that
`bughunter history --archive` can read. Do not start deleting evidence.

---

## ADR-012 — Framework packs as a separate extension point

### Why it is needed
Understanding a Spring application means understanding bean wiring, transaction boundaries,
route tables and derived queries. None of that is Java knowledge, and all of it is essential
to correct impact analysis on the primary target.

### Decision
`FrameworkPack` is a trait distinct from `LanguageAnalyzer`. A pack detects itself, enriches
parsed files with framework semantics, and contributes impact expansion. Edges it creates are
marked `resolution = 'framework'`.

### Alternatives considered

**Put Spring knowledge inside `bh-lang-java`.** Fewer moving parts, and it conflates two
independent axes: there is Java without Spring, and Spring-shaped DI reasoning recurs in
NestJS and in Python DI containers. It also drifts toward the hard-coded Java-specific logic
constraint 12 forbids.

**A generic annotation-driven rule engine.** Configurable, no code per framework, and unable
to express the interesting relationships — Spring Data deriving a query from a method name is
not a pattern match.

**No framework awareness.** Honest and much less useful: the call graph of a Spring app is
mostly interfaces, and a tool that cannot resolve `@Autowired PaymentRepository` to its
implementation cannot answer the question users actually ask.

### Advantages
Spring, NestJS, Django and axum packs evolve independently of their language analyzers.
Framework-derived edges are visibly labelled, so their confidence can be judged separately.
A new framework is a new pack, touching nothing else. Keeps `bh-core` free of framework
knowledge, satisfying constraint 12 on both axes.

### Disadvantages
A second extension point to design, document and version. Packs will lag framework releases.
Some knowledge genuinely straddles the two — Java records and Lombok are language-ish but
behave like framework magic — and the boundary will occasionally be arbitrary.

### When to change it
If packs end up as thin annotation lookups in practice, collapse them into the analyzers. If
instead they grow to carry real semantics — full Spring context resolution, ORM query
analysis — promote them to first-class plugins with their own versioning, and consider
loading them dynamically so a pack can ship without a BugHunter release.

---

## ADR-013 — Symptom-driven investigation as a second entry point

### Why it is needed
Everything designed so far starts from a change: *what did this commit break*. That is not
how bugs arrive. They arrive as a person pointing at a screen saying "this number is wrong",
often long after the commit that caused it, and frequently with no idea which side of the
stack is at fault.

### Decision
A second entry point, `investigate`, seeded by a `SymptomReport` — a description plus
observations the agent read off a screenshot — which is anchored to symbols deterministically,
traced across the frontend/backend seam, and ranked into suspects. It converges with the
change-driven path: same fingerprinting, same lifecycle, same verification.

### Alternatives considered

**Leave it to the agent.** An agent with file access can already grep for a label and read
components. Rejected because it cannot cross the seam: nothing in the source text connects
`fetch('/api/cart')` in one repository to `@GetMapping` in another, and reconstructing that
by reading files costs an enormous amount of context to get a graph BugHunter already has.

**A separate tool.** Clean, and it would duplicate the index, the fingerprints and the
verification engine — and produce a second bug database that disagrees with the first.

**Extend `impact` with a UI target.** Tempting, and wrong: `impact` answers "what does
changing X affect", which is forward reasoning from a known symbol. Investigation is
*backward* reasoning from an unknown one, and the hard part is the anchoring, which `impact`
has no concept of.

### Advantages
Uses the index the product already builds, so the marginal cost is one table and one edge
type. Reaches bugs the change-driven path structurally cannot — anything older than the
baseline, or introduced by data rather than code. Makes the whole index pay off for the
person who has a screenshot and no commit to blame. Converges into the existing lifecycle
rather than forking it.

### Disadvantages
Anchoring is heuristic and will sometimes point at the wrong component — mitigated by the
clarification protocol rather than by pretending to precision. Requires frontend framework
packs, which are a real body of work. The trace is only as good as the seam resolution, which
degrades on dynamically constructed URLs. A second entry point is a second set of failure
modes to explain.

### When to change it
If anchoring accuracy on the golden fixtures cannot be pushed past roughly 70 % without a
clarifying question, invert the interaction: lead with the questions instead of attempting
an anchor first. If cross-repo estates dominate, the seam becomes a service-graph problem and
this merges into the V2 cross-repo work.

---

## ADR-014 — Join the stack at the HTTP contract

### Why it is needed
The dependency graph stops at every language boundary. A TypeScript `fetch()` and a Java
`@PostMapping` are unrelated symbols, so no traversal can get from a UI symptom to a
repository method. Something has to join them.

### Decision
Join at the HTTP contract. Both framework packs already extract route data; a resolution tier
canonicalizes both sides to `METHOD /path/:p` and emits a `calls_http` edge with
`resolution = 'contract'`. A backend route is already a symbol with `kind='route'`, so the
existing unresolved-edge sweep does the matching with no new tables.

### Alternatives considered

**A shared schema or IDL as the source of truth.** Correct where one exists, and most
codebases do not have one, or have one that has drifted. OpenAPI is therefore used as a
*third source of evidence* — and a spec disagreeing with its handler is itself a finding —
never as the join mechanism.

**Runtime tracing / OpenTelemetry.** Perfectly accurate, and it requires the system to be
running, instrumented and reproducing the bug. A static tool that only works on a live
system is not a static tool.

**Name-based heuristics** (`CartService` ↔ `useCart`). Cheap, and wrong often enough to be
worse than nothing, since a false seam edge produces a confidently wrong trace.

**Do not cross the seam; report the two sides separately.** Honest, and it abandons the
question the user actually asked, which is *why is this number wrong* — an answer that stops
at the network boundary is not an answer.

### Advantages
Works on any codebase that speaks HTTP, with no instrumentation and nothing running. Reuses
the existing `kind='route'` symbol, `dst_fqn_hint` and the Tier-3 sweep, so the marginal
schema cost is zero. Unlocks the contract-mismatch detector, which finds a large and very
common bug class with no model at all. Path canonicalization is positional, so the two sides
need not agree on parameter names.

### Disadvantages
Dynamically constructed URLs (`fetch(base + path)` where `path` is computed) will not
resolve. Gateway rewrites must be configured in `config.toml` or the join silently misses —
mitigated by reporting unmatched calls rather than dropping them. GraphQL, gRPC and message
queues are not covered by this mechanism at all.

### When to change it
Add per-protocol join tiers when a target codebase is GraphQL-first (join on operation name
and selection set) or gRPC-first (join on the `.proto` service and method — considerably
easier, since there genuinely is a shared IDL). The `calls_http` edge type becomes
`calls_rpc` alongside it; nothing else moves.

### Revision — 2026-08-31: the trigger fired immediately

The first real target codebase (`autoland-management/sales`) is Spring for GraphQL on the
backend and Apollo with `graphql-codegen` on the frontend: **27 GraphQL controllers against
3 REST files.** The decision above still stands — join at the contract, statically, with
nothing running — but for this stack the contract is a schema coordinate, not a URL path.

What was added, as `EdgeType::CallsGraphql` alongside `CallsHttp`:

| Side | Extracted | Emits |
|---|---|---|
| backend | `@QueryMapping` / `@MutationMapping` / `@SchemaMapping` | a `kind='route'` symbol `graphql:Query.vehicles`, and a `routes` edge to its handler |
| frontend | `gql` documents | a `graphql:op:Vehicles` symbol, and a `calls_graphql` edge per root field |
| frontend | `useQuery(VehiclesDocument)` | a `calls_graphql` edge to `graphql:op:Vehicles` |

Two hops rather than one, because a component names an *operation* while a resolver serves a
*field*, and neither file mentions the other. The `<Name>Document` suffix is graphql-codegen
output, which is what makes the first hop a contract rather than a naming guess.

This turned out **better** than the HTTP join, and the ADR's reasoning was wrong on one
point: it argued that most codebases have no shared IDL, so the join could not rely on one.
A GraphQL project does have one — the `.graphqls` schema — and both sides are generated from
it. The join is exact, and `resolution='contract'` records that.

Measured on the real project: 402 contract edges, and a reverse trace from
`VehicleService#list` reaches six React components through the seam.

---

## ADR-015 — Structured clarification instead of guessing

### Why it is needed
A symptom report is often under-specified. Four components on a route render a total; the
description does not say which. BugHunter must do something, and picking one silently is the
worst available option — it produces a confident wrong answer that nobody can identify as a
guess.

### Decision
Any tool may return `clarification_required` instead of a result: what is already resolved,
concrete questions each carrying a `why` and file-path options, and `can_proceed_without`
with the confidence that proceeding would yield. Answers resume the investigation by id.

### Alternatives considered

**Guess the most likely candidate and report low confidence.** Standard practice, and it
relies on a human noticing a number. In practice the headline is read and the confidence is
not, so a 0.35 guess gets acted on as a finding.

**Return an error.** Honest and useless: it discards the anchoring work already done and
tells the caller nothing about what would help.

**Return all candidates and let the agent choose.** Reasonable, and it makes the agent guess
instead — with less information than BugHunter has, since BugHunter measured the ambiguity.
Retained as the `can_proceed_without: true` path, where the caller may explicitly accept the
lower confidence.

**Free-text "please clarify".** The agent must then invent what to ask, and the answer comes
back in a shape nothing can consume.

### Advantages
Ambiguity is refused rather than guessed at. The `why` on each question means the human
learns what evidence matters — a Network tab status, say — and supplies it unprompted next
time. Resolved state is returned with the question, so no work is thrown away.
`can_proceed_without` keeps soft ambiguity from becoming a hard block. It is the same
structured-refusal shape as `permission_required`, so it is one mechanism to learn.

### Disadvantages
Questions generated from a template rather than from measured ambiguity would train people to
ignore them — the rule against that is a rule, and rules erode. A round trip costs latency.
Non-interactive callers (CI) need a policy for what to do with a question, which is
`--answers` or accept the degraded confidence.

### When to change it
If telemetry-free observation shows users habitually accepting `can_proceed_without` rather
than answering, the questions are not earning their round trip: either the anchoring must
improve or the questions must get sharper. If a question is answered the same way nine times
out of ten, it should become a configuration default instead.

---

## ADR-016 — The agent reads the image; BugHunter never receives it

### Why it is needed
The investigation entry point starts from a screenshot. Something must turn pixels into
observations, and where that happens determines what BugHunter becomes.

### Decision
The calling agent reads the image and passes a structured `SymptomReport`. BugHunter accepts
an optional path and hash for the screenshot purely as provenance on the bug record, and
never opens it.

### Alternatives considered

**Embed a vision model.** Hundreds of megabytes, a GPU expectation, and a hard dependency in
a tool whose distribution story is "one 12 MB static binary you can scp".

**Bundle an OCR engine (Tesseract or similar).** Lighter, and it reads text without
understanding a screen — it cannot tell that the number next to the total is the wrong one,
which is the entire content of the user's complaint.

**Accept the image and forward it to a configured AI provider.** Coherent with the direct
provider path, and it makes BugHunter responsible for transmitting a screenshot that may
contain customer data, production identifiers or credentials, from a component whose redaction
pass works on text.

### Advantages
Consistent with [ADR-005](#adr-005-agent-as-ai-provider-by-default): the reasoning lives with
the agent that already has a model and a paying user. Keeps the binary small and the
dependency tree clean. Keeps a whole class of sensitive data out of BugHunter's
responsibility. Works identically with any agent that can read an image, and degrades
gracefully to a text-only report for one that cannot — a description plus a route is still a
usable seed.

### Disadvantages
Observation quality varies by agent, and BugHunter cannot verify that the reported visible
text actually appears in the screenshot. The CLI has no image path at all, so a terminal user
must type observations themselves. A mis-transcribed label produces a confidently wrong
anchor — mitigated by requiring anchors to converge before proceeding without a question.

### When to change it
If agents prove unreliable at transcription in practice, add a *verification* step rather
than a vision stack: ask the agent for the label's bounding box or surrounding text and check
that the combination exists in `ui_strings`. That keeps the vision outside and adds a
deterministic cross-check inside, which is the shape the rest of the product already uses.


---

## ADR-017 — `external` is a resolution outcome, not a failure

### Why it is needed
The first real measurement of edge resolution reported **20 % resolved** and looked like a
broken analyzer. It was not: 9,514 of 13,467 edges pointed at `org.springframework`,
`org.mockito` and sibling Gradle modules that were never scanned. Those edges are *correctly*
absent from the index. A denominator that includes them measures the size of the JDK, not
the quality of resolution.

### Decision
A fifth resolution value, `external`, for an edge whose target lies outside every package the
project defines. Resolution rate is reported over in-project edges only, with the external
count shown alongside.

```
edges  13467  (96% of 4370 in-project resolved, 9514 external)
```

### Alternatives considered

**Do not emit the edge at all.** Cheapest, and it discards real information: "this service
calls Spring's `StringUtils`" matters for dependency-upgrade impact, and once a sibling
module *is* scanned those edges resolve with no re-extraction.

**Count them as unresolved.** What was happening. It makes the headline metric meaningless
and, worse, hides genuine resolution bugs inside a large constant — the record-accessor and
static-import bugs were both invisible at 20 % and obvious at 68 %.

**Maintain a hard-coded list of external packages.** Brittle, and wrong for a monorepo where
`mn.autoland.model` is external today and internal tomorrow. Deriving project packages from
the indexed symbols themselves needs no configuration and self-corrects.

### Advantages
The metric means something, so it can be used as a regression signal. Diagnosing resolution
becomes possible: `bughunter graph` breaks the count down by tier, which is how the two bugs
above were found. External edges keep their hint, so widening the scan resolves them later
with no re-parse.

### Disadvantages
"Project package" is inferred, so a project whose own code lives under a package it shares
with a library would misclassify. A file in a package with no indexed types cannot be
classified and falls back to unresolved. Two numbers to explain instead of one.

### When to change it
If monorepo users routinely scan one module at a time, promote external edges to a
first-class *cross-module* view rather than a footnote — at that point the interesting
question becomes which unscanned modules this one depends on, which is exactly the V2
cross-repo service graph.