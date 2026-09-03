# Resolution Accuracy Harness — Design

**Date:** 2026-09-03
**Status:** Design, approved for planning
**Scope:** Tier 0 of evaluation — measuring whether the dependency graph is *correct*, not merely *populated*

---

## 1. The problem

Nexus reports that 96 % of in-project edges resolved. That number has never been checked
against anything, and three separate defects mean it cannot be trusted as stated.

### 1.1 The metric measures coverage and is read as accuracy

`Store::edge_counts` (`crates/nexus-store/src/lib.rs:1265`) computes

```sql
SUM(dst_symbol_id IS NOT NULL AND resolution <> 'external-graph')
```

That counts edges that acquired *a* destination. Nothing anywhere checks whether the
destination is the *right* one. No test, no assertion, no report.

### 1.2 The metric is inflatable, and the resolver inflates it

Three tiers of `resolve_edges` write more than one row for a single call site — one `UPDATE`
on the original row plus N−1 `INSERT ... SELECT` copies:

| Tier | Location | Cap |
|---|---|---|
| Overload | `nexus-store/src/lib.rs:1047-1071` | ≤ 4 candidates |
| GraphQL coordinate | `nexus-store/src/lib.rs:1109-1131` | **uncapped** |
| Bare member name | `nexus-store/src/lib.rs:1185-1207` | ≤ 4 candidates |

A site with four candidates contributes four "resolved" edges of which at most one is
correct. **The headline number therefore rises as the resolver grows less certain.** The
GraphQL tier has no cap at all, so its contribution is unbounded by construction.

### 1.3 `scan` and `graph` already disagree with each other

`ResolveStats` (`nexus-store/src/lib.rs:140-158`) increments `ambiguous` **once per call
site**; `edge_counts` counts **N rows** for that same site. `scan` renders the first
(`nexus-core/src/engine/scan.rs:204-207`), `graph` renders the second
(`nexus-core/src/engine/query.rs:1081-1089`).

**Measured on a clean clone of `main` at `46e2fff`, one scan, one database:**

```
scan     edges 4458  (45% of 3606 in-project resolved, 852 external)
graph    edges 4603  ·  3751 in-project · 1784 resolved (48%)
```

Two commands, two answers. The 145-edge gap is exactly the fan-out rows the ambiguous tiers
insert during resolution: `scan` reports the pre-resolution site count, `graph` counts rows
afterwards. This is a live bug, independent of everything else in this document, and the two
numbers are the cheapest possible demonstration of §1.2.

### 1.4 The confidence constants are unvalidated

Every edge carries a `confidence`, which is a probability claim. The values are:

| Tier | Constant | Location |
|---|---|---|
| exact FQN | 1.00 | `lib.rs:1033` |
| GraphQL exact join | 0.95 | `lib.rs:1030` |
| GraphQL single coordinate | 0.95 | `lib.rs:1102` |
| unique prefix | 0.90 | `lib.rs:1044` |
| overload fan-out | 0.9 / n | `lib.rs:1054` |
| GraphQL fan-out | 1.0 / n | `lib.rs:1113` |
| inherited member | 0.85 | `lib.rs:1144` |
| unique simple name | 0.70 | `lib.rs:1157` |
| bare member name | 0.60 | `lib.rs:1181` |

None has ever been measured. `AGENTS.md:32` already concedes that "no ranking weight has
been tuned". `docs/architecture/11-risks.md` R8 names this exact failure — *"weights tuned
once by feel, never revisited"* — and guards it for the Context Engine ranker while leaving
the resolver unguarded.

**The largest contributor is one day old and has never been checked.** Commit `f07e2b1`
(2026-09-03, *"fix(resolve): let a call to a method find the method"*) added the `by_member`
tier, taking Rust in-project resolution from 23 % to 48 % and method bindings from 29 to 973.
It writes at `heuristic` 0.6 and includes a 2-to-4-candidate arm that inserts one row per
candidate. It accounts for roughly 987 of this repository's heuristic edges — the difference
between the pre- and post-`f07e2b1` breakdowns.

So the 48 % now published in both READMEs is **majority-produced by a tier introduced
yesterday, at a confidence nobody has validated, using the fan-out mechanism §1.2 shows
inflates the metric.** That is the single strongest argument for building this harness before
building anything else on top of the graph.

### 1.5 The repository already contradicts itself

Three different values were published for Rust resolution:

| Source | Claim | Audience | Verdict |
|---|---|---|---|
| `README.en.md:255`, `README.md:251` | "this repository resolves **48%**" | user-facing | **correct** — matches `nexus graph` |
| `AGENTS.md:32`, `docs/architecture/10-roadmap.md:416` | "**13 %**" (547/4,158) | internal | stale by 3.7×, predates the `by_member` tier |
| `AGENTS.md:159` | "Rust sat at **23%**" | internal | correct *as history* — past tense, pre-`by_member` |

Corrected 2026-09-03 to 48 % (1,784 / 3,751 at `46e2fff`), with provenance stamped at each
site. The root cause was not arithmetic: **no published number carried a "measured at"
marker**, so nobody could distinguish stale from wrong. Every corrected site now names the
commit it was measured on.

The documented tier table (`docs/architecture.md:251-257`, ADR-003 at
`docs/architecture-decisions.md:117-123`) says `heuristic` spans 0.70–0.95 and lists no
`contract` tier. The code has `contract` at 0.95 and `heuristic` reaching down to 0.60.

And the denominator is implemented **four times**: `edge_counts`, `ResolveStats::resolved()`
(`lib.rs:165-167`), `render.rs:241`, and `scripts/check_smoke.py:31-37`.

### 1.6 A decision gate is wired to the wrong quantity

`docs/architecture/12-non-goals.md:111-112` sets the trigger for building LSP sidecars at
*"measured impact **recall** below 85 % for a language"* and then satisfies it with
*"Current measurement: 96 % of in-project edges resolved. Not fired."*

Coverage is not recall. They are different quantities and this document defines both for the
first time. Meanwhile `docs/architecture/10-roadmap.md:416` records the same trigger as
**fired for Rust**. A major architectural commitment currently rests on a metric that has
never measured the thing its own trigger names.

---

## 2. What this design covers

**In scope.** An evaluation harness that measures the accuracy of edge resolution against a
compiler-grade oracle, and a replacement for the in-product coverage metric that cannot be
inflated by fan-out.

**Explicitly out of scope.** Changing the resolver. No new tier, no receiver-type inference,
no stack-graph rewrite. Those are the changes this harness exists to *evaluate*; designing
them now would mean choosing a remedy before measuring the disease. `docs/architecture/11-risks.md`
R8 is precisely the failure of skipping that order.

**Deliberately deferred, with the trigger stated.** Hand-labelled gold edges in the fixture
corpus (§9.1). Wait until the harness has shown where the resolver actually errs; labelling
against a guessed failure distribution wastes the labelling effort.

---

## 3. Placement and the product-side footprint

### 3.1 New crate `crates/nexus-eval`

A development tool. **It must not enter the shipped binary.** The oracle path pulls in
protobuf and the `scip` crate, and constraint 1's rule — the deterministic build carries no
analysis dependency it does not need — applies here as much as it does to an HTTP client.

It is the mirror of `nexus-fixtures`: fixtures generate repositories and must never mark
their own work; `nexus-eval` marks and must never generate. `crates/nexus-cli/tests/boundaries.rs`
gains one assertion, in the shape of the existing
`nothing_but_the_composition_root_depends_on_the_fixture_generator` (`boundaries.rs:257`):
**nothing in the workspace depends on `nexus-eval`.**

Added to `Cargo.toml` `members`. Not added to `workspace.dependencies`, because nothing may
depend on it.

Dependencies, with the pins forced by upstream:

```toml
scip     = "0.9"       # types + symbol grammar
protobuf = "=3.7.2"    # rust-protobuf, NOT prost — see §4.4
serde_json, clap, blake3
```

### 3.2 The one new thing Nexus exposes

`nexus graph` gains `--edges <path>`, writing **NDJSON**, one record per edge:

```json
{"src_fqn":"...","src_file":"...","site_line":42,"edge_type":"calls",
 "dst_fqn":"...","dst_file":"...","dst_start_line":88,"dst_end_line":103,
 "resolution":"heuristic","confidence":0.6}
```

**Not on stdout.** `crates/nexus-cli/tests/json_contract.rs:83-104` pins that a command emits
exactly one JSON document, and an edge array for a multi-million-line repository has no
business being buffered into it. stdout keeps its single summary document unchanged in shape.

This requires one new `Store` method (SQL stays in `nexus-store`, constraint 3 intact), one
`Engine` method, and one `render` arm. That is the entire footprint inside the shipped
product: one flag, one query.

---

## 4. The oracle

### 4.1 Indexers, pinned

| Language | Tool | Version | Build requirement |
|---|---|---|---|
| Rust | `rust-analyzer scip` | date tag `2026-08-31` (no semver) | working Cargo workspace; runs `cargo check` for `OUT_DIR`s |
| Java | `scip-java index` | 0.13.1 | **a successful compile** |
| TypeScript | `scip-typescript index` | 0.4.0 | `node_modules` installed |
| Python | `scip-python index` | 0.6.6 | virtualenv activated |

`scip` and `scip-java` moved from the `sourcegraph` org to **`scip-code`**: Maven coordinates
`org.scip-code:*`, Docker `ghcr.io/scip-code/scip-java`, main class
`org.scip_code.scip_java.ScipJava`. `scip-typescript` and `scip-python` remain under
`sourcegraph`.

Versions are recorded in every eval run's output, the way `scans.tool_versions_json` records
grammar versions. An oracle that changes silently is worse than no oracle — the identical
argument `AGENTS.md` makes under "Cache invalidation must include tool versions" and
`docs/performance.md` §5 makes about grammar upgrades.

### 4.2 The oracle relation

Build `symbol → definition position` from every occurrence whose `symbol_roles` has
`Definition = 0x1` set, **index-wide, not per-document** — a cross-file reference's
definition lives in a different `Document` of the same `Index`.

Every occurrence *without* that bit is a reference at `(path, line)`.

Two exclusions, both forced by the format rather than chosen:

- **`external_symbols` carries no position.** `SymbolInformation` has fields `symbol`,
  `documentation`, `relationships`, `kind`, `display_name`, `signature_documentation`,
  `enclosing_symbol` — no range, no document. A symbol defined outside the index is
  positionally unlocatable, so it cannot be compared. This is the same boundary ADR-017 drew
  for the same reason.
- **`local ` symbols are skipped.** The SCIP grammar reserves the `local ` prefix for
  entities local to one document, and `scip-typescript` and `scip-java` restart local
  numbering per file, so `local 3` names different things in different documents.
  (`rust-analyzer` happens to use an index-wide counter; relying on that would be relying on
  an implementation detail.) Function-scoped locals carry no cross-file edges by definition,
  so skipping them costs nothing.

### 4.3 The comparison unit is a call site

Key: `(src_path, site_line)`. **Not the edge.**

This is what makes fan-out visible instead of flattering. One site with four candidate edges
is one site with one correct answer and three wrong ones.

### 4.4 Matching is positional

A Nexus edge agrees with the oracle when the oracle's definition position falls **inside the
span** of the Nexus destination symbol — `dst_file` equal, `dst_start_line ≤ def_line ≤
dst_end_line` — with the innermost span winning when spans nest.

**No FQN string translation anywhere.** SCIP writes `mn/pay/PaymentService#createPayment().`
— `/` separators, `()` method suffix with a disambiguator, no parameter types. Nexus writes
`mn.pay.PaymentService#createPayment(String,Money)`. Every rule mapping one to the other is a
judgment call, and every judgment call is somewhere a nicer number could be manufactured.
Line numbers have no knobs.

Two mechanical requirements the reader must satisfy:

- **Read both range encodings.** `Occurrence.range` (repeated int32) is deprecated in the
  proto in favour of `oneof typed_range`, but current indexers still emit the legacy field.
  A reader that handles only `typed_range` silently sees zero occurrences.
- **`scip` 0.9.0 has no read helper** — its crate root exports only `write_message_to_file`.
  Reading requires `protobuf` as a direct dependency at exactly `=3.7.2`, so the generated
  types unify:

```rust
use protobuf::Message;
let index = scip::types::Index::parse_from_reader(&mut reader)?;
```

### 4.5 The comparable set, stated before any number is produced

Precision and recall are computed **only** over sites where all of the following hold:

1. the oracle records a reference whose definition is a function or method defined
   **in-project** (§4.2 makes any other target positionally unlocatable);
2. the site lies inside a symbol Nexus indexed;
3. the edge type is one SCIP can judge.

Excluded edge types, counted and reported separately, never folded into either numerator or
denominator: `calls_graphql`, `calls_http`, `renders`, `routes`, `persists`, `emits`, and any
edge with `resolution = 'framework'` or `'external-graph'`. SCIP has no opinion about a
GraphQL seam or a Spring bean wiring. Counting the oracle's blind spots as Nexus's errors is
the mistake ADR-017 already caught once, in the opposite direction.

---

## 5. The mathematics

### 5.1 Notation

`S` is the set of comparable call sites (§4.5). For `s ∈ S`, `D(s)` is the oracle's single
correct destination and `N(s)` is Nexus's candidate set — possibly empty, possibly larger
than one.

### 5.2 Four metrics, because no single one is honest

| Metric | Definition | What it punishes |
|---|---|---|
| **Recall** (site-level) | `\|{s : D(s) ∈ N(s)}\| / \|S\|` | missing the answer entirely |
| **Precision** (edge-level) | correct comparable edges / all emitted comparable edges | **fan-out** — 4 candidates, 1 right = 0.25 |
| **Strict site accuracy** | `\|{s : N(s) = {D(s)}}\| / \|S\|` | any ambiguity at all |
| **F1** | harmonic mean of recall and precision | — |

**Recall and precision are reported as a pair, always.** Recall alone is today's failure
mode: it is the number that rises when the resolver fans out.

Deliberately not computed: mean reciprocal rank. The four above already price ordering, and a
fifth number nobody reads is a number nobody checks.

### 5.3 Confidence intervals — Wilson, not normal

Per-tier sample sizes are small and accuracies sit near 1.0, exactly where the normal
approximation produces intervals extending past 100 %. For `k` correct of `n`, `p̂ = k/n`,
`z = 1.96`:

```
             p̂ + z²/2n                    z
center  =  ─────────────    half  =   ─────────  ·  sqrt( p̂(1−p̂)/n + z²/4n² )
              1 + z²/n                 1 + z²/n

interval = [ center − half , center + half ]
```

Every rate in §7 is reported with its Wilson interval. A rate without one is not reported.

### 5.4 Calibration

Each edge carries `confidence` `c(e)`, a claim that `P(correct) = c`. With oracle labels
`y(e) ∈ {0,1}`:

**Brier score.** `BS = (1/N) Σ (c(e) − y(e))²`.

The load-bearing property is that Brier is a **strictly proper scoring rule**: it is
minimised only by reporting one's true belief. The score cannot be improved by inflating
confidences to look decisive or deflating them to look cautious. That is what makes it safe
to track over time — it cannot be gamed by editing constants.

**Expected calibration error.** `ECE = Σ_b (n_b/N) · |acc(b) − conf(b)|`.

Bins are **not** equal-width. Nexus's confidence is a set of nine discrete constants (§1.4),
so **the bins are the tiers**. Calibration becomes nine independent hypothesis tests rather
than a smoothing exercise.

**Per-tier verdict, as a decision rule.** A tier is *miscalibrated* iff its claimed constant
falls outside the Wilson interval of its measured accuracy. Crisp, falsifiable, no judgment.

### 5.5 The corrected constant

Not raw `k/n`, which proposes `1.00` off a 12-for-12 run. The **Jeffreys posterior mean**
under a `Beta(½, ½)` prior:

```
ĉ = (k + ½) / (n + 1)
```

**Guard against overfitting a small corpus: a tier whose Wilson half-width exceeds 0.15 does
not get its constant changed.** It is reported as *insufficient evidence*. Under-powered
measurement must not launder itself into a config edit — that is R8 wearing a lab coat.

### 5.6 The ambiguous tier decomposes into two testable claims

`0.9 / n` asserts two things, and they are measured separately:

- **set-recall** — does the candidate set contain the truth? (the `0.9`)
- **within-set uniformity** — given a set of size `n`, is the correct candidate uniformly
  distributed across positions? (the `1/n`), tested by χ² against uniform.

If uniformity fails, the candidates are orderable and the flat split is discarding
information the resolver already has.

### 5.7 Statistical power

Half-width ≈ `z √(p(1−p)/n)`. At `p ≈ 0.8`:

| Target half-width | Sites required |
|---|---|
| ± 0.05 | n ≈ 246 |
| ± 0.03 | n ≈ 683 |

**The harness refuses to print a tier verdict below n = 100**, printing the interval and
`under-powered` instead.

---

## 6. Prerequisite fixes

Both are defects in their own right and both would corrupt the harness's output. Neither can
be deferred past it.

### 6.1 `Resolution::parse` silently mislabels two resolution classes

`crates/nexus-store/src/lib.rs:1340`:

```rust
resolution: Resolution::parse(&resolution).unwrap_or(Resolution::Heuristic),
```

The `Resolution` enum (`crates/nexus-types/src/lib.rs:219-229`) has variants `Exact |
Framework | Contract | Heuristic | External | Unresolved`. It has **no `Sibling` and no
`ExternalGraph`**, but both are real stored values, permitted by the CHECK constraint since
`migrations/0004_sibling_resolution.sql` and `0006_external_graph.sql` and proven to reach the
database by `crates/nexus-core/tests/monorepo_module.rs:104-108` and
`crates/nexus-core/tests/context_pipeline.rs:499-505`.

So every `sibling` and `external-graph` edge surfaced through graph traversal is **relabelled
`Heuristic`**. `edge_counts` dodges it by reading the raw TEXT column, which is why it has
gone unnoticed.

**The direction of the error is the harmful one.** Both `sibling` and `external-graph` mean
*"nobody resolved this against a symbol table"*, and both are reported as `heuristic`, which
claims a tier that did. `external-graph` carries a documented confidence ceiling of 0.5
(`nexus-core/src/graphify.rs:23`) specifically so an imported claim cannot outrank a parsed
edge — and then the tier label says `heuristic` anyway, defeating the ceiling's purpose at
the point where a reader decides how much to trust the edge.

This is load-bearing here: §5.4 bins calibration **by tier**. A contaminated `heuristic`
population yields a corrected constant computed over the wrong set of edges. It also means
`min_confidence` on impact chains has been reporting the wrong tier to users.

**Not visible in this repository.** Its own scan shows neither value — the defect surfaces on
a Java monorepo with sibling modules, and on any project with `[scan] resolution =
"external-graph"` enabled. Measurements taken with direct SQL against
`symbol_edges.resolution` bypass `parse` and are unaffected; only readers going through the
enum see the wrong tier.

**Fix:** add `Sibling` and `ExternalGraph` variants; replace `unwrap_or` with an explicit
error. A resolution string the enum does not know is a schema/code disagreement and must fail
loudly, not default to the middle of the range.

### 6.2 Site counting, replacing edge counting

`edge_counts` and `edges_by_resolution` both change to count **distinct call sites**, keyed
`(src_symbol_id, site_line, dst_fqn_hint)`. SQLite has no multi-column `COUNT(DISTINCT …)`,
so this is a `GROUP BY` subquery:

```sql
SELECT COUNT(*) FROM (
  SELECT 1 FROM symbol_edges
  WHERE project_id = ?1
  GROUP BY src_symbol_id, site_line, dst_fqn_hint
)
```

Every consumer reads `EdgeCounts` transitively, so **no signature changes**.

Three constraints on the change:

- **Both queries move or neither does.** `crates/nexus-core/tests/monorepo_module.rs:96-113`
  asserts `edges_sibling == by_resolution["sibling"]` — "the summary count and the stored rows
  must agree". Moving one breaks it.
- **`site_line` is nullable**, non-null for every analyzer-produced edge and NULL only for
  `external-graph` imports (`lib.rs:1971-1977`), whose `dst_fqn_hint` is also NULL. Those
  collapse to one group per source symbol. They are already excluded from `resolved`, so this
  affects only `total` and `by_resolution`, and must be handled explicitly rather than left to
  emerge.
- **The acceptance test is §1.3.** `scan` and `graph` must report the same resolution figure
  for the same project. `ResolveStats::resolved()` already counts sites, so this is the
  assertion that proves the two implementations converged.

---

## 7. The two report cards

### 7.1 `nexus graph` — in-product, no oracle available

```
edges       13467    in-project 4370    external 9514    sibling 0
sites        3128    resolved 2984  (95.4% coverage)
                     ambiguous  412  (13.2%)    fanout 1.31 edges/site
tiers       exact 1809 · contract 224 · supertype 198 · simple 241 · bare-member 402
note        coverage is not accuracy — run `make eval` for measured precision
```

Three changes, each load-bearing:

- **the denominator is sites**, so fan-out cannot inflate it;
- **fan-out is printed**, so what used to hide inside the number has its own line;
- **the word "coverage"** replaces every implication of correctness, with a pointer to where
  correctness is actually measured.

This is strictly better than the current line *with no oracle installed*, which is why it
ships in the product rather than in the harness.

### 7.2 `nexus-eval` — the measured report card

```
oracle      scip-java 0.13.1 · spring-petclinic @ <sha>
coverage    files: 312 of 312 indexed by oracle          (see §8.1)
comparable  2417 of 3128 sites   (711 excluded: 402 non-project target, 309 oracle-blind type)

precision   0.78  [0.76-0.80]        recall  0.91  [0.90-0.93]        F1  0.84
strict      0.71  [0.69-0.73]

calibration Brier 0.121    ECE 0.094

tier           claims    measured              n      verdict
exact           1.00     0.993 [0.988-0.996]   1809   ok
contract        0.95     0.962 [0.930-0.980]    224   ok
supertype       0.85     0.771 [0.710-0.822]    198   MISCALIBRATED -> 0.77
simple-name     0.70     0.612 [0.550-0.670]    241   MISCALIBRATED -> 0.62
bare-member     0.60     0.383 [0.337-0.431]    402   MISCALIBRATED -> 0.39
ambiguous       0.9/n    set-recall 0.88 · within-set uniform (chi2, p=0.41)
```

**Numbers are illustrative. The columns are the contract.** Emitted as JSON as well as this
rendering; the JSON is what the baseline comparison in §8.3 reads.

**Expect the first honest number to be worse than 96 %.** That is the harness working.

---

## 8. Determinism, failure, and testing the instrument

### 8.1 Oracle coverage cross-check — mandatory

Two of the four indexers degrade **silently**:

- `scip-typescript` skips any file over `--max-file-byte-size` (default **1 MB**) without
  saying so;
- `scip-python` is a pyright fork that deliberately does not abort on analysis timeout,
  emitting a partial index rather than failing.

A partial oracle **inflates precision**, because Nexus edges in unindexed files drop out of
the comparable set instead of being judged. The harness's failure mode would be a flattering
result — exactly what it exists to prevent.

**Therefore:** every file Nexus indexed must appear as a `Document` in the oracle index, and
the gap is reported as a first-class number beside precision (§7.2, `coverage` line). A run
whose file coverage is below 95 % is marked `partial` and its metrics are advisory.

### 8.2 Indexer failure is `inconclusive`, never zero

If `scip-java` cannot build the project, that says nothing about Nexus's resolver. This is the
existing rule — *"an infrastructure failure leaves confidence unchanged, never lowered"* —
applied one level up.

`scip-java` fails closed: a non-zero build exit propagates and `aggregate` never runs, so no
`index.scip` is written at all. But the javac plugin writes one `.scip` per source file into
`target/scip-targetroot` (Maven) or `build/scip-targetroot` (Gradle) as compilation proceeds.
**Recovery procedure:** on non-zero exit, run `scip-java aggregate <targetroot>` and mark the
resulting index `partial` per §8.1. In a multi-module Maven reactor this yields every module
before the failing one.

### 8.3 `make eval` is not `make check`

It needs four external toolchains and, for Java, a full project compile. Wiring that into the
commit path gets it disabled within a fortnight — `docs/architecture/13-evaluation.md` §2 says
so in as many words about Tier 2, and the reasoning transfers.

`make eval` runs on a schedule against a committed baseline JSON. **Regression is judged
against the confidence interval, not exact equality.** An exact-match gate would fail on
oracle noise and be muted within a month.

### 8.4 Oracle caching

Indexes are cached keyed by `(repo sha, indexer name, indexer version)` and never regenerated
silently. An oracle that drifts measures the oracle — the argument `nexus-fixtures` already
makes about fixtures that drift (`crates/nexus-fixtures/src/lib.rs:11-13`).

### 8.5 The instrument gets its own tests

`nexus-eval` is a measuring instrument. An unvalidated instrument is worse than none, because
it produces numbers people believe.

Required: a hand-built miniature `.scip` fixture with known-correct answers, a synthetic edge
dump, and assertions that precision, recall, Wilson bounds, Brier score and the Jeffreys
estimate equal values computed by hand in the test body. If the instrument cannot reproduce
arithmetic done on paper, nothing downstream of it means anything.

### 8.6 Corpus

Primary targets, both viable today:

- **`spring-petclinic`** — Maven, already cloned by `make smoke`, driven by `scip-java`. The
  realism control `docs/architecture/13-evaluation.md` §3 insists on, because it is a
  repository we did not author.
- **Nexus itself** — a Cargo workspace, driven by `rust-analyzer scip`. This is where the
  13 %-vs-23 %-vs-48 % contradiction (§1.5) gets settled.

The four generated fixtures (`spring-payments`, `next-storefront`, `acme-monorepo`,
`legacy-billing`) are secondary: small, so their per-tier intervals will mostly fail the
n = 100 floor of §5.7. They are useful for the instrument's own tests, not for headline
numbers.

---

## 9. Migration: what this invalidates

The metric change alters a number published in roughly twenty places. CLAUDE.md's rule — *"a
fact written in two places is a fact that will eventually disagree with itself"* — has already
been violated here (§1.5). This list is part of the deliverable, not an afterthought.

**Code (7 files):**
`nexus-store/src/lib.rs` (`edge_counts`, `edges_by_resolution`, `EdgeCounts` docs,
`Resolution::parse`), `nexus-types/src/lib.rs` (enum variants), `nexus-core/src/report.rs`
(field docs), `nexus-core/src/engine/query.rs:1081-1089` and `:144-151`,
`nexus-core/src/engine/scan.rs:204-207`, `nexus-cli/src/render.rs:236-267` and `:625-673`,
`nexus-mcp/src/lib.rs:488` (tool description string).

**A fourth denominator implementation:** `scripts/check_smoke.py:31-37` recomputes the
percentage itself in CI.

**Tests:** `nexus-core/tests/monorepo_module.rs:74-141`, `:96-113`, `:305-325`;
`nexus-core/tests/context_pipeline.rs:476-510`; `nexus-core/tests/call_resolution.rs:130-136`.
`nexus-store` has no `tests/` directory and its inline `mod tests` never touches
`edge_counts`, so the new SQL lands with zero existing coverage.

**User-facing documentation:** `README.en.md:253-256`, `README.md:251-252` (both carry the
contradicted 48 %).

**Internal documentation:** `AGENTS.md:32`, `:159`, `:256-257`;
`docs/architecture-decisions.md:772`, `:784`, `:795`, `:806-808`, `:820-821`, and ADR-003's
tier table at `:117-123`; `docs/architecture.md:207`, `:251-257`;
`docs/architecture/03-current-state.md:68-71`; `docs/architecture/10-roadmap.md:376`, `:416`;
`docs/architecture/12-non-goals.md:108-112`; `docs/roadmap.md:31-32`;
`docs/performance.md:133`.

**Agent-facing prose carrying a threshold that loses meaning:**
`commands/nexus-status.md:7` ("Below ~80% means impact results are…"),
`commands/nexus-scan.md:9`, `integrations/README.md:47`.

**ADR-017 needs a revision section**, not a rewrite. Its argument — that the denominator must
exclude what was never in scope — is correct and this design extends it. What changes is the
*unit* of the numerator and denominator, from edges to sites.

**A new ADR is required** for the coverage/accuracy split: that `nexus graph` reports coverage,
that coverage is explicitly not accuracy, and that accuracy is measured out-of-band against an
oracle. Numbering follows `docs/architecture/decisions/`.

---

## 10. Deferred, with triggers

| Item | Trigger to build it |
|---|---|
| **Gold edges in fixtures** | the SCIP oracle's noise floor proves too high on ≥ 2 languages, or a tier's true accuracy is needed below the n = 100 floor |
| **Receiver-type inference** for `obj.foo()` | bare-member tier measures below its claimed 0.60 — §1.4's most-suspected constant |
| **Retiring the ambiguous fan-out** | precision loss from fan-out exceeds recall gain, measurable directly from §5.2 and §5.6 |
| **Scope/stack-graph resolver** | the tier ladder proves unfixable rather than merely miscalibrated; needs decomposition into several specs |
| **Splitting `nexus-store/src/lib.rs`** | it is 3,607 lines and growing; AGENTS.md already notes it "should expose filtered views rather than raw tables". Out of scope here — noted so it is not rediscovered |

---

## 11. References

**Theory.**
Néron, Tolmach, Visser & Wachsmuth, *A Theory of Name Resolution*, ESOP 2015 —
<https://eelcovisser.org/publications/2015/NeronTVW15.pdf>. The resolution calculus that the
current tier ladder is an unprincipled approximation of.

Creager & van Antwerpen, *Stack graphs: Name resolution at scale*, EVCS 2023 —
<https://arxiv.org/abs/2211.01224>. Scope graphs made file-incremental: each file compiles to
an isolated subgraph with no visibility into other files, and resolution is path-finding over
the union. This is constraint 4 and the rescan model, arrived at independently. Powers
GitHub's Precise Code Navigation.

**Methodology.**
*On the recall of static call graph construction in practice*, ICSE 2020 —
<https://dl.acm.org/doi/10.1145/3377811.3380441>. Median recall **0.884** for standard static
call-graph analyses against a dynamic oracle; establishes the precision/recall-against-oracle
protocol.

Reif et al., *Judge: identifying, understanding, and evaluating sources of unsoundness in call
graphs*, ISSTA 2019 —
<https://www.researchgate.net/publication/334407623_Judge_identifying_understanding_and_evaluating_sources_of_unsoundness_in_call_graphs>.
Catalogues *why* static call graphs miss edges.

**Format.**
SCIP protocol — <https://scip-code.org/>, proto at
<https://github.com/scip-code/scip/blob/main/scip.proto>, Rust bindings at
<https://crates.io/crates/scip>.

**Internal.**
`docs/architecture/13-evaluation.md` (Tiers 1 and 2, which this extends downward),
`docs/architecture/11-risks.md` R8, ADR-003, ADR-017.
