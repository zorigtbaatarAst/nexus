# Principles

Ten rules. Each one has teeth: a test, a boundary, or a number. A principle nothing can
violate is not a principle, it is a slogan.

---

## 1. Deterministic computation before LLM reasoning

If a query can answer it, no model is asked. The dividing line is
[`ai-integration.md` §5](../ai-integration.md): a model is for properties a compiler cannot
express — business-logic errors, races, transaction boundaries, data consistency,
behavioural regression. Call graphs, coverage, type errors, dead code, lint and test results
are queries.

**Teeth:** deterministic findings are not confidence-clamped; model findings are clamped at
0.75 (`MODEL_CONFIDENCE_CAP`). If a rule can express it, spending a token on it is spending
money to become less accurate.

## 2. Maximum useful information per token

The optimisation target. Not recall, not coverage, not feature count.

Every context payload is *selected under a budget*, never assembled and then truncated.
Truncation is the failure path; selection is the mechanism.

**Teeth:** every `ContextPackage` carries `tokens_estimated`, `items_considered`,
`items_included`. `nexus context --stats` prints the ratio. A change that raises tokens
without raising usefulness is a regression, and it is visible.

## 3. Reference, do not inline

Nexus says *where*, not *what*. A `file:line` plus a three-line window, never a whole file.
The agent already has `Read`; duplicating it is paying twice for the same bytes.

**Teeth:** no context item may exceed a fixed line window. A file-server-shaped API is
rejected at review.

## 4. Every inclusion and every exclusion is explainable

The Context Engine must answer *why was this here* and *why was that not* — with the score
terms, not a hand-wave.

**Teeth:** `ContextPackage.ledger` is mandatory output, not a debug flag. A ranker whose
decisions are invisible is a ranker whose bugs are invisible.

## 5. Evidence or nothing

A claim without a checkable `file:line` is rejected at the boundary, not down-ranked. This
already holds for findings and facts; it extends to context items and verification verdicts.

**Teeth:** empty-evidence candidates are counted and reported. A silently discarded finding
is indistinguishable from a model that found nothing — so it is never silent.

## 6. Memory is structured, and it decays with the code

Nexus stores rows, not transcripts. A chat log is unqueryable, unbounded, unverifiable and
provider-specific.

A fact whose evidence has moved is **invalidated**, not retrieved. Memory that outlives the
code it describes is not memory, it is a trap that reads as authority.

**Teeth:** `facts.invalidated_at` is set by the scan that changes a symbol named in the
fact's evidence. Pinned by a test that edits a symbol and asserts the fact stops surfacing.

## 7. The agent's "done" is a claim, not a result

Compile, test, lint, diff, impact. An infrastructure failure yields **inconclusive**, never
**failed** — a test that would not compile says nothing about the hypothesis.

**Teeth:** verification verdicts are a closed enum with `Inconclusive { why }`. Collapsing it
into `Failed` is what makes a gate get switched off.

## 8. Agent-agnostic at the seam, agent-specific only at the edge

The contract is the **CLI verb** and the **MCP tool**. Hooks, plugins and slash commands are
thin shells over those. No agent-specific logic ever enters the binary.

**Teeth:** a `cargo metadata` boundary test already forbids the core from knowing its
adapters. Adding "if Claude Code then…" anywhere in `crates/` is the smell this exists to
catch.

## 9. Fail open, never block the developer

Anything on the developer's critical path — a hook on every prompt, a rescan after every edit
— is time-bounded and exits 0 on failure, silently. A tool that occasionally hangs a session
gets uninstalled once and never reinstalled.

**Teeth:** every hook has a hard timeout and a fail-open exit. p95 budget stated per hook and
asserted.

## 10. New technology requires a fired trigger, not an argument

No daemon, no vector database, no LSP sidecar, no second index, no service until a **measured
number** says the simple thing stopped working. The trigger is written down before the feature
is wanted, so the decision is not made by whoever is most excited.

**Teeth:** [`roadmap.md`](../roadmap.md) V2 already states triggers as numbers
(`rescan > 2 s`, `impact p95 > 250 ms`, recall `< 85 %`). This document adds one for vector
search: a **measured retrieval miss rate that graph + lexical + git ranking cannot close**.
Until then, it is a dependency with no problem.

---

## The four inherited invariants

Not new, not negotiable, restated so a redesign cannot quietly drop them:

- **Ledger tables are append-only.** `scans`, `changes`, `commits`, `test_runs`,
  `finding_occurrences`, `finding_verifications`, `audit_events`. An `UPDATE` there destroys
  regression detection, which is the strongest thing the product does.
- **Cache invalidation includes tool versions.** `scans.tool_versions_json`. Bump a grammar
  without it and the index keeps the old wrong symbols forever, with no error anywhere.
- **Two hashes per symbol.** `sig_hash` ripples to every caller; `body_hash` ripples only
  along data and effect edges. Collapse them and impact becomes noise.
- **Only `nexus-store` contains SQL.** No exceptions, including "just this one query".
