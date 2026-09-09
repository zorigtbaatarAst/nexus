# The seeding gate — evaluated

Pre-registered in [`2026-09-09-seeding-reads-prose-design.md`](../superpowers/specs/2026-09-09-seeding-reads-prose-design.md)
§6, before any of C1–C3 was written:

> The Tier 2 sweep is re-run only if the oracle's site recall rises on at least **3 of the 5**
> tokio tasks, relative to the committed baseline (R1 1.0, R2–R5 0.0).

**One of five rose. The gate is not met. No Tier 2 sweep follows.**

## The count

| task | baseline | after C1–C3 | direction |
|---|---:|---:|---|
| R1 stream-map size-hint overflow | 1.0 | 1.0 | unchanged |
| R2 lines-codec invalid UTF-8 | 0.0 | 0.0 | unchanged |
| R3 framed spurious decode | 0.0 | 0.0 | unchanged |
| R4 abstract-socket leading NUL | 0.0 | 0.0 | unchanged |
| R5 semaphore reopens after forget | 0.0 | **1.0** | **rose** |

Baseline is `git show 749f50f:crates/nexus-core/tests/golden/retrieval_tokio.json`, the state
before Task 2 touched anything. After-state is the committed
`crates/nexus-core/tests/golden/retrieval_tokio.json` at HEAD. Neither file moved in this task —
this is a reading, not a re-baseline.

**Count of tasks whose recall rose: 1 (R5).** R1 was already at its ceiling and stayed there;
"unchanged at 1.0" is not a rise, and the gate's condition is about tasks that moved, not tasks
that already passed. 1 is short of the required 3 by 2 whole tasks — not a near miss on a
continuous metric, a binary count that landed at a third of the bar.

**Verdict: the gate is not met. Per §6, the correct outcome is a written negative result and no
spend, which is what this document is.** "Retrieval that did not improve cannot produce an
end-to-end cost improvement, and paying $29 to confirm a number already in hand buys nothing" —
that reasoning was written before this run's numbers existed and it still holds now that they
do. This is not a failure of the task; it is the gate doing the job it was built for.

## What each change actually did

### C1 — code shape, not sentence position: this is the one that worked

`is_plain_word` disqualified any word with a leading capital, on the theory that such a word
"was typed *as* code." True of `StreamMap`, false of the first word of every English sentence,
and all five tokio prompts open with one (`Summing`, `Feeding`, `Rebuilding`, `Binding`,
`After`). The fix — test for *interior* evidence of a naming convention instead of position —
turned out to live in **two places**, not one: `is_plain_word` itself, and a second, independent
copy of the same leading-capital test inside `targets()`. Task 2 fixed the first. Fixing only
that achieved nothing, because the second copy still disqualified `After` on its own. Fixing
both — Task 2's later commit, `225cea1`'s predecessor `566dbe0` closing the second site — is
what took **R5 from 0.0 to 1.0**, landing `tokio/src/sync/batch_semaphore.rs` at rank 3.

The mechanism, confirmed by reading `--explain` before and after: R5's prompt opens "**After** a
semaphore has been closed, forgetting permits…". Before the fix, `After` was the only word that
seeded (`STOPWORDS` holds `after`, lowercase, and never sees `After` because the leading-capital
check routes it around the stopword list entirely), and it seeded `reset_after` and
`NOTIFY_AFTER` — real symbols, wrong file. That wrong anchor won a place in the package and
crowded out room for the correct one during graph expansion. With `After` correctly read as
prose and stopped, `semaphore` reaches the target list and anchors the right file.

### C2 — the family cap becomes a fraction of the index: correctly built, inert for the words it was written for

Implemented exactly as specified: `max(6, ⌈0.01 × symbol_count⌉)`. At tokio's 8,470 live
symbols the cap is 85 (measured against the fixture's actual live-symbol count, 8,027, the cap
is 81) — comfortably above `semaphore`'s 20 matches and `framed`'s 11, both of which the old
fixed cap of 6 deleted outright.

**And it changed nothing for either word**, because neither reaches the cap at all.
`exactly_named` matches case-sensitively on a symbol's last path segment, and in the tokio index
`semaphore` has **three** exact matches — `Tx#semaphore`, `Rx#semaphore`,
`OwnedSemaphorePermit#semaphore` — and `framed` has **two** — `Decoder#framed`,
`codec::framed`. `targets()`'s match arms are `[]` (nothing), `[only]` (exactly one — seeds at
full strength), and `[_, _, ..]` (two or more — the *ambiguous* arm, which seeds nothing and
never calls `token_family` or consults any cap). Two or three exact matches both land in that
last arm. The cap Task 3 built is real, correctly derived, and covered by its own test
(`family_cap(39) == 6`, `family_cap(8_470) == 85`) — it simply never gets consulted for the two
words the spec's own D2 used as its motivating example.

**The spec's diagnosis was wrong about which rule binds.** §2 D2 says the cap "makes
`token_family` return nothing when a word is a token of more than six names" and names
`semaphore` (20) and `framed` (11) as the words it deletes. Both words are in fact deleted by
the ambiguous-exact-match arm, upstream of `token_family` and upstream of any cap — fixing the
cap cannot reach a word that never gets that far. See the correction filed against the spec
itself, below. C2 was still kept: it is not wrong, and it still raises the ceiling for whatever
words do reach `token_family` with more than six family members on this corpus.

### C3 — seed strength follows evidence strength: mechanically correct, did not move R2

Implemented as `SeedStrength` (`ProseToken` 0.3, `ProseExact` 0.6, `CodeShape` 1.0), replacing
the flat `seed_proximity: 1.0` every seed got regardless of how it was found. Read on R2's
prompt directly, with `--explain` (Task 4 Step 6, run against the tokio fixture at commit
`a75c989`):

```
  included   0.70  tokio::time::error::Error#invalid            selected
            seed 0.60 · graph 0.00 · churn 0.10 · recency 0.00 · hist 0.00 · fact 0.00 · test 0.00 · arch 0.00 · cost -0.00
```

`Error#invalid` fell from seed strength 1.00 to 0.60, and its total score from 1.10/1.01
(itself and its container) to 0.70/0.61 — first place in the package to **fourth**. Three items
now outscore it (0.78, 0.74, 0.74) where nothing did before. The grading is real and it is
visible.

**It still outranks the only item from the right directory.** The sole `tokio-util/src/codec/`
item anywhere in R2's 34-item package is `tokio_util::codec::encoder::Encoder` at 0.55 — and it
did not arrive as a seed at all; it arrived at depth 1 via an `implements` edge from a *test*
helper (`SliceEncoder`) that the prose word `slice` seeded. 0.70 (`Error#invalid`, demoted) is
still greater than 0.55 (the one correct-directory item, unseeded and reached by accident).
Demoting the wrong anchor cannot promote a right one past it when nothing right was seeded to
begin with — there is nothing for the demotion to work on. Two reasons, both confirmed by
reading the code, not inferred from the score:

1. **Nothing from `lines_codec.rs` or `framed_read.rs` is seeded at all.** `framed` and `codec`
   both fail to seed (the former via the same ambiguous-exact-match arm C2 cannot reach;
   `codec`/`decoder` are non-stopwords that evidently name no symbol uniquely either). Weighting
   can reorder a candidate set. It cannot add to one that is missing the right member.
2. **Expansion discards seed strength entirely before it reaches ranking.** `engine/query.rs`
   hands `expand::run` the seed slice, and `context/expand.rs:31` maps each `Seed` down to
   `s.symbol.clone()` before `impact::run` ever sees it. A 0.3-strength `ProseToken` seed's graph
   neighbours enter the candidate set at full graph score, undiminished by how weak the seed that
   reached them was. That is how `Encoder` scored 0.55 in the first place: the `slice` seed is
   `ProseToken` (0.3), but its neighbour's `graph 0.48` term carries no memory of that.

R2's recall stayed 0.0. C3 met its own acceptance test (the ordering it introduced is real and
covered) and did not move the number it was written hoping to move, and the report that
implemented it said so at the time rather than after the fact.

### The remaining blocker, now precisely located

Three independent investigations (C1's fix, C2's trace, C3's `--explain` reading) converge on
one mechanism: **the ambiguous-exact-match arm.** A prose word matching two or more symbols by
exact last-path-segment name — `semaphore` (3 matches), `framed` (2 matches) — seeds *nothing*,
not a weakly-graded something. It is upstream of the family cap (so C2 cannot widen it) and
upstream of seed strength (so C3 cannot demote something into its place, because nothing is
there). Whatever fixes R2, R3 and R4 next has to change what that arm does, not how much a
family cap admits or how a seed is weighted once found.

## Hook latency — the other axis

Full detail, method, and per-repository breakdown: [`hook-latency.md`](hook-latency.md)
§"Re-measured, 2026-09-09."

Summary: spring-boot's `UserPromptSubmit` was already over budget (302 ms against 150 ms)
before this branch. It is worse now — 380 ms on a prompt naming a symbol, and a symptom-only
prompt that used to fall back to a 694 ms lexical scan now seeds broadly under the widened
family cap and costs **1,016 ms**, more than the lexical path it replaced. That is a further
breach on the repository the spec's §7 named as the one that decides, and acceptance criterion
5 in the design doc — "spring-boot's `UserPromptSubmit` has not regressed" — did not hold.

This is a second, independent reason nothing further is warranted here, on top of the recall
gate above. Hooks stay off by default, as they already were; this branch does not reopen ADR-024's
question and gives it one more data point against reopening it.

## Bottom line

- **Gate:** 1 of 5 tasks rose (R5). Required: 3 of 5. **Not met.**
- **Latency:** spring-boot's `UserPromptSubmit` regressed on both measured paths. **Also a
  block**, independent of the gate.
- **No Tier 2 sweep runs.** No further spend follows from this branch.
- **What is left, in priority order** — the same list Task 4's report gave, now with the
  ambiguous-exact-match arm identified as the load-bearing one: fix what happens when a prose
  word matches two-or-more symbols by exact name (currently: nothing is seeded); make expansion
  carry seed strength into the candidates it produces, so C3's grading has something downstream
  to affect; then re-run the oracle before spending anything.
