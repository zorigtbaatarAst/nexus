# The ambiguous-exact-name gate — evaluated

Pre-registered in
[`2026-09-09-ambiguous-exact-names-design.md`](../superpowers/specs/2026-09-09-ambiguous-exact-names-design.md)
§6, before this cycle's code was written:

> The Tier 2 sweep is re-run only if site recall rises on at least **2 of the 3** tasks
> currently at `0.0` — R2, R3, R4 — with R1 and R5 holding at `1.0`.

**Zero of three rose. R5 fell from `1.0` to `0.0`. The gate fails on both of its clauses — the
count required and the hold condition stated alongside it — and the spec's own acceptance item 4
("R1 and R5 hold at `1.0`") is breached by the same fact.** No Tier 2 sweep follows.

Reproduction: `NEXUS_TIER1_REQUIRED=1 cargo test --locked -p nexus-core --test debug_supply
the_task_package_reaches -- --nocapture` against the committed `retrieval_tokio.json`. The
ratchet fails with the R2/R3/R5 lines the table below reports. **That golden is untouched by
this cycle** — a fall is never re-baselined past, and `git diff` against it is empty.

## The count

Before is the committed baseline (last cycle's shipped state — see
[`seeding-gate.md`](seeding-gate.md)); after is the live tree with this cycle's
`SeedStrength::ProseAmbiguous` change (`crates/nexus-core/src/context/seeds.rs`), which admits
every match of a 2–4-way exact-name collision instead of seeding none of them.

| task | recall | found | items | tokens |
|---|---|---|---|---|
| R1 stream-map size-hint overflow | 1.0 → 1.0 | `stream_map.rs` at 2 | 21 → 26 | 1678 → 2152 |
| R2 lines-codec invalid UTF-8 | 0.0 → 0.0 | — | 3 → 42 | 427 → 3420 |
| R3 framed spurious decode | 0.0 → 0.0 | — | 16 → 49 | 1424 → 3900 |
| R4 abstract-socket leading NUL | 0.0 → 0.0 | — | 39 → 48 | 3224 → 3957 |
| R5 semaphore reopens after forget | **1.0 → 0.0** | `batch_semaphore.rs` at 3 → none | 69 → 7 | 3934 → 756 |

**Count of tasks whose recall rose: 0.** Required: 2 of {R2, R3, R4}. Not one moved off `0.0`.
**Count of tasks that were supposed to hold and didn't: 1 (R5).** R1 held exactly. R5 is the one
task the spec required to stay at `1.0`, and it is the task that broke.

R1 and R4 also grow in size (R1: +23.8% items / +28.2% tokens; R4: +23.1% items / +22.7% tokens)
without tripping the ratchet's rank/size guards — below whatever relative threshold those guards
use. Noted, not chased further: it does not change the verdict, but it means the two rows the
ratchet left alone are not evidence the change is free elsewhere either.

**Verdict: the gate is not met.** Per the pre-registered condition, the outcome is a written
negative result and no spend — this document is that result. There is no reading of "2 of 3"
that 0 satisfies, and no reading of "R5 holds" that a fall to `0.0` satisfies. This is not a near
miss on a continuous metric; it is a binary count at zero, plus an explicit hold condition
broken.

## What the change did

### R5: the package didn't dilute, it shrank — a right answer for a wrong one

Before this change, `semaphore` matched three symbols by exact last-path-segment name
(`Tx#semaphore`, `Rx#semaphore` in `tokio/src/sync/mpsc/chan.rs`;
`OwnedSemaphorePermit#semaphore` in `tokio/src/sync/semaphore.rs`) and the ambiguous-exact-match
arm seeded none of them — the defect this cycle exists to fix. With nothing anchored, the request
fell through to the BM25 lexical fallback that `Engine::task_package` runs when no stage anchors
anything, and the fallback ranked `batch_semaphore.rs` — the actually correct file — at rank 3 in
a 69-item, 3934-token package.

After this change, `semaphore` anchors on exactly those three getter-idiom matches — none of them
in `batch_semaphore.rs` — and because something now anchors, the request no longer qualifies for
the fallback. Graph expansion runs from three wrong accessors instead, producing a small,
confident, wrong package: 7 items, 756 tokens, the required file absent. This is not dilution
inside a bigger package (R2/R3's pattern, below); the package got *smaller* and worse, because a
broad-but-lucky lexical answer was replaced by a narrow-but-wrong graph one.

### This revises last cycle's account of R5 — stated plainly, not softened

`docs/eval/seeding-gate.md` reports that C1 took R5 from `0.0` to `1.0` by fixing a
leading-capital bug, and says outright that with the bad anchor gone "`semaphore` reaches the
target list and anchors the right file." **That is true of the number and wrong about the
cause.** No seed in that shipped tree ever pointed at `batch_semaphore.rs`. C1 removed a bad
anchor (`After`, misread as code-shaped, seeding `reset_after`/`NOTIFY_AFTER`); with nothing left
to anchor the prompt, it qualified for the BM25 fallback, and the fallback — not the graph — is
what found the file. A correction note has been added to `seeding-gate.md` in place, next to the
claim it corrects.

On R5, BM25 beats the graph. This is the same relationship the Tier 2 sweep already measured on
R3, where BM25 beat the graph's token CPS by 17.9% (`tier2-result.md`, "Per task, and where it
inverts"). R5 is the second task where the path the graph is supposed to improve on turns out to
be the one already holding the right answer.

The spec's own risk section (§7) named a null result — nothing seeded landing in the target file
— as the informative failure mode to watch for. What happened on R5 is not that: it is a fourth
task's worth of *active regression*, a task that was already right becoming wrong. A regression
is at least as informative as a null result here, and arguably more: it shows the scarce resource
on R5 was never anchoring — the fallback already had the file without any — it was the room a
wrong anchor now occupies instead of leaving the field to the fallback.

### R2, R3, R4: the same harm pattern as the reverted C2

| task | items | tokens | recall |
|---|---|---|---|
| R2 lines-codec invalid UTF-8 | 3 → 42 | 427 → 3420 | 0.0 → 0.0 |
| R3 framed spurious decode | 16 → 49 | 1424 → 3900 | 0.0 → 0.0 |
| R4 abstract-socket leading NUL | 39 → 48 | 3224 → 3957 | 0.0 → 0.0 |

All three grow toward the 4,000-token package budget — R3 and R4 land within 100 tokens of it —
with recall unchanged on every one. This is the identical shape `seeding-gate.md` measured for
C2 (there: 3→34, 16→48, 39→50 items, recall unchanged on all three, before that change was
reverted for it). C2 was reverted for exactly this pattern on the token-family cap; this cycle
reproduces it on the ambiguous-name cap instead. The arm doing the over-admitting differs; the
shape — more seeds, more expansion, no more of it landing in the required file — does not.

### The one place it helps: the generated corpus, where the ambiguous match *is* the answer

Not everything points the same way, and this should not get buried under the tokio numbers.
`order` is the exact name of both `OrderController#order` (a Java resolver method) and a GraphQL
field in `order.graphqls` — an ambiguous exact match by the same rule tokio's `semaphore` and
`framed` hit. Before this change it seeded nothing, the same defect. Now it seeds both, and one
of the two — `order.graphqls`, the canonical anchor for `plants_bug`
(`tests/fixtures/specs/next-storefront/fixture.toml`) — is found at rank 2, where it was missed
entirely before. `crates/nexus-core/tests/golden/debug_supply.json` was legitimately re-recorded
for this row (`found {} → {"api/src/main/resources/graphql/order.graphqls": 2}`, items 4→6,
tokens 477→626) and is not touched by this document.

The split is exactly what the mechanism above predicts. Tokio's ambiguous matches are
plausible-but-wrong siblings sharing a getter-idiom name — three `semaphore()` accessors, none of
them the semaphore type the bug is about. The generated corpus's ambiguous match is the actual
answer wearing a second hat: the same domain word names both the resolver method and the schema
field of the one bug it concerns. Admitting every match at a discount helps exactly when one of
the matches is the right one, and hurts exactly when none of them is — which is the common case
for a word ambiguous enough to hit this arm in the first place, since most such words turn out to
be language idioms (`drop`, `poll`, `semaphore`-as-getter) rather than domain terms with a single
referent split across formats.

## Latency — deliberately not measured

The brief's Step 3 and the spec's acceptance item 6 call for re-measuring
`docs/eval/hook-latency.md`'s four-repository sweep with Task 3's prompt recording in place.
**That step was skipped by explicit instruction, not by oversight, because the gate above has
already failed on recall.** A change that will not ship on this evidence gains nothing from a
latency number: measuring it answers a question no one will act on. Task 3's fix — every latency
figure now printed beside the exact prompt that produced it, round-trip proven on
spring-petclinic — is in place and ready if this problem is revisited. The spec's own risk (§7:
"up to four extra seeds per ambiguous word, each expanding," against spring-boot's
already-breached 150 ms `UserPromptSubmit` budget) is therefore recorded here as **unmeasured**,
not as cleared. Nothing in this document should be read as evidence the latency risk is small.

## Bottom line

- **Gate:** 0 of 3 rose (R2, R3, R4 all held at `0.0`). Required: 2. **Not met.**
- **Hold condition:** R1 held at `1.0`. R5 did not — it fell to `0.0`, breaching the gate's other
  clause and the spec's acceptance item 4 in the same fact.
- **R5's mechanism:** a BM25 fallback that happened to be right was replaced by a graph
  expansion that is confidently wrong, because the fix did exactly what it was built to do —
  make `semaphore` anchor — on the three symbols that are not the answer.
- **R2/R3/R4:** grew toward the token budget in the shape C2 was reverted for, recall unmoved on
  all three.
- **One real win, on the other corpus:** `order` in the generated corpus, where the ambiguous
  match *is* the fix. `debug_supply.json` reflects it, already committed, untouched here.
- **Latency:** not measured, by instruction, because the gate already failed. Recorded as
  unmeasured risk, not a cleared one.
- **No Tier 2 sweep runs.** No further spend follows from this cycle.

**What this means for the next cycle.** Two cycles running, "admit more seeds" has demoted a
correct answer. C2 sank two already-correct ranks (R1's from 4 to 11, R5's from 3 to 37) by
widening the token-family cap. This cycle went further and dropped R5's recall to zero outright,
by admitting three getter-idiom matches that the earlier, narrower arm had no way to reach. Both
times the newly admitted evidence was wrong evidence, and both times it crowded out a route —
rank position last cycle, the lexical fallback this cycle — that was already reaching the right
file some other way. The spec's own §7 named the alternative before this run had a number: if the
change moved nothing, the lever is expansion, not seeding. Nothing did move on R2, R3 or R4, and
the one task that moved broke. This is the second cycle's worth of evidence for that reading, now
with a mechanism attached: `seeding-gate.md`'s "remaining blocker" section already noted that
`context/expand.rs` maps each seed down to its bare symbol before `impact::run` ever sees it, so
a graded, less-often-disqualified seed stage still hands expansion the same undifferentiated
symbol list it always did. Grading what gets seeded has now been tried twice, in opposite
directions — narrower via `is_plain_word`/`targets()` in C1, broader via `ProseAmbiguous` here —
with one win apiece attributable to something other than the grading itself (C1's fallback, this
cycle's already-correct `order` collision), and no win yet attributable to expansion using the
grade at all. The next lever to try, per the spec's own prediction and this document's
confirmation of it, is expansion — not another adjustment to what gets seeded or at what
strength.
