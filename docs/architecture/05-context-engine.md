# Context Engine

The spine. One call in, one ranked and budgeted package out, with a full account of why.

Lives at `nexus-core::context` — the location [`memory-model.md`](../memory-model.md) §3 already
named. It is a module, not a crate: it needs the Engine's queries and adds no new dependency.

---

## 1. The pipeline

```
TaskRequest
   │
   ├─ 1 intent      deterministic classification of what is being asked
   ├─ 2 seeds       resolve the task to concrete symbols and files
   ├─ 3 expand      bounded graph traversal from the seeds
   ├─ 4 signals     attach git, test, finding, fact and profile evidence
   ├─ 5 rank        one scoring function over every candidate
   ├─ 6 budget      select under a token ceiling, by density, with a diversity guard
   └─ 7 package     emit items + the inclusion ledger
                            │
                       ContextPackage
```

No stage calls a model. The whole pipeline is queries, arithmetic and a sort.

---

## 2. Input and output

```rust
pub struct TaskRequest {
    /// The developer's prompt, or a command's subject. May be empty for --session.
    pub text: String,
    /// Explicit anchors when the caller has them (a hook knows the edited file).
    pub files: Vec<String>,
    pub symbols: Vec<String>,
    pub budget_tokens: usize,       // default 4000; --session defaults to 800
    pub purpose: Purpose,           // Session | Task | Review | Debug | Verify
}

pub struct ContextPackage {
    pub project: ProjectSummary,    // what this is: languages, frameworks, build, datastores
    pub items: Vec<ContextItem>,    // ranked, already within budget
    pub ledger: InclusionLedger,    // why each candidate is in or out
    pub tokens_estimated: usize,
    pub items_considered: usize,
    pub items_included: usize,
    pub since: Option<ScanUid>,     // set on a delta package
}

pub struct ContextItem {
    pub kind: ItemKind,   // Symbol | File | Finding | Fact | Change | Test | Decision
    pub anchor: CodeRef,  // file:line — always present, no exceptions
    pub window: Option<String>,  // at most WINDOW_LINES lines. Never a whole file.
    pub score: f64,
    pub terms: ScoreTerms,       // every weighted term, individually
    pub why: &'static str,       // one clause, human-readable
}
```

`ContextItem` carries **anchors, not contents**. Principle 3: Nexus says where. The agent has
`Read` and can pull more; paying to ship bytes it already has access to is paying twice.

---

## 3. Stage 1 — Intent

Deterministic. A verb table and a target extractor, not a classifier and certainly not a model.

| Signal in the text | Intent | Effect downstream |
|---|---|---|
| `fix`, `bug`, `broken`, `fails`, `error`, a stack trace | `Debug` | Findings and history weighted up; forward impact down |
| `add`, `implement`, `build`, `support` | `Build` | Conventions, sibling implementations, architecture facts weighted up |
| `refactor`, `rename`, `move`, `extract` | `Refactor` | Reverse impact and coverage dominate |
| `review`, `check`, `is this safe`, `done` | `Review` | Changed set only; coverage and seam crossings dominate |
| `why`, `how does`, `what is`, `explain` | `Explain` | Decisions, ADRs, facts weighted up; churn down |
| nothing matches | `Unknown` | Balanced weights, and the package **says** the intent was not determined |

`Unknown` is a first-class outcome, not a default dressed up as a guess. In the face of
ambiguity, the package reports that it guessed nothing and falls back to balanced weights —
which is honest, and measurably better than a template classification that is wrong 30 % of the
time and never says so.

**Never ask a model to classify intent.** It costs a round trip and a token bill to replace a
table lookup, and it introduces non-determinism into the one part of the system that must be
reproducible for a package to be explainable.

## 4. Stage 2 — Seeds

Seeds are resolved in priority order, and every seed records *how* it was found:

1. **Explicit** — `files`/`symbols` on the request. A hook editing `PaymentService.java` knows.
2. **Exact FQN or path** in the text.
3. **Changed set** — for `Review`/`Verify`, the symbols the current rescan reports. Free: the
   cascade already computed it.
4. **Symbol name match** — `idx_symbols_name`, exact then prefix.
5. **Text match** — `ui_strings` (once populated) for a user-visible label, including non-English
   i18n values. This is the strongest signal for a bug report and today the table is empty.
6. **Fact subject match** — the task names a module a fact is about.

Zero seeds is a legitimate result and is reported as such. A package built from nothing is
worse than an empty package plus "I could not anchor this to the code" — the second lets the
agent ask a better question; the first sends it confidently into the wrong module.

## 5. Stage 3 — Expand

`impact::run`, reused unchanged. Direction follows intent: `Reverse` for `Refactor`/`Review`
(who breaks), `Forward` for `Debug` (what this reaches), both for `Explain`.

Bounds are the existing ones — `max_depth`, `min_score`, `fan_out_cap` — and the existing
reporting of having been capped. Every expanded candidate inherits its `Hop` chain and
`min_confidence`, which is what makes an item's presence in the package *provable* rather than
asserted.

## 6. Stage 5 — Ranking

One function. Every term recorded per candidate.

```
score(item) =   w_seed  · seed_proximity        // 1.0 at a seed; the impact score otherwise
              + w_graph · graph_score           // Π edge_weight × confidence along the chain
              + w_churn · churn                 // log1p(commits touching it in window) / log1p(max)
              + w_recent· recency               // exp(-age_days / half_life)
              + w_hist  · prior_findings        // REGRESSED 1.0, VERIFIED 0.8, UNVERIFIED 0.5, IGNORED 0
              + w_fact  · fact_relevance        // subject_match × source_weight × confidence
              + w_test  · test_relevance        // covers a seed, or is a seed nothing covers
              + w_arch  · arch_relevance        // named in a decision fact or a profile anchor
              − w_cost  · token_cost_norm       // estimated tokens / budget
```

Sub-terms, all deterministic:

- `subject_match`: exact FQN 1.0, module prefix 0.6, project 0.3 — as `memory-model.md` §3
  already specifies.
- `source_weight`: human 1.0, deterministic 0.9, ai 0.7 — likewise.
- `recency` half-life: 30 days, configurable.
- `token_cost`: `bytes / 3.5`, the same estimator `budget::fit` uses. It is an estimate and the
  package says so; a real tokenizer is a dependency bought for a rounding error.

Weights live in `.nexus/policy.toml` with documented defaults. They are **data, not code**, so
tuning them is a config change and a re-run, not a release.

### Why one formula

Because a ranker assembled from special cases cannot be debugged. With a single weighted sum,
every surprising inclusion decomposes into terms you can read and a weight you can change.
Special-case rules ("always include the controller") are how a ranker becomes folklore.

## 7. Stage 6 — Budget

Selection, not truncation.

1. Sort by **density** — `score / token_cost` — not by raw score. A 40-token fact that scores
   0.6 beats a 900-token class that scores 0.9. This is where the token optimisation actually
   happens.
2. Fill greedily to `budget_tokens`.
3. **Diversity guard**: at most `MAX_PER_COMPONENT` items (default 3) from one class or file
   before the next component gets a turn. Without it a hot class fills the whole budget with
   its own methods and the package describes one file instead of one change.
4. **Floor**: items below `min_score` are excluded even when budget remains. An unfilled budget
   is not a problem to solve. Padding a package with weak items is the exact behaviour the
   core principle forbids.

`budget::fit` in `nexus-mcp` stays, demoted to a last-resort guard against a serialisation
surprise. It should never fire once selection works, and if it does, that is a bug in the
budgeter — so its firing is logged, not silent.

## 8. Explainability

Mandatory output, not a debug flag.

```
$ nexus context --task "fix payment idempotency" --explain
included  0.91  mn.pay.PaymentController#createPayment   seed: exact name match
                seed 1.00 · graph 0.00 · churn 0.31 · hist 0.80 · fact 0.72 · cost -0.04
included  0.84  fact arch.payment.idempotency            subject match on mn.pay (human)
included  0.77  mn.pay.PaymentService#pay                graph: calls ← controller (0.9, resolved)
excluded  0.71  mn.pay.PaymentRepository#save            budget exhausted at 4000 tokens
excluded  0.44  mn.pay.PaymentMapper#toDto               below floor (0.50)
excluded    —   fact risk.payment.locking                invalidated at scan 41 (evidence moved)
excluded    —   src/main/resources/application.yml       redacted: path deny-list

considered 61 · included 12 · 3,780 of 4,000 tokens
```

Two questions the engine must always answer — *why was this included* and *why was this
excluded* — are answerable because the ledger records the decision for every candidate, not
only the winners. A ranker that only explains its inclusions cannot be debugged for the failure
that matters most: the right file that never made it in.

## 9. Compression

Compression is what you do **after** selection, and only where selection cannot go further. It is
a smaller lever than the budget and it is applied in this order:

1. **Deduplicate.** The same symbol reached by three edge chains is one item with three paths, not
   three items. Free, and it is the largest single saving on a dense graph.
2. **Collapse siblings.** Six methods of one class, all above the floor, become one class item
   listing six member names with their lines. Preserves the information; drops the repetition of
   the enclosing context.
3. **Trim the window.** The default 3-line window shrinks to 1 (the signature) for items in the
   lower half of the package. The anchor is the payload; the window is a convenience.
4. **Path elision.** A `Hop` chain longer than 3 renders as `A → … (2 hops) → D` with the weakest
   confidence retained, because the weakest link is the only part of a long chain that changes a
   decision.

Explicitly **not** doing:

- **LLM summarisation of context.** Paying a model to shrink what you send a model is paying twice
  and losing fidelity in the middle. It also makes the package irreproducible, which destroys
  §8.
- **Lossy paraphrase of evidence.** An anchor is `file:line` or it is not evidence. There is
  nothing to compress in a line number.

If compression is doing heavy lifting, the budgeter is failing — the right response is a better
selection, not a better squeeze.

## 10. Freshness

A package describes a moment. Three mechanisms keep it honest about which one.

**Staleness is structural, not temporal.** Nothing expires on a clock. A package is stale when
the code it describes moves, which is observable:

| Signal | Meaning |
|---|---|
| `HEAD` sha | commits landed |
| dirty hash | uncommitted edits exist |
| `scan_uid` | the index the package was built from |
| `weights_hash` | the ranking policy that produced it |

All four are in the **package cache key**, so a change to any of them is a miss rather than a
stale hit. The dirty hash is the one that matters most in practice: an agent editing files
uncommitted is the normal case, and a cache keyed only on `HEAD` would serve context describing
code that no longer exists (R9).

**Delta packages.** After the first package in a session, `--since <scan_uid>` returns only what
changed plus what the change newly reaches. The steady-state cost of staying informed is
proportional to the edit, not to the project — which is what makes the `PostToolUse` rescan hook
affordable.

**Facts carry their own freshness.** A fact whose evidence symbol has moved is *invalidated*, not
down-weighted, and never appears in a package
([`06-memory.md`](06-memory.md) §3). Stale memory is the failure that reads as authority, so it
is excluded rather than discounted.

**Every package states its basis.** `ContextPackage` carries the `scan_uid`, the `HEAD` sha and a
`dirty: bool`. An agent reasoning from a package can tell what it was true of, and a package
built on a dirty tree says so rather than implying a clean one.

## 11. Caching

- **Package cache** keyed on `(intent, seed set, HEAD sha, dirty hash, budget, weights hash)`.
  An identical question inside one session costs one lookup. Any component changing invalidates
  it; the dirty hash is what makes it safe on an uncommitted tree.

## 12. What the Context Engine must never do

- Call a model. Any stage. Ever.
- Return a whole file.
- Include an item with no `file:line` anchor.
- Fill remaining budget with low-scoring items.
- Silently truncate — every omission is a ledger row.
- Know which agent is asking. Purpose is a parameter; the caller's identity is not a concept here.
