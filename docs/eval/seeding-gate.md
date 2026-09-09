# The seeding gate — evaluated

Pre-registered in [`2026-09-09-seeding-reads-prose-design.md`](../superpowers/specs/2026-09-09-seeding-reads-prose-design.md)
§6, before any of C1–C3 was written:

> The Tier 2 sweep is re-run only if the oracle's site recall rises on at least **3 of the 5**
> tokio tasks, relative to the committed baseline (R1 1.0, R2–R5 0.0).

**One of five rose. The gate is not met. No Tier 2 sweep follows.**

## The count

| task | baseline | shipped (C1+C3) | direction | rank of the answer |
|---|---:|---:|---|---|
| R1 stream-map size-hint overflow | 1.0 | 1.0 | unchanged | 4 → **2** |
| R2 lines-codec invalid UTF-8 | 0.0 | 0.0 | unchanged | — |
| R3 framed spurious decode | 0.0 | 0.0 | unchanged | — |
| R4 abstract-socket leading NUL | 0.0 | 0.0 | unchanged | — |
| R5 semaphore reopens after forget | 0.0 | **1.0** | **rose** | — → **3** |

Baseline is `git show 749f50f:crates/nexus-core/tests/golden/retrieval_tokio.json`, the state
before Task 2 touched anything. The shipped column is the committed
`crates/nexus-core/tests/golden/retrieval_tokio.json` at HEAD, which is **C1 and C3 with C2
reverted** — see §C2. Recall is identical either way; the rank column is not, which is the
whole point of §C2 and of the ratchet fix that goes with it.

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
prose and stopped, `semaphore` reaches the target list and anchors the right file. *(Corrected
below — it doesn't. Nothing anchors; a fallback does.)*

### Correction (2026-09-09, from the ambiguous-exact-names cycle): R5 rose through the fallback, not the graph

The paragraphs above are right about the number and wrong about the cause. R5 did rise from
`0.0` to `1.0` when C1 landed, and `batch_semaphore.rs` did land at rank 3 — but no seed in that
shipped tree points at the file, and none ever did. `semaphore` did not "reach the target list"
by anchoring anything: before C1, and still after it, the plain-word branch's ambiguous-exact-
match arm saw `semaphore`'s three exact matches (`Tx#semaphore`/`Rx#semaphore` in
`tokio/src/sync/mpsc/chan.rs`, `OwnedSemaphorePermit#semaphore` in `tokio/src/sync/semaphore.rs`)
and seeded none of them, same as always. What C1 actually did was remove `After` as a bad anchor.
With *nothing* anchoring the prompt at all, `Engine::task_package` fell through to its BM25
lexical fallback — and the fallback, not the graph, is what ranked `batch_semaphore.rs` at 3.

This surfaced because the next cycle changed exactly the arm this document's "remaining blocker"
section identifies below, so that `semaphore` *does* anchor — on the three getter-idiom matches
above. None of them is in `batch_semaphore.rs`. Anchoring something disqualifies the prompt from
the fallback it had been quietly relying on, and R5's recall fell straight back to `0.0`. On R5,
BM25 beats the graph — the same relationship the Tier 2 sweep measured on R3, where BM25 beat
the graph's token CPS by 17.9% (`tier2-result.md`, "Per task, and where it inverts"). Full
account: [`ambiguous-names-gate.md`](ambiguous-names-gate.md).

### C2 — the family cap becomes a fraction of the index: reverted, because it cost four things and bought none

**C2 is not in the tree. It was reverted** — `225cea1` (the cap) and `a3d05ea`/`bdac524` (the
`count_symbols` store query that existed only to serve it). What follows is why, and it is
harsher than the first version of this section, which called the change "inert" and said it
"still raises the ceiling". Inert would have been fine. It was not inert.

**It was inert for the two words it was written for.** Implemented exactly as specified:
`max(6, ⌈0.01 × symbol_count⌉)`, which at tokio's 8,470 live symbols is 85 (81 against the
fixture's actual 8,027) — comfortably above `semaphore`'s 20 matches and `framed`'s 11, both of
which the old fixed cap of 6 deleted outright. And it changed nothing for either word, because
neither reaches the cap at all. `exactly_named` matches case-sensitively on a symbol's last path
segment, and in the tokio index `semaphore` has **three** exact matches — `Tx#semaphore`,
`Rx#semaphore`, `OwnedSemaphorePermit#semaphore` — and `framed` has **two** — `Decoder#framed`,
`codec::framed`. `targets()`'s match arms are `[]` (nothing), `[only]` (exactly one — seeds at
full strength), and `[_, _, ..]` (two or more — the *ambiguous* arm, which seeds nothing and
never calls `token_family` or consults any cap). Two or three exact matches both land in that
last arm.

**The spec's diagnosis was wrong about which rule binds.** §2 D2 says the cap "makes
`token_family` return nothing when a word is a token of more than six names" and names
`semaphore` (20) and `framed` (11) as the words it deletes. Both words are in fact deleted by
the ambiguous-exact-match arm, upstream of `token_family` and upstream of any cap — fixing the
cap cannot reach a word that never gets that far. The correction is filed against the spec
itself.

**And on every word that did reach the cap, it made the packages worse.** This was in the
branch's own committed goldens the whole time, one commit apart, and nobody read them.
`git show 566dbe0:crates/nexus-core/tests/golden/retrieval_tokio.json` is C1 alone; `225cea1`
is C1 plus the cap:

| task | C1 alone (`566dbe0`) | C1+C2 (`225cea1`) | |
|---|---|---|---|
| R1 | rank **4**, 21 items, 1 675 tok | rank **11**, 37 items, 2 820 tok | the correct answer sank |
| R2 | recall 0.0, **3** items, **423** tok | recall 0.0, **34** items, **2 883** tok | 6.8× the tokens, still nothing right |
| R3 | recall 0.0, 16 items, 1 424 tok | recall 0.0, 48 items, 3 932 tok | 2.8× the tokens |
| R4 | recall 0.0, 39 items, 3 226 tok | recall 0.0, 50 items, 3 949 tok | drift |
| R5 | rank **3**, 49 items, 3 934 tok | rank **37**, 69 items, 3 900 tok | the correct answer sank |

Four harms, zero benefits: both correct answers demoted — R5's from third in the package to
thirty-seventh, which is past where anyone reads — and three packages inflated, R2's by 2 460
tokens of material that still contains none of the answer. Recall did not move on any row,
which is exactly why this shipped green: **the ratchet compared recall and nothing else**, and
recall is a set membership test that cannot see a correct answer sinking or a package tripling.
That blind spot is a defect in the instrument rather than in C2, and it is fixed independently
of this revert — `ratchet_failures` now guards rank and package size on the same
hold-or-improve terms.

**What was said about latency does not survive re-measurement, and is not part of the case.**
The first version of this section reported C2 as a further breach of spring-boot's
`UserPromptSubmit` budget: 302 ms → 380 ms seeded, and a symptom-only prompt going from a 694 ms
lexical fallback to 1 016 ms. Re-measured with all three binaries against one clone and one
*recorded* prompt pair, C2 is latency-neutral on the seeded path and **faster** on the natural
one (1 419 ms → 924 ms), because a prompt that seeds something does not then pay for a full
lexical scan. The earlier comparison used prompts that were never written down, so it compared
two different measurements. See [`hook-latency.md`](hook-latency.md) §"Re-measured a third
time". The retrieval evidence above is reproducible from committed goldens and is the whole
reason for the revert.

**`TOKEN_FAMILY_NAME_CAP = 6` is the cap again**, as it was, calibrated on `spring-payments`
where `idempotency` is a token of 4 names and `payment` of 13. Nothing in this branch showed
that number to be wrong; C2 showed that widening it on a large index buys nothing and costs
rank.

### C3 — seed strength follows evidence strength: mechanically correct, did not move R2

Implemented as `SeedStrength` (`ProseToken` 0.3, `ProseExact` 0.6, `CodeShape` 1.0), replacing
the flat `seed_proximity: 1.0` every seed got regardless of how it was found. Read on R2's
prompt directly, with `--explain` (Task 4 Step 6, run against R2's start state in the **tokio
fixture**: `a75c989ce4054895ad619bb3e7fbc059c658a557`, the commit `tokio_fixture.sh` writes for
`R2-lines-codec-invalid-utf8`). That SHA is not an object in *this* repository — `git cat-file -t
a75c989` here fails, and the first version of this line invited exactly that check. It lives in
`target/fixtures/tokio`, it is recorded in `target/fixtures/tokio.manifest.json`, and it is
reproducible: the fixture script pins the author, the committer and both dates, so
`make tokio-fixture` rebuilds the same SHA. This is C3's only evidence, so it has to be
re-runnable, and it is. **The reading below was taken with C2 still in the tree**; the tree it
describes no longer exists, so the same command re-run after the revert is recorded underneath
it:

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

#### The same reading, re-run after C2 was reverted

C2's revert takes R2's package from 34 items back to 3, so the paragraphs above describe a
package that is gone. Re-run at the same start state against the current tree:

```
Code (3)
  tokio::time::error::Error#invalid  name match   tokio/src/time/error.rs:78 · seed: 'invalid' in the request names exactly one symbol
  tokio::time::error::Error  depth 1              tokio/src/time/error.rs:26 · forward: via calls …Error#invalid
  tokio::tests::test_clock::resume_lets_time_move_forward_instead_of_resetting_it  name match

  included   0.70  tokio::time::error::Error#invalid
            seed 0.60 · graph 0.00 · churn 0.10 · …
  included   0.61  tokio::time::error::Error
  included   0.60  tokio::tests::…::resume_lets_time_move_forward_instead_of_resetting_it
            seed 0.30 · graph 0.00 · … · test 0.30 · …
```

**C3's grading is still visible and still correct**: `Error#invalid` carries `seed 0.60`
(`ProseExact`) rather than the flat 1.00 it had before C3, and the `instead` seed carries
`seed 0.30` (`ProseToken`). What changed is the surrounding package. `Error#invalid` is back to
**first of three** rather than fourth of thirty-four — the three items that outscored it were
C2's, and they were wrong too. Nothing from `tokio-util/src/codec/` appears at all now, not even
the accidental `Encoder` at 0.55: with the cap reverted, the `slice` seed that reached it does
not fire. So the conclusion is unchanged and if anything sharper — **C3 reorders a candidate set
that does not contain the answer.** Demotion has nothing to promote in its place. That is the
blocker below, not a weighting problem.

### The remaining blocker, now precisely located

Three independent investigations (C1's fix, C2's trace, C3's `--explain` reading) converge on
one mechanism: **the ambiguous-exact-match arm.** A prose word matching two or more symbols by
exact last-path-segment name — `semaphore` (3 matches), `framed` (2 matches) — seeds *nothing*,
not a weakly-graded something. It is upstream of the family cap (which is why C2 could not widen it, and part of why C2 was
reverted) and
upstream of seed strength (so C3 cannot demote something into its place, because nothing is
there). Whatever fixes R2, R3 and R4 next has to change what that arm does, not how much a
family cap admits or how a seed is weighted once found.

## Hook latency — the other axis, and the one claim this document got wrong

Full detail, method, and per-repository breakdown: [`hook-latency.md`](hook-latency.md),
§"Re-measured, 2026-09-09" and §"Re-measured a third time, 2026-09-09".

**This section originally reported a further breach and blocked the change on it.** It said
spring-boot's `UserPromptSubmit` went 302 ms → 380 ms on a prompt naming a symbol, and that a
symptom-only prompt which used to fall back to a 694 ms lexical scan now seeded broadly under
C2's widened cap and cost 1,016 ms. Acceptance criterion 5 was declared failed on that basis.

**It does not hold up.** `scripts/eval/measure.sh` takes its two prompts from the environment
and neither earlier run recorded the ones it used, so the two figures being subtracted came from
different measurements. Re-measured with three binaries — pre-branch `749f50f`, branch tip
`23fa712`, and the branch with C2 reverted — against one clone, in one session, with a recorded
prompt pair: the seeded path is 405 / 391 / 414 ms, all noise, and the symptom-only path is
1 419 / 924 / 1 400 ms, where C2 is the *fast* one because seeding something means not paying
for a lexical scan afterwards. **C2 caused no latency regression that survives measurement.**

What is true, and was true before this branch: spring-boot breaches three of four hook budgets,
`UserPromptSubmit` by a factor of two or more on every path. That is ADR-024's existing
condition, unmet for the reasons it was already unmet. Hooks stay off by default; this branch
neither reopens that question nor adds a reason to.

The revert of C2 stands on the retrieval evidence — four measured harms in the branch's own
committed goldens — which is reproducible from `git show` and does not depend on a prompt
anybody forgot to write down.

## Bottom line

- **Gate:** 1 of 5 tasks rose (R5). Required: 3 of 5. **Not met.**
- **C2 reverted.** Not for latency — that claim did not survive re-measurement — but for four
  retrieval harms with no benefit: R1's answer rank 4 → 11, R5's 3 → 37, and three packages
  inflated up to 6.8×, all at unchanged recall.
- **Latency:** spring-boot is over budget on three of four hooks and was before this branch.
  Post-revert it is within noise of pre-branch on every path. **Not a block, and not a win.**
- **No Tier 2 sweep runs.** No further spend follows from this branch.
- **What is left, in priority order** — the same list Task 4's report gave, now with the
  ambiguous-exact-match arm identified as the load-bearing one: fix what happens when a prose
  word matches two-or-more symbols by exact name (currently: nothing is seeded); make expansion
  carry seed strength into the candidates it produces, so C3's grading has something downstream
  to affect; then re-run the oracle before spending anything.
- **And the instrument was blind, which is why C2 shipped green through four reviews.** The
  ratchet compared `recall` and nothing else, while the golden recorded each found site's rank
  and each package's size all along. `ratchet_failures` now guards those too. Any future change
  to seeding or ranking is measured against a ruler that can see a correct answer sinking.
