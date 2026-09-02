# Current state

Assessment of the Nexus codebase as of 2026-09-02, at commit `ff91b2f`. Everything here was
measured, not inferred.

**Method.** Direct reading of all 13 crates (16,600 lines of Rust); the SQLite schema across
five migrations; the live self-index at `.nexus/nexus.db`; and a structural graph built with
`graphify update crates --no-cluster` (1,056 nodes, 2,908 edges) because Nexus cannot index
Rust and therefore cannot index itself. That command reproduces the graph in seconds; the
output is not committed.

---

## 1. What is there

| Crate | Lines | Role |
|---|---:|---|
| `nexus-types` | 647 | shared ids/enums/DTOs |
| `nexus-store` | 2,419 | SQLite; the only SQL in the workspace |
| `nexus-vcs` | 156 | git2: HEAD, dirty, changed paths |
| `nexus-lang` | 204 | `LanguageAnalyzer` trait + registry |
| `nexus-lang-java` | 1,730 | tree-sitter-java + Spring pack |
| `nexus-lang-ts` | 812 | TypeScript/TSX + GraphQL operations |
| `nexus-lang-graphql` | 261 | `.graphqls` schema indexing |
| `nexus-core` | 4,519 | the Engine: index, graph, change detection, impact, findings |
| `cap-bughunter` | 1,304 | 4 deterministic detectors |
| `cap-architect` | 729 | 3 advisory rules |
| `cap-review` | 791 | 3 change-safety rules |
| `nexus-mcp` | 748 | rmcp adapter, 16 tools |
| `nexus-cli` | 2,280 | composition root |

150 tests across 33 files. `nexus-vcs` has zero.

---

## 2. Boundaries: sound, and genuinely enforced

The structural graph confirms the layering is real, not aspirational:

```
crate            fan-out   fan-in
nexus-core            35      132     ← the hub, as designed
nexus-types            0       42     ← leaf, as designed
nexus-lang             6       41
nexus-store           10       14
nexus-vcs              0        3
cap-*                ~35        2     ← depend on core, nothing depends on them
```

No cross-crate cycles. `nexus-cli/tests/boundaries.rs` reads `cargo metadata` and fails the
build on seven forbidden edges. **This is the single best thing about the codebase and the
redesign must not touch it.**

## 3. What to preserve

These are load-bearing and expensive to have gotten right:

- **The finding lifecycle.** Fingerprint identity that survives renames and reformats, an
  occurrence ledger, a status machine where `FIXED` requires evidence, and a `capability`
  column so a review comment and a bug share machinery without sharing rules. ADR-007/018/021.
- **The impact traversal** (`impact.rs`). Bounded weighted BFS, `score = Π edge_weight ×
  confidence`, fan-out cap that *reports* being capped, `min_confidence` along the chain, and
  the full `Hop` path on every result. It is explainable by construction — which is exactly
  the property the Context Engine needs, so it is the ranker's foundation, not a thing to
  replace.
- **The cross-stack seam.** The `.graphqls` schema is the contract, not the annotations
  (ADR-014 revision). This is the crown jewel: nothing in the source text connects
  `fetch('/api/x')` to `@QueryMapping`, and Nexus connects them. On a real 880-file project,
  5,665 symbols, 96 % of in-project edges resolved, 641 ms.
- **`sibling` vs `external` resolution.** On a six-service monorepo, 6,247 of 9,514 "external"
  edges were the project's own code. The distinction is what makes impact numbers trustworthy.
- **The evidence requirement and the 0.75 model clamp.**
- **`budget::fit`'s instinct** — that a response has a ceiling a caller cannot widen, and that
  truncation must never be silent. The instinct is right; its placement is wrong (§4, P1).
- **Append-only ledgers, two hashes per symbol, SQL-in-one-crate, argv-never-strings,
  stdout/stderr discipline, exit codes as interface.**

---

## 4. Problems

### P1 — There is no Context Engine, and the design doc says there should be

[`memory-model.md`](../memory-model.md) §3 specifies `ContextBuilder` in `nexus-core::context`
with a relevance formula and a top-K token budget. **It does not exist.** `grep` for
`ContextBuilder`, `mod context`, `token_budget`, `relevance` across `nexus-core` and
`nexus-store` returns nothing.

What exists instead is `nexus-mcp::budget::fit` — 60 lines that serialize a value and trim an
array until the bytes fit. It is *truncation applied at the adapter*, after assembly, with no
knowledge of what matters. Its own note says "showing the N highest-ranked", but nothing
ranked them.

Consequences: the core has no notion of a token budget; the CLI has no equivalent at all; and
the agent does the assembly itself, across many calls, at full token price. **This is the gap
between the current product and the stated mission.**

### P2 — Nine of twenty-one tables are unwired

Never inserted into, never read:

`external_deps` · `commits` · `tests` · `test_coverage` · `test_runs` · `ui_strings` ·
`audit_events` · `finding_verifications` · `finding_relations`

**None of them is obsolete.** Each is a designed subsystem with no code yet, and
[`roadmap.md`](../roadmap.md) states the reason they exist early: adding them later would mean
migrating a populated database. They stay.

> **Correction.** An earlier draft of this document claimed *twelve of twenty-four* tables were
> dead, including a legacy `bugs*` set that "should be dropped, not carried". That was wrong.
> Migration `0003` renamed `bugs` → `findings` and dropped the originals, so those four tables
> have never existed in any database this code creates. The error came from grepping
> `nexus-store/src/lib.rs` for `FROM bugs`, finding zero hits, and reading *absent* as *dead*.
> Schema questions are answered against a database, not against a grep.

Three of the nine are not cosmetic:

- **`commits` dead** ⇒ no git intelligence is persisted. `nexus-vcs` is 156 lines: HEAD, dirty
  flag, changed paths. No blame, no author, no churn, no co-change, no history. One of the ten
  required pillars is essentially absent.
- **`tests` / `test_coverage` dead** ⇒ "what covers this change?" — Review's flagship rule — is
  answered by `impact::is_test`, a **path-name string match** (`/test/`, `.test.ts`, `Test`
  suffix). A schema designed for real coverage sits empty beside it.
- **`test_runs` / `finding_verifications` dead** ⇒ the verification subsystem has no data layer
  in use, because it has no code either (P5).


### P3 — Language is not actually an extension point

`nexus-core/Cargo.toml` depends on `nexus-lang-java`, `nexus-lang-ts` and
`nexus-lang-graphql`; `engine.rs` imports and constructs all three. The `LanguageAnalyzer`
trait and `Registry` exist, and the analyzers correctly know nothing of storage — but the
*choice* of analyzers is compiled into the platform.

Compare capabilities, which are registered by the composition root and cannot be reached from
the core. The two extension points are asymmetric, and the language one is the fake.

**Proof, and it is stark:** Nexus indexes its own repository as **113 files, 0 symbols, 0
edges**. There is no Rust analyzer. The tool cannot be used on itself, so no design decision
here has ever been tested against its own codebase.

### P4 — The rule abstraction is triplicated, and there are two graph implementations

Each capability defines its own trait with the same shape:

| Crate | Trait | Signature |
|---|---|---|
| `cap-architect` | `Rule` | `run(&self, ctx, scoped) -> Vec<Finding>` |
| `cap-bughunter` | `Detector` | `run(&self, ctx, scoped) -> Vec<Finding>` |
| `cap-review` | `Rule` | `run(&self, ctx, scoped, graph) -> Vec<Finding>` |

Each has its own `all()`. Each has its own doc comment saying the same thing.

Separately, `cap-review::rules::Graph` builds in-memory reverse adjacency with a bounded
`reachable_from` — a second traversal alongside `impact.rs`'s store-backed BFS, with different
depth semantics (4 vs 5) and no shared scoring. Two implementations of "who depends on this"
can disagree about the same symbol, silently.

### P5 — The verification subsystem is 271 lines of design and zero lines of code

[`verification-engine.md`](../verification-engine.md) specifies the pipeline, the SafeWriter
jail, the baseline-revision double run, the judgement matrix and the failure modes. No
`nexus-verify` crate exists. `AGENTS.md` references it as though it does.

Every surface currently says "nothing is verified by reproduction yet", which is honest — but
it means the agent's "done" is accepted on trust, which the brief explicitly rejects.

### P6 — `engine.rs` is a 2,069-line god file

`Engine` as the single public API is a good decision and should stay. Its *implementation* in
one file is not:

- `rescan` — ~522 lines
- `analyze` — ~239 lines
- 25 public methods, 20 private functions, one file

Four new subsystems are about to land. This file is what will resist them.

### P7 — `ProjectContext` materialises the entire project regardless of scope

`ProjectContext` holds `&[SymbolFacts]`, `&[EdgeFacts]`, `&[FileFacts]` — *all* symbols, *all*
edges, *all* files. `ctx.scoped(scope)` narrows **after** the full load.

So `analyze --files one/file.java` pays the same materialisation cost as a whole-project run.
At the current measured scale (5,665 symbols, ~40,000 edges) this is fine. At the 500 KLOC the
roadmap targets it is not, and the narrowing that `Scope` promises is not actually a saving.

### P8 — Agent integration is entirely model-decided

There are no hooks. `.claude-plugin/plugin.json` ships commands, a skill and `mcp.json`; there
is no `hooks.json`, and `grep -rn hooks` across the plugin surface finds nothing.

Every path into Nexus requires the model to *choose* to call it. `skills/nexus/SKILL.md` is a
well-argued plea to remember. That is the distance between "gives the agent better context"
and "hopes the agent asks" — and it is the second-largest gap after P1.

### P9 — `ask` is orchestration living in an adapter, and it is N+1

`nexus-cli/src/ask.rs` answers "what changed", "what's affected", "what's known", "what next".
Its `suggest()` loops over up to 40 changed symbols calling `engine.impact()` **and**
`engine.findings_for()` per symbol — 80 engine calls, each its own traversal.

By the repo's own rule — *"if a handler needs two Engine calls, the missing method belongs in
`nexus-core`"* — all of this belongs in the core. It is also **not exposed over MCP**, so the
agent cannot ask the single most context-shaped question in the codebase: *what should I look
at next?*

`nexus_get_known` in `nexus-mcp` breaks the same rule: two engine calls in one handler.

### P10 — Fact invalidation is documented and unimplemented

[`memory-model.md`](../memory-model.md) §2 rule 3: *"When a scan changes a symbol referenced in
a fact's `evidence_json`, that fact gets `invalidated_at` set and is excluded from retrieval
until re-established. A fact about code that no longer exists is a trap."*

`invalidated_at` is **read** in the retrieval query and **never written** anywhere in the
workspace. Supersession by `fact_key` works; invalidation by change does not exist.

So a fact recorded against `PaymentService#pay():48` survives that method being deleted, and
is retrieved forever as established knowledge with evidence pointing at a line that no longer
means what it did. This is a live correctness bug in the memory layer, and it is the exact
trap the doc warns about.

Retrieval is also only half the specified formula: `ORDER BY source, confidence DESC` — no
subject-match weighting, no recency decay, no top-K, no budget.

---

## 5. Existing integrations

| Surface | Shipped | State |
|---|---|---|
| **MCP server** | `nexus mcp` on stdio, 16 tools via `rmcp` | Working. Conformance-tested (`tests/mcp_conformance.rs`) |
| **Claude Code plugin** | `.claude-plugin/plugin.json` v0.3.0 — commands, skill, `mcp.json` | Working |
| **Slash commands** | 8 in `commands/` — scan, rescan, impact, known, explain, analyze, status, update | Working |
| **Skill** | `skills/nexus/SKILL.md`, describes three capabilities at three moments | Working, and well written |
| **Codex** | `integrations/codex/config.toml` | Config only; untested against a live Codex |
| **Copilot** | `integrations/copilot/mcp.json` | Config only |
| **Hooks** | — | **Absent.** `grep -rn hooks` across the plugin surface finds nothing (P8) |
| **CI** | `.github/workflows/` — `make check` = fmt + clippy `-D warnings` + test | Working |
| **Release** | `release.yml` fails a tag disagreeing with the workspace version; `install.sh` verifies checksums | Working |

The integration surface is **broader than it is deep**: three agents are configured, none is
invoked automatically, and the two non-Claude configs have never been exercised end to end.

---

## 6. Technical debt ledger

Ranked by *interest rate* — how much each one will cost the next four subsystems, not how ugly
it is.

| Debt | Size | Interest | Pay off in |
|---|---:|---|---|
| `engine.rs` 2,069 lines; `rescan` 522, `analyze` 239 | large | **High.** It is what four new subsystems must be added to | Phase 1.1 |
| **Fact invalidation unimplemented** (P10) | ~50 lines | **High, and compounding.** Every memory improvement built on rot | Phase 1.6 |
| 9 unwired tables (`commits`, `tests`, `test_coverage`, `test_runs`, `ui_strings`, `external_deps`, `audit_events`, `finding_verifications`, `finding_relations`) | — | **Medium.** Each is a designed capability with no code; they are the roadmap, not waste. None is obsolete | Phases 2, 4, 5 |
| `Rule`/`Detector` triplicated across 3 capabilities | −150 lines | Medium. A fourth capability triplicates it again | Phase 1.2 |
| Two graph implementations that can disagree (`cap-review::Graph` vs `impact.rs`, depth 4 vs 5) | ~100 lines | Medium. Silent disagreement about the same symbol | Phase 1.2 |
| `ask.rs` orchestrating in the CLI, N+1 (80 engine calls / 40 symbols) | ~200 lines | Medium. Blocks exposing `what next` over MCP | Phase 1.4 |
| `nexus-core` hard-depends on 3 `nexus-lang-*` crates (P3) | ~200 lines | Medium. Every new language is a core edit | Phase 5.1 |
| `ProjectContext` materialises everything, then narrows (P7) | ~350 lines | Low now, high at 500 KLOC | Phase 5.4 |
| `impact::is_test` — a filename string match standing in for coverage | ~300 lines | Medium. Review's flagship rule rests on it | Phase 4.5 |
| `nexus-vcs` has zero tests | ~150 lines | Medium. History work lands directly on it | Phase 1.5 |
| `project.rs` doc comment describes detectors, not `ProjectContext` | 1 line | Trivial. Noted because it misleads a reader on first contact | Phase 1 |

**Unnecessary complexity: almost none.** This is worth stating plainly, because it is unusual.
There is no speculative abstraction here — no plugin loader, no event bus, no dependency-injection
container, no interface with one implementer that did not need one. The `Capability` trait is the
only extension point and its doc comment explicitly argues *against* the alternatives that were
not built.

The debt above is overwhelmingly **unfinished design**, not over-design. That is a much better
position to redesign from, and it is why the plan in [`10-roadmap.md`](10-roadmap.md) adds rather
than replaces.

---

## 7. Summary judgement

**The foundations are good and the layering is genuinely well engineered.** The change
detection, the finding lifecycle, the impact traversal and the cross-stack seam are the hard
parts, they work, and they are pinned by tests and by hard-won trap documentation.

**What is missing is the layer the brief is actually about.** Nexus today is an excellent
*index with an agent-facing query API*. It is not yet an intelligence layer, because:

- nothing assembles context (P1),
- nothing invokes it automatically (P8),
- nothing checks the result (P5),
- history is not used (P2),
- and memory rots without saying so (P10).

None of those requires re-architecting what exists. All five are additions on top of a sound
base — which is why the chosen architecture in [`architecture.md`](04-future-architecture.md) is
additive, and why the migration in [`implementation-roadmap.md`](10-roadmap.md)
starts by deleting dead weight rather than by moving load-bearing walls.
