# Knowledge Selectivity — Design

**Status:** approved; **absorbed into** [`2026-09-03-retrieval-design.md`](2026-09-03-retrieval-design.md),
which is what gets built. This document remains the record of what was measured about
graphify's prose and the four prose-to-code joins that carry no signal.
**Date:** 2026-09-03
**Supersedes nothing.** Corrects a regression introduced by `012782d`
(`feat(memory): import an external graph's knowledge, not just its edges`).

## The problem

`nexus memory import` reads graphify's semantic pass and records every `concept` and
`rationale` node as a fact. On this repository that is 678 facts, and it made the session
package worse:

```
session package before import:   0 considered,  0 included,  194 tokens
session package after import:  671 considered,  9 included,  752 tokens
```

The nine it bought are `arch.34-000-tokens…`, `arch.a-dismissal…`, `arch.a-reformat…`,
`arch.a-typical…`, `arch.acme-monorepo…` — **chosen alphabetically**. Every session now pays
558 extra tokens for the first nine claims by name.

Two independent defects produce that.

### Defect 1 — a heading is imported as if it were a claim

graphify's `concept` nodes are mostly *names of things*, not assertions. Of 681 prose nodes,
the recorded facts include `next`, `react`, `Golden Fixture Repositories`, `acme-monorepo
Fixture`. Of 678 recorded, 641 are file-scoped — they name no symbol at all. Thirteen come
from `tests/fixtures/**` blobs, which are test data rather than this project's design.

### Defect 2 — the session package treats a stable order as a priority order

`Store::facts` is `ORDER BY fact_key`, and its own comment says so:

> Ordered by key alone: stable, so a caller can rely on it, and *not* a ranking.

`Engine::session_package` consumes that order directly with `Selection::Ordered`. With a
handful of hand-written facts the distinction never showed. With hundreds of imported ones it
is the whole behaviour.

The function already promises the right thing and does not do it. Its doc comment says
"durable facts". Its `basis.selection` string says `"phase-1 fixed query: open findings then
durable facts, in store order"`. [`06-memory.md`](../../architecture/06-memory.md) §3 gives
`Durable` the highest retrieval weight. The code carries the reason for the gap:

> The lifecycle states are Phase 3.1, so "durable" is approximated by the order the store
> already returns […] The approximation gets better when the lifecycle lands; it does not
> get unwound.

Phase 3 landed. The approximation was never unwound.

## What was ruled out, and why

Four ways to join a prose claim to the code it is about were measured against this
repository's own graph and index. Three carry no usable signal. They are recorded here so
nobody spends the week finding out again.

| Candidate join | Measurement | Verdict |
|---|---|---|
| graphify's own prose→code edges | 13 of 6,727 links | Its two passes were never joined |
| Shared louvain community | 50 of 681 prose nodes, and the sampled communities resolve to empty code sets | No signal |
| The document's dominant crate | 9 of 68 documents clear a 50% share, and all nine are dominated by `nexus_core`, which is 60% of the codebase | A base rate, not a signal |
| The claim's own text, located in its document | 262 of 641 locatable; the rest are model paraphrases appearing nowhere | Reaches 41% at best |

**Conclusion: there is no honest structural join.** The 31 claims that anchored on a symbol
(30 after duplicate keys collapsed) did so because they *name* one. Inference does not recover the rest, and a design that
pretends otherwise anchors design claims on whatever symbol happens to end with the right
word — the failure that put six of them on `NoContinuousIntegration` before the last change
tightened subject resolution.

This design therefore does not try to anchor more claims. It stops importing what is not a
claim, and it makes the session package deliver what it already says it delivers.

## Design

### 1 — Import only what is a claim

`Engine::import_graphify` keeps a prose node when **any** of these holds:

| Rule | Basis | Matches |
|---|---|---|
| `file_type == "rationale"` | An assertion by construction | 267 |
| a `concept` that some node is a `rationale_for` | The thing being justified | 27 |
| a `concept` whose label reads as a sentence | ≥ 4 words, ≥ 2 of them lowercase-initial | 123 |
| a `concept` that names an indexed symbol | The existing `symbol_named_in` test | 31 |

and **never** when `source_file` starts with `tests/` or contains `/blobs/`.

Rules overlap. Rules 1–3 minus fixtures keep **408** of 681 exactly; rule 4 adds roughly
**19** more that the first three miss, for a union near **427**. The 408 is measured; the 19
is measured with a proxy for `symbol_named_in`, so the implementation should report the real
figure rather than assert this one.

Kept: *"Budget is selection, not truncation"*, *"stdout carries results, stderr carries
everything else"*, *"Truncation is reported, never silent"*.
Dropped: *"next"*, *"react"*, *"Golden Fixture Repositories"*, *"acme-monorepo Fixture"*.

**The fourth rule is not redundant.** Rules 1–3 alone drop `nexus-mcp::budget::fit` and
`nexus-cli::main composition root` — headings by shape, but they name code, which is the
entire reason to keep a claim.

**The third rule is a guess about English and is expected to be wrong sometimes.** It is kept
deliberately: without it the import loses claims identifiable only as prose, and those are the
ones with the most to say. It errs in both directions and the store stays broad enough that
`nexus ask` still finds most things.

`ImportReport` gains a `skipped_not_a_claim: usize` alongside the existing `skipped`, so the
filter's effect is visible in the command's own output rather than only in a test.

### 2 — The session package carries durable facts

`Engine::session_package` filters its fact candidates to `durable == true`. Nothing else about
the session path changes: the selection stays `Ordered`, findings still come first, and the
basis string finally describes what happens.

This is not a rule invented for graphify. It is §3's table, applied:

| State | Enters the session package? |
|---|---|
| Candidate (`durable = 0`) | No |
| Validated (`durable = 0`, `validated_count ≥ 1`) | No |
| Durable (`validated_count ≥ 3`, or `source = 'human'`) | Yes |

An imported claim arrives as a candidate and earns its place by surviving three scans with its
evidence anchor intact — the promotion arm that already exists in
`Store::validate_facts`. A human fact is durable on arrival and is unaffected.

**Three scans, no exception.** No hand-promotion path is added: `source = 'human'` already
means "a person vouched for this", and `nexus fact` already writes it.

### 3 — Task packages keep candidates, deliberately

The asymmetry is the point.

A **task** package is about relevance to a symbol the request named. §3's table says a
candidate fact is retrieved and *marked as* a candidate — the `[ai]` in the rendered item text
is that marker. A claim about the symbol under the cursor is worth reading even unverified.

A **session** package is what a session starts from with no task in hand. Starting from
unverified guesses, ranked alphabetically, is precisely the trap §3's `Observed → dropped`
edge exists to prevent.

No change to `task_package`.

## Consequences

| | Before this change | After |
|---|---|---|
| Session package, fresh import | 752 tokens, 9 alphabetical claims | ~194 tokens, no unearned claims |
| Session package, after 3 clean scans | unchanged | claims that survived enter, highest-weight |
| Task package naming `SafeWriter` | 720 tokens, 3 claims | unchanged |
| Facts recorded from this repo's graph | 678 recorded, 671 live | ~427 recorded |
| `nexus ask facts` | 671 rows, `next` and `react` among them | ~420, all claim-shaped |

Existing tests are unaffected: all three fact-recording tests in `session_context.rs` use
`source: "human"`, which is durable on arrival.

## Acceptance criteria

1. A graphify node labelled `next`, `Golden Fixture Repositories` or `acme-monorepo Fixture`
   is not recorded as a fact. A node labelled `Budget is selection, not truncation` is.
2. No fact is recorded from a node whose `source_file` is under `tests/` or contains
   `/blobs/`.
3. A `concept` node that names an indexed symbol is recorded even when its label is not a
   sentence.
4. `ImportReport.skipped_not_a_claim` counts the difference and the CLI prints it.
5. On a fixture where every fact is an imported candidate, the session package contains no
   facts and its token count is within 10% of the same project before the import.
6. A fact promoted to `durable` — three surviving scans, or `source = 'human'` — does appear
   in the session package.
7. A task package naming a symbol with candidate facts about it still carries them, capped by
   the existing diversity guard.
8. `make check` green; the golden packages do not move, because no fixture has a non-durable
   fact.

## Open, and deliberately not solved here

**Anchoring the ~390 claims that name no symbol.** Measured above: no structural join exists.
They remain file-scoped facts, retrievable by `nexus ask` and by a task that names their
document, and invisible to a task that does not. Closing this needs a claim to acquire an
anchor through *use* — a session that reads a claim and records which symbol it was about —
which is a separate design and overlaps "letting a claim earn confidence".

**Nothing verifies an imported claim against the code.** It stays at confidence 0.5 forever
unless a person or a verification run says otherwise. Survival of three scans makes it durable
without making it *true* — a fact whose anchor never moves is unfalsified, not confirmed. That
distinction deserves its own design.
