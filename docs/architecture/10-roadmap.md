# Roadmap

Six phases. Each ships something true and useful on its own; no phase requires rewriting an
earlier one. Sizes are rough Rust line counts for the change, not calendar estimates.

**One deliberate departure from the requested ordering.** Fact invalidation is scheduled in
**Phase 1**, not Phase 3 where the rest of the memory work lives. It is not a feature — it is a
live correctness bug (R5: `invalidated_at` is read and never written), and every later memory
improvement compounds on top of the rot until it is fixed. Fifty lines, moved forward.

---

## Phase 0 — Architecture foundation

**Objective:** a clear, realistic, evidence-based master plan before any code changes.

**Deliverables:** this directory — 13 documents, an ADR set, 5 diagrams.

**Dependencies:** none.

**Risks:** designing against a codebase we cannot dogfood (R6), and analysis that outruns
evidence.

**Success criteria**
- Every claim about the current state is measured, not inferred, and states its method.
- Every non-goal names the trigger that would reverse it.
- Every phase below has a *test* as its definition of done, not a feeling.
- All diagrams render (`mermaid-cli`); all internal links resolve.

**Do not build yet:** anything. No production code changes in this phase.

**Status: complete.** Phase 1 began on 2026-09-02.

---

## Phase 1 — Nexus foundation

**Objective:** clear the ground and land the smallest useful spine. Ships one visible feature —
a session package — and pays off the debt that would otherwise resist everything after it.

**Features**

| # | Task | Size |
|---|---|---:|
| 1.1 | Split `engine.rs` → `engine/{mod,scan,rescan,analyze,query}.rs`. Public API byte-identical | ~0 net |
| 1.2 | Hoist `Rule` into `nexus_core::rules` (one `Rule`, one `Graph`); delete the three private copies and `cap-review::Graph` | −150 |
| ~~1.3~~ | ~~Migration `0006`: drop `bugs`, `bug_occurrences`, `bug_verifications`, `bug_relations`~~ **Void.** Those tables never exist: migration `0003` renames them to `findings*`. Verified against a live database on 2026-09-03 — `nexus init` then `SELECT name FROM sqlite_master WHERE name LIKE 'bug%'` returns no rows. See [`03`](03-current-state.md) P2 | 0 |
| 1.4 | Move `ask.rs` orchestration into `Engine::ask` (`engine/query.rs`). The N+1 is documented, not fixed: `findings_for` matches on five conditions and has no batched form that keeps one definition of a match (commit `ba915d3`) | ~200 |
| 1.5 | `nexus-vcs` tests — it has zero, and history work lands on it next | ~150 |
| 1.6 | **Fact invalidation on change** — set `invalidated_at` when a scan moves a symbol named in a fact's evidence | ~50 |
| 1.7 | `ContextPackage` / `ContextItem` / `InclusionLedger` types; `nexus context --session` | ~350 |
| 1.8 | `SessionStart` hook + `nexus init --hooks`. **Off by default** | ~80 |

**Dependencies:** none. Every task is inside the existing architecture.

**Status (2026-09-03): complete.** 1.1, 1.2, 1.4, 1.5 landed on 2026-09-02 (`c82bb52`,
`eded128`, `ba915d3`, `ffef5ba`); 1.6, 1.7 and 1.8 on 2026-09-03; 1.3 is void, see the row.

Success criteria, each against the code:

- `make check` green at **205 tests**; no behavioural surface moved by 1.1–1.5.
- A fact whose evidence symbol is edited stops being retrieved and the row still exists —
  `nexus-core/tests/fact_invalidation.rs`, plus `nexus-store`'s row-kept assertion.
- `nexus context --session` returns profile, open findings and durable facts inside 800
  estimated tokens — `nexus-core/tests/session_context.rs`.
- The `SessionStart` hook fails open: with `nexus` absent from `PATH` the hook's own command
  string exits 0 and prints nothing — `nexus-cli/tests/hooks.rs`.

**Measured, not hoped:** `nexus context --session` p95 is **4 ms** on this repository
(113 files) and 2 ms on a one-file fixture, against ADR-024's 400 ms budget. Twenty runs of
the release binary.

**Two gaps this phase leaves, both named rather than absorbed.** The `nexus fact` verb takes
no evidence, so a fact recorded at a terminal is unanchored: it is never invalidated by 1.6
and never included by 1.7, which excludes it with `no file:line anchor`. Fixing it belongs
with **3.5**, the human entry point. And Phase 1 selection is a fixed query in store order,
so on a project with many open findings the facts can be squeezed out entirely — visible in
the ledger, and what **2.7**'s density sort exists to fix.

Next: **Phase 2**, starting at 2.1.

**Risks:** R7 (core god object — 1.1 is the mitigation, which is why it is first), R2 (hook
latency, first exposure).

**Success criteria**
- `make check` passes; MCP conformance unchanged; no behavioural surface moved by 1.1–1.5.
- A fact whose evidence symbol is edited stops being retrieved, **and the row still exists**.
- `nexus context --session` returns profile + open findings + durable facts in **≤ 800 tokens**.
- The `SessionStart` hook fails open: removing `nexus` from `PATH` mid-session leaves Claude Code
  fully working.

**Do not build yet**
- Ranking. The session package is a fixed query, not a ranked selection — that is Phase 2.
- The `UserPromptSubmit` hook. Measure `SessionStart` latency first.
- Any new language analyzer.

---

## Phase 2 — Context intelligence

**Objective:** the Context Engine. Ask for context on a task, get a ranked, budgeted package with
a full account of why each thing is in it — one call, no model.

**Features**

| # | Task | Size |
|---|---|---:|
| 2.1 | Stage 1 intent — deterministic verb table; `Unknown` a first-class outcome | ~150 |
| 2.2 | Stage 2 seeds — explicit · fqn/path · changed set · name match · fact subject | ~250 |
| 2.3 | Stage 3 expand — reuse `impact::run`, direction from intent | ~80 |
| 2.4 | Stage 4 signals — churn, recency, coverage, prior findings, facts, profile | ~200 |
| 2.5 | Populate `commits`; git history primitives in `nexus-vcs`, derivation in `core::history` | ~300 |
| 2.6 | Stage 5 rank — one weighted sum, every term recorded; weights in `policy.toml` | ~300 |
| 2.7 | Stage 6 budget — density sort, greedy fill, diversity guard, score floor | ~200 |
| 2.8 | The inclusion ledger and `--explain` | ~150 |
| 2.9 | Package cache on `(intent, seeds, HEAD, dirty hash, budget, weights hash)` | ~120 |
| 2.10 | `nexus context --task` CLI; `nexus_get_context` + `nexus_what_next` MCP tools | ~250 |
| 2.11 | `UserPromptSubmit` hook | ~40 |
| 2.12 | Graphify consumption for unanalysed languages, behind `resolution = "external-graph"` | ~200 |
| 2.13 | **Tier 1 evaluation harness** — golden packages, ledgers, re-baselining protocol ([`13`](13-evaluation.md) §10) | ~300 |
| 2.14 | `--carry-seeds` / `--recent` and `Intent::Referential` ([`13`](13-evaluation.md) §14.1) | ~120 |

**Dependencies:** Phase 1 (all of it — 1.1 for room, 1.4 for the query layer, 1.7 for the types).

**Risks:** **R1 (ranker confidently wrong — the defining risk of this phase)**, R2 (hook latency
on the critical path), R8 (weights become folklore), R9 (stale cache).

**Success criteria**
- **Golden packages:** five fixed tasks on the `spring-payments` fixture, contents asserted. A
  ranking change that alters them must be deliberate.
- **Every excluded candidate carries a reason.** Asserted — an unexplained exclusion fails the
  test.
- `nexus context --task` p95 **< 150 ms** on the 880-file fixture. Asserted, because it is on a
  per-prompt hook.
- `--stats` prints `items_considered`, `items_included`, `tokens_estimated`.
- A dirty working tree produces a cache miss.
- **Tier 1 of [`13-evaluation.md`](13-evaluation.md) passes**: golden packages exact, every
  candidate explained, dirty-tree cache miss asserted, anchor retention ≥ 0.8.
- The Tier 2 benchmark runs end to end on at least one task family, with a complete run
  manifest. Thresholds are not gated until Phase 4 — the number that matters is that the
  measurement exists before the claims do.

**Do not build yet**
- The Tier 2/3 benchmark arms. The corpus repositories must exist first.
- **Weight tuning.** Ship the ledger, gather evidence, *then* tune. Tuning before measurement is
  folklore (R8).
- Context compression or summarisation. Selection first; if selection works, compression is
  solving a problem that no longer exists.
- Embeddings. N9's trigger cannot fire before this phase produces the ledger data that would fire
  it.

---

## Phase 3 — Persistent engineering memory

**Objective:** memory with a lifecycle — knowledge that is validated, ages honestly, and is
retrievable by humans as well as machines.

**Features**

| # | Task | Size |
|---|---|---:|
| 3.1 | Lifecycle states `candidate → validated → durable`; the per-scan validation pass | ~200 |
| 3.2 | Full retrieval formula (subject match · state weight · recency), replacing `ORDER BY source, confidence` | ~120 |
| 3.3 | Extended `fact_key` namespaces: `discovery.` `failure.` `incident.` `pattern.` `constraint.` | ~60 |
| 3.4 | `nexus memory export --markdown` — one file per namespace, `[[wikilinks]]`, generated header | ~200 |
| 3.5 | `nexus fact add` — the human entry point, `source='human'`, straight to durable | ~80 |
| 3.6 | `nexus export` / `import` for findings and facts; conflicts reported, never auto-resolved | ~250 |

**Dependencies:** Phase 1.6 (invalidation must already work — the lifecycle sits on top of it),
Phase 2.6 (facts are ranked by the same function as everything else).

**Risks:** R5 (residual rot in the validated/durable transitions), and memory that grows without
bound — mitigated because facts are keyed and superseded rather than appended per observation.

**Success criteria**
- A fact recorded by an agent, validated across three scans, is retrieved at durable weight.
- A fact whose evidence moved is invalidated, **kept on disk**, and re-establishable by a new fact
  under the same key.
- `nexus memory export --markdown` output opens as an Obsidian vault with working backlinks, and
  **Nexus never reads it back**.
- "What did Nexus believe at scan 12, and what changed its mind?" is answerable from the database.

**Do not build yet**
- Obsidian plugins, sync, or a vault manager. The exporter writes files; a viewer is the
  viewer's problem.
- Any parse-back path from Markdown into the store. One-way, permanently.
- A team-shared server. `export`/`import` over a committed file is the first answer (N13).

---

## Phase 4 — Verification intelligence

**Objective:** "done" gets checked. An agent's completion claim is followed by a compile, a test
run, a lint, and a verdict.

**Features**

| # | Task | Size |
|---|---|---:|
| 4.1 | `nexus-verify` crate: `Plan`, `Check`, `Verdict`; allowlist execution, argv only, timeouts | ~400 |
| 4.2 | Build/test/lint command derivation from the existing `detect` profile | ~150 |
| 4.3 | Baseline-revision run and the four-cell judgement matrix | ~250 |
| 4.3b | **Detached-worktree baseline with a per-sha cache** ([`13`](13-evaluation.md) §13.1) — `git stash` is never used | ~150 |
| 4.4 | Populate `test_runs` and `finding_verifications` | ~150 |
| 4.5 | `test_coverage` from runner output; retire `impact::is_test` as the coverage source | ~300 |
| 4.6 | `nexus verify` CLI + `nexus_verify` MCP tool | ~150 |
| 4.7 | `Stop` and `PostToolUse` hooks | ~60 |
| 4.8 | Verification → finding status transitions (`VERIFIED`, `REGRESSED`) — the feedback edge | ~120 |
| 4.9 | Boundary test: `nexus-verify` must not depend on `nexus-store` | ~20 |

**Dependencies:** Phase 1 (change detection via the split engine), Phase 2.5 (baseline revision
handling shares the history primitives).

**Risks:** **R3 (executing project commands — the largest security surface in the system)**, and
the gate being switched off if it cries wolf.

**Success criteria**
- **An already-red suite yields `Inconclusive`, never `Failed`.** Asserted. This single rule
  decides whether the gate survives contact with a real project.
- A missing build tool yields `Inconclusive` with a remedy, never a crash.
- `policy.execute = "none"` yields `permission_required` over MCP, never an execution.
- Review's "nothing tests this" finding is backed by a real coverage row, not a filename match.
- On `spring-payments`, commits 3 → 6 → 7 produce `VERIFIED` → `FIXED` → `REGRESSED` with the
  correct commits recorded at each step.
- **A dirty tree still gets a baseline verdict**, computed once per sha in a detached worktree
  and reused thereafter.
- **The T3–T8 thresholds of [`13-evaluation.md`](13-evaluation.md) §11 become release gates
  from this phase on.**

**Do not build yet**
- **Test generation.** It arrives with the `SafeWriter` jail in Phase 5, never before (N15).
- Docker sandboxing. Host execution under the allowlist first; measure whether the sandbox is
  needed.
- Auto-fix on a failed verdict. Permanently out of scope (N6).

---

## Phase 5 — Advanced intelligence

**Objective:** breadth, once the loop is proven. Nothing here starts before Phases 1–4 have
demonstrated value on a real project.

**Features**

| # | Task | Size |
|---|---|---:|
| 5.1 | Inject `nexus-lang::Registry` at the composition root; drop `nexus-lang-*` from `nexus-core`'s manifest; add the boundary test | ~200 |
| 5.2 | `nexus-lang-rust` — **Nexus can finally index itself** | ~900 |
| 5.3 | `nexus-lang-python` + Django/FastAPI pack | ~900 |
| 5.4 | Scoped `ProjectContext`: load only what the scope admits | ~350 |
| 5.5 | `ui_strings` population + FTS5 — text seeding from any locale | ~400 |
| 5.6 | Reproduction-test generation behind `SafeWriter`; Docker sandbox, host opt-in | ~700 |
| 5.7 | Weight tuning from accumulated ledger data — the first tuning backed by evidence | ~100 |

**Dependencies:** all previous phases. 5.7 depends specifically on Phase 2.8 having accumulated
real ledger data.

**Risks:** R4 (scope — this phase is the most temptingly open-ended), R6 until 5.2 lands, and R3
again at 5.6.

**Success criteria**
- **`nexus scan` on this repository reports non-zero symbols and edges.** Today: 113 files,
  0 symbols, 0 edges. That number is the acceptance test.
- A capability run under `--files one.java` measurably loads less than a full run.
- A Mongolian UI label reaches the backend method that serves it, with no model involved.
- A weight change at 5.7 cites ledger evidence in its commit message.

**Do not build yet**
- Languages beyond Rust and Python. Each analyzer is permanent maintenance surface; add on
  demand.
- The daemon, LSP sidecars, embeddings, or a team server — all still behind their triggers
  ([`12-non-goals.md`](12-non-goals.md)).

---

## Triggered, never scheduled

Not a phase. Each unlocks on a number, written down now so the decision is later made by a
measurement.

| Item | Trigger | Status |
|---|---|---|
| `nexusd` daemon + watcher | no-op `rescan` > 2 s, or `impact` p95 > 250 ms | **not fired** — 641 ms full scan, 880 files |
| LSP sidecars | impact recall < 85 % for a language | **not fired** — 96 % resolved |
| Vector search / embeddings | ledger misses clustering on semantic similarity | **not fired** — no evidence gathered |
| Team-shared store (server) | >1 developer maintaining the same findings, *and* export/import proving insufficient | not fired |
| Monorepo sharding | full scan > 30 min, or CI write contention | not fired |
| Cross-repo service graph | the seam crosses repositories in a real estate | not fired |
| CI mode / PR annotations | adoption in pipelines | not fired |
| Go / C# / Kotlin analyzers | user demand; each is one crate behind `LanguageAnalyzer` | not fired |

---

## Sequencing

```
  Phase 1            Phase 2              Phase 3            Phase 4            Phase 5
  ───────────────    ─────────────────    ───────────────    ───────────────    ─────────────────
  split engine       intent + seeds       lifecycle states   nexus-verify       language registry
  hoist Rule         expand + signals     full retrieval     baseline matrix    rust + python
  drop bugs*         commits + churn      namespaces         test_runs          scoped context
  ask → core         rank + budget        markdown export    real coverage      ui_strings + FTS5
  vcs tests          ledger + explain     fact add           verify hooks       test generation
  fact invalidation  cache                export/import      status feedback    weight tuning
  session package    task hook + MCP
  session hook       graphify signal
```

Read left to right: nothing forces a rewrite of anything to its left. If a phase would require
one, the boundary was drawn in the wrong place and the design is wrong, not the plan.

Full risk detail, with detection signals, in [`11-risks.md`](11-risks.md).
