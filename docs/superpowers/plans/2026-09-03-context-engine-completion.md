# Context Engine Completion (roadmap 2.4 – 2.14) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish Phase 2. `nexus context --task "<prompt>"` returns a ranked, budgeted package with a full account of why every candidate is in or out, in one call, with no model anywhere in the pipeline.

**Architecture:** Stages 1–3 exist as library functions. This plan adds stages 4–7 beside them, then assembles the seven into `Engine::context`, then exposes it. Weights become data in `policy.toml`. Git history gets primitives in `nexus-vcs` and a populated `commits` ledger. Everything the pipeline decides is recorded per candidate, because a ranker that cannot be decomposed cannot be debugged.

**Tech Stack:** Rust 1.82+, `toml` 0.9 (already a workspace dependency), `git2` revwalk, existing `impact::run`.

**Spec:** [`05-context-engine.md`](../../architecture/05-context-engine.md) §6 (ranking formula and sub-terms), §7 (budget), §8 (explainability), §10 (freshness), §11 (caching), §12 (prohibitions); [`07-agent-integration.md`](../../architecture/07-agent-integration.md) §2 (hook table, three new MCP tools); [`13-evaluation.md`](../../architecture/13-evaluation.md) §10 (Tier 1) and §14.1 (multi-turn); [ADR-024](../../architecture/decisions/ADR-024-hooks-are-the-invocation-tier-and-ship-off-by-default.md).

## Note on this plan's form

The other plans in this directory give every step as runnable code. Eleven tasks at that fidelity would be a document longer than the code it describes, and nobody would read it. This one fixes the **design decisions and the acceptance criterion** for each task and gives code only where the obvious implementation is wrong. Each task still ends with `make check` green and one commit naming its roadmap id.

## Global Constraints

- **Roadmap 2.4 through 2.14 is the scope.** Phase 3, 4 and 5 items stay out: no fact lifecycle, no `nexus-verify`, no new language analyzer, no test generation.
- **No stage calls a model. Ever.** §12. The pipeline is queries, arithmetic and a sort.
- **Every excluded candidate carries a reason.** §8. Asserted by 2.13; an unexplained exclusion fails the build.
- **No item without a `file:line` anchor, no whole file, no padding of remaining budget, no silent truncation.** §12.
- **Weights are data, not code** (§6). They live in `.nexus/policy.toml` with documented defaults, so tuning is a config change and a re-run.
- **No weight tuning in this phase.** The roadmap's "do not build yet" is explicit: ship the ledger, gather evidence, then tune (2.7 of Phase 5). Defaults are argued, not fitted.
- **Avoid N+1.** Signals are built as one index per request from a fixed number of queries, never one query per candidate. This is on a per-prompt hook budgeted at 150 ms.
- **`make check` after every task.** Baseline: **222 passing tests**. No task may reduce it.
- **Only `nexus-store` contains SQL.** New queries go in the store.
- **`nexus-core` must not gain an HTTP client.** `toml` is fine; the boundary test checks for `reqwest`/`hyper`/`ureq`.
- **`nexus-mcp` handlers stay: deserialize → one `Engine` call → serialize.**
- **Ledger tables stay append-only.** `commits` is a ledger: inserted, never updated.
- **`git add` names files.** Commit per task, message naming the roadmap id, ending with:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_014cmQc2a8FrfdFr7WzGM4iC
  ```

---

## Decisions taken before writing code

**1. Churn cannot come from the `commits` table, and the schema says so.** `commits` has no path column — it is a commit ledger, not a file-touch index. So 2.5 splits: the table is populated for history questions and for the basis of a package, and **churn is derived at request time** from one `nexus-vcs` revwalk that returns per-path touch counts for a window. One git traversal per request, cached in the signal index, rather than one `git log` per candidate.

**2. 2.4 ships before 2.5 and degrades honestly.** The roadmap orders signals before history, but churn needs history. Rather than reorder, the churn signal returns 0 and records `no commit history yet` until 2.5 lands, exactly as the text-match seed source reports its empty table. Order is preserved and each task is independently shippable.

**3. Coverage in this phase is the filename match, and the signal says so.** Real coverage is roadmap 4.5. `impact::is_test` is what exists. The signal carries `source: "naming"` so that when 4.5 lands, the consumer changes nothing and the provenance changes from `naming` to `runtime`.

**4. One formula, no special cases.** §6's weighted sum, every term stored in the `ScoreTerms` struct that Phase 1 already defined. No rule of the form "always include the controller": §6 names that as how a ranker becomes folklore.

**5. The cache is a file, keyed by content, and a miss is silent.** §11's key is `(intent, seeds, HEAD, dirty hash, budget, weights hash)`. It lives in `.nexus/cache/context/<hash>.json`, which `init` already creates and `.gitignore` already ignores. A corrupt or unreadable entry is a miss, never an error: a cache that can fail a request is worse than no cache.

**6. `nexus_what_next` is `ask next` over MCP.** 07 §2 says it already exists in the CLI and has never been reachable by an agent. It is a thin handler over the existing `Engine::ask`, not new logic.

**7. Graphify consumption needs a migration.** `symbol_edges.resolution` has a CHECK constraint listing five values. `external-graph` is a sixth. Migration `0006` rewrites the constraint and bumps `SCHEMA_VERSION` to 6. Edges from an external graph are **never counted in the resolution rate** — they are a different kind of evidence, and folding them in would inflate the number ADR-017 exists to keep honest.

**8. `--recent` and `--carry-seeds` are inputs, never state.** §14.1: Nexus stays a pure function of (request, index, memory). `--recent` reaches the verb table and is discarded; it never touches the store.

---

## Tasks

### 2.4 — Stage 4: signals

**Files:** create `crates/nexus-core/src/context/signals.rs`; modify `context/mod.rs`, `engine/query.rs`; store gains `findings_by_anchor` if a batched form is needed.

**Deliverable.** `SignalIndex::build(store, project_id)` runs a fixed number of queries and answers per candidate: `churn`, `recency`, `coverage`, `prior_findings`, `facts`, `arch`. `Signals` is a plain struct with a `notes` list for what could not be computed.

Sub-terms are §6's, verbatim: `subject_match` exact FQN 1.0 / module prefix 0.6 / project 0.3; `source_weight` human 1.0 / deterministic 0.9 / ai 0.7; finding status REGRESSED 1.0 / VERIFIED 0.8 / UNVERIFIED 0.5 / IGNORED 0; recency half-life 30 days.

**Acceptance.** A candidate carrying a REGRESSED finding scores 1.0 on the history term and 0.5 for an UNVERIFIED one. A fact whose subject is the candidate's FQN scores higher than one whose subject is its package. Building the index for N candidates issues a number of queries independent of N — asserted by a test that builds it twice with different candidate counts and compares a query counter.

### 2.5 — Populate `commits`; history primitives

**Files:** modify `crates/nexus-vcs/src/lib.rs`, `crates/nexus-store/src/lib.rs`; create `crates/nexus-core/src/history.rs`; modify `engine/scan.rs` and `engine/rescan.rs`.

**Deliverable.** `Repo::recent_commits(limit)` and `Repo::touch_counts(window_days)` via one revwalk. `Store::insert_commit` (append-only, `INSERT OR IGNORE` on the unique sha). Scan and rescan record the commits they can see. `core::history::churn` normalises touch counts to `log1p(n)/log1p(max)`.

**Acceptance.** After a scan of a repository with three commits, `SELECT COUNT(*) FROM commits` is 3, and scanning again does not duplicate them. A file touched by every commit scores churn 1.0; one touched once scores lower; an untouched file scores 0.

### 2.6 — Stage 5: rank

**Files:** create `crates/nexus-core/src/context/rank.rs`, `crates/nexus-core/src/policy.rs`; modify `engine/mod.rs` (`DEFAULT_POLICY` gains `[context.weights]`), `context/mod.rs`.

**Deliverable.** `Weights` loaded from `.nexus/policy.toml`, defaults documented in the file itself. `rank::score(candidate, signals, weights) -> (f64, ScoreTerms)` implementing §6's sum. `Weights::hash()` for the cache key.

Defaults, argued rather than fitted: `seed 1.0`, `graph 0.8`, `churn 0.3`, `recency 0.2`, `history 0.6`, `fact 0.5`, `test 0.3`, `arch 0.3`, `cost 0.4`. Seeds dominate because an explicitly named symbol is not a guess; cost is meaningful but never decisive on its own.

**Acceptance.** Every term in `ScoreTerms` is non-zero for at least one candidate in a fixture where all six signals fire. Editing a weight in `policy.toml` changes the order without recompiling — asserted by a test that writes a policy file and re-reads it. A missing or malformed `policy.toml` yields the defaults and a note, never an error.

### 2.7 — Stage 6: budget

**Files:** modify `crates/nexus-core/src/context/mod.rs` (replace `fill`).

**Deliverable.** Density sort (`score / token_cost`), greedy fill, diversity guard (`MAX_PER_COMPONENT`, default 3), score floor (`min_score`). Each exclusion names which rule refused it.

**Acceptance.** A 40-token item scoring 0.6 is included ahead of a 900-token item scoring 0.9. A class with ten qualifying methods contributes at most three before another component gets a turn. An item below the floor is excluded even with budget remaining, and its ledger row says `below floor`, not `budget exhausted`. The session package's Phase 1 behaviour is preserved: its candidates are already ordered and all score equal, so the density sort is stable over them.

### 2.8 — The ledger and `--explain`

**Files:** modify `crates/nexus-cli/src/render.rs`, `crates/nexus-cli/src/main.rs`.

**Deliverable.** `--explain` renders §8's format: decision, score, label, reason, and the term breakdown for included items.

**Acceptance.** Output shows an included item's terms summing to its score, and an excluded item's reason. Every ledger row renders; none is elided.

### 2.9 — Package cache

**Files:** create `crates/nexus-core/src/context/cache.rs`; modify `engine/query.rs`.

**Deliverable.** Key over `(intent, seed fqns, HEAD sha, dirty hash, budget, weights hash)`. Read-through, write-behind, in `.nexus/cache/context/`.

**Acceptance.** Two identical requests on an unchanged tree: the second is a hit. Touching a tracked file changes the dirty hash and produces a miss. A corrupt cache file is a miss, not an error.

### 2.10 — `nexus context --task` and the MCP tools

**Files:** modify `crates/nexus-cli/src/main.rs`, `render.rs`, `crates/nexus-mcp/src/lib.rs`; wire `Engine::context` to the full pipeline.

**Deliverable.** `--task "<text>"` with `--budget`, `--explain`, `--stats`, `--files`, `--symbols`. MCP `nexus_get_context` and `nexus_what_next`.

**Acceptance.** `--stats` prints `items_considered`, `items_included`, `tokens_estimated`. `Purpose::Task` no longer returns `Unsupported`. `--session` behaviour is unchanged. Each MCP handler makes exactly one `Engine` call.

### 2.11 — `UserPromptSubmit` hook

**Files:** modify `crates/nexus-cli/src/hooks.rs`, `tests/hooks.rs`.

**Deliverable.** Second hook entry, `nexus context --task "$CLAUDE_USER_PROMPT" --budget 4000`, same fail-open string form, installed by the same flag.

**Acceptance.** Both hooks installed, idempotent, and the new command fails open with `nexus` off the path.

### 2.12 — Graphify consumption

**Files:** create `crates/nexus-store/migrations/0006_external_graph.sql`; modify `nexus-store/src/lib.rs` (`SCHEMA_VERSION` 6), `crates/nexus-core/src/graphify.rs` (new), `engine/scan.rs`.

**Deliverable.** When `.nexus/config.toml` sets `resolution = "external-graph"` and a graphify output exists, its edges are imported for files no analyzer claims, with `resolution = 'external-graph'` and a confidence ceiling below any parsed edge.

**Acceptance.** Imported edges appear in impact results, are excluded from the resolution-rate denominator, and are absent when the config flag is off. Migration applies cleanly to an existing database.

### 2.13 — Tier 1 evaluation harness

**Files:** create `crates/nexus-core/tests/golden_packages.rs`, `crates/nexus-core/tests/golden/*.json`.

**Deliverable.** Fixed tasks over a generated fixture, package contents asserted against committed goldens, with a documented re-baselining command.

**Acceptance.** Five golden tasks pass. Every candidate in every golden ledger carries a reason. A ranking change that alters a golden fails the build with a readable diff.

### 2.14 — `--carry-seeds`, `--recent`, `Intent::Referential`

**Files:** modify `context/intent.rs`, `context/seeds.rs`, `context/mod.rs`, `engine/query.rs`, `crates/nexus-cli/src/main.rs`.

**Deliverable.** `Intent::Referential` when target extraction is empty and a referential marker is present. `--carry-seeds` enters stage 2 at `w_carry`; `--recent` reaches the verb table only.

**Acceptance.** "Now do the same for orders" with no carried seeds reports `Unknown` rather than guessing. With carried seeds it anchors to them. `--recent` never reaches the store — asserted by a test that passes a fact-shaped string and checks `facts` is unchanged.

---

## Self-review

**Spec coverage.** §6's formula, sub-terms and "weights are data": 2.6. §7's four budget rules: 2.7. §8: 2.8, asserted by 2.13. §10's basis fields: already in `PackageBasis` from 1.7. §11's cache key: 2.9. 07 §2's hook table row and three MCP tools: 2.10 and 2.11 (`nexus_verify` is Phase 4). 13 §10 and §14.1: 2.13 and 2.14.

**Risks this plan carries.** Two, both recorded now. The default weights are argued, not measured, and the roadmap forbids tuning them here — so the first real evidence about them arrives with 2.13's goldens and is acted on in Phase 5.7. And the churn revwalk is one git traversal per request; if it dominates the 150 ms budget on a large history, the answer is a window cap, not a cached table, because a cached table is a second source of truth about history.
