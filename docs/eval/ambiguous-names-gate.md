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

**Outcome: the change is reverted.** `2df198f` reverts `9827bd6`, `de6b2b2` and `c0b40ab` —
`SeedStrength::ProseAmbiguous`, `AMBIGUOUS_NAME_CAP`, and the `debug_supply.json` row
re-recorded alongside them. Reproduce any number in this document at `9827bd6`, not at `HEAD`;
at `HEAD` both goldens pass and no Rust source differs from `296d83d`. The instrument work
stays: `e250206` (`measure.sh` recording the prompt beside every latency figure) and this
document. Everything below is a record of what was tried and what it cost, not a description of
the shipped tree.

## The count

Before is the committed baseline (last cycle's shipped state — see
[`seeding-gate.md`](seeding-gate.md)); after is the tree with this cycle's
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

## The cost the gate never looked at: `make check` fails, and it fails on every prompt

Everything above measures one instrument — `debug_supply`, five tokio tasks. **The largest
measured cost of this change is not in it, and no clause of the pre-registered gate would ever
have looked where it is.**

`make check` fails. The failing test is
[`crates/nexus-cli/tests/overhead.rs:139`](../../crates/nexus-cli/tests/overhead.rs), the test
`a_prompt_that_names_no_code_costs_nothing`, on one row of its five:

| prompt | `brief_bytes` | `plain_bytes` |
|---|---|---|
| `the amount looks wrong on the receipt` | **0 → 321** | **181 → 503** |

The prompt is ordinary English. It names no code. It is a complaint about a receipt. The other
four quiet prompts are unmoved, so this row is the whole of the failure.

`amount` is the exact name of two symbols in
`tests/fixtures/specs/spring-payments/blobs/` — `Payment#amount` (`Payment-v1.java`,
`Payment-versioned.java`) and `PaymentDto#amount` (`PaymentDto.java`). Two matches is a 2-way
collision, comfortably inside `AMBIGUOUS_NAME_CAP = 4`, so after this change the arm seeds both
and the brief package that used to be empty is 321 bytes of entity field and DTO mirror.

The test's own framing states the stakes better than any paraphrase of them:

> A brief package growing past zero on a prompt that named no code is the regression this
> exists to catch: it is paid on every prompt, by every session, for ever.

**That is a difference in kind, not in degree, from the costs in the table above.** R2, R3 and
R4's package inflation is paid on five benchmark tasks by whoever runs the benchmark, and a
benchmark can be re-run under a better design tomorrow. 321 bytes on a receipt complaint is
paid by every `UserPromptSubmit` hook, in every session, on every prompt containing an ordinary
English word that the index has also seen as a name — and there is no way to stop paying it
except to not ship the change. `overhead.rs`'s own comment records that `amount` occurs 37
times in that fixture's Java source, and that this row was chosen for exactly that reason: it
is ordinary English the index has genuinely seen, which is the case where accidental seeding
shows up.

`crates/nexus-cli/tests/golden/overhead.json` was **not** re-baselined, and re-baselining it
was never an option: the golden's entire purpose is to fail here, so re-recording it would have
defeated the guard rather than satisfied it. It is unchanged since `7de12fb`, untouched by this
cycle, and after the revert it passes on its own.

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
for this row in `de6b2b2` (`found {} → {"api/src/main/resources/graphql/order.graphqls": 2}`,
items 4→6, tokens 477→626) — and **reverted with it** in `2df198f`. The golden is byte-identical
to `296d83d` again. The win is recorded here; it is not in the tree.

The split is exactly what the mechanism above predicts. Tokio's ambiguous matches are
plausible-but-wrong siblings sharing a getter-idiom name — three `semaphore()` accessors, none of
them the semaphore type the bug is about. The generated corpus's ambiguous match is the actual
answer wearing a second hat: the same domain word names both the resolver method and the schema
field of the one bug it concerns. Admitting every match at a discount helps exactly when one of
the matches is the right one, and hurts exactly when none of them is — which is the common case
for a word ambiguous enough to hit this arm in the first place, since most such words turn out to
be language idioms (`drop`, `poll`, `semaphore`-as-getter) rather than domain terms with a single
referent split across formats.

## No value of the cap separates the win from the harm

This is the most useful thing the cycle produced, and it is an argument rather than a number, so
it is written down where the numbers are.

The obvious repair for the split above is to narrow `AMBIGUOUS_NAME_CAP` until it admits `order`
and refuses tokio's idioms. **It does not exist.** The counts say so outright:

| word | matches | what it did |
|---|---|---|
| `order` | **2** — `OrderController#order`, the `order.graphqls` field | the one favourable result |
| `amount` | **2** — `Payment#amount`, `PaymentDto#amount` | the regression paid on every prompt |
| `framed` | **2** — `Decoder#framed`, `codec::framed` | R3's inflation, recall unmoved |

The win is a 2-way collision. The always-paid regression is a 2-way collision. R3's inflation is
a 2-way collision. The smallest cap that admits anything at all is 2, and it admits all three;
any cap that admits the win admits the harm. **"Just narrow the cap" is not an available fix,
and a successor must not reach for it.** Cutting the cap to 2 would have avoided only
`semaphore`'s three matches — and R5, where the correct file was being found by the BM25
fallback anyway, is not where the expensive damage was.

What separates them is not arity. It is what the collision *is*:

- **One concept seen in two formats.** `order` names a Java resolver method and a GraphQL field
  — two languages, two artefact kinds, one thing. The second match is the first one wearing
  another hat, and either is a correct anchor for the same bug.
- **N same-language siblings sharing an idiom name.** `Tx#semaphore`, `Rx#semaphore` and
  `OwnedSemaphorePermit#semaphore` are three unrelated accessors in one language that happen to
  spell a getter the same way; every match is a different thing and every one is wrong.
  `Payment#amount` and `PaymentDto#amount` are the same shape at arity 2 — an entity field and
  its DTO mirror, both Java, neither of them about a receipt.

A rule that can tell those apart reads the languages and kinds of the colliding symbols. It does
not count them. **That is a different design, not a tuning of this one**, and it should be
specified and pre-registered as one.

**Pre-register a successor against both instruments.** This cycle's gate named `debug_supply`
and only `debug_supply`. The cost that decided the outcome was in `overhead`, and the gate as
written could have been passed outright while `make check` was red. Any successor's gate must
name **both** `debug_supply` and `overhead`, and it must state that the `overhead` golden is
never re-baselined to accommodate the change under test.

### `order` is kept as evidence, not as behaviour

`order` is the motivating example for whatever comes next, and it survives here rather than in
the code: `debug_supply.json` is back at its pre-change row, and the behaviour that produced the
win is gone with the rest of the change.

What it is evidence *for* is the lever the closing section below names: **expansion honouring
`SeedStrength`.** `crates/nexus-core/src/context/expand.rs` maps every seed down to its bare
symbol — `seeds.iter().map(|s| s.symbol.clone())` — before `impact::run` ever sees it, so a
discounted seed and a full-strength one expand identically, to the same breadth, at the same
cost. A grade that expansion actually honoured is a different proposition from a grade that only
the ranker sees: it is what could let `order`'s cross-format match contribute a little without
letting `amount`'s two Java siblings buy the same reach on every prompt. That is the successor
`order` motivates, and it is why the result is worth keeping after the change that produced it
is reverted.

### A hazard that survives the revert

Whoever tries next inherits one thing this cycle neither created nor fixed, and it is worth
knowing before designing the next rule.

The count such a rule would test is derived from a hit set that is **already truncated**:
`find_symbols_by_word(project_id, word, WORD_HIT_LIMIT)` in
`crates/nexus-core/src/context/seeds.rs`, with `WORD_HIT_LIMIT = 200`. Truncation can only ever
*lower* a count, never raise it. So a genuine language idiom — `drop` at 104 exact matches,
`poll` at 90 — can arrive at the arm reporting a handful, and be pushed **down** into whatever
band the rule seeds. A rule that excludes idioms by counting them is defeated by the one thing
that reliably miscounts them.

The arm restored by the revert is fail-safe against this by construction: `[_, _, ..]` seeds
nothing for *any* count of two or more, so a lowered count still lands in the same refusal.
`token_family` is fail-safe deliberately and says so in its own doc comment — it refuses on
`hits.len() >= WORD_HIT_LIMIT` outright, because "a filled window is a refusal, not a sample to
judge". **Any successor rule that branches on a count must be fail-safe in the same direction**,
and `token_family`'s is the cheap way to do it: refuse on the fact of truncation instead of
trusting arithmetic the window can move.

The `[only]` arm carries the same hazard today, pre-existing and untouched by this cycle. It
seeds at `ProseExact` when exactly one exact match survives the window, and truncation is one of
the ways exactly one can come to survive it — a word with three exact matches among five hundred
hits can present as unique. Nothing here made that worse, and nothing here fixed it.

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
- **The largest cost, and the gate never looked at it:** `make check` fails.
  `crates/nexus-cli/tests/overhead.rs:139` — `the amount looks wrong on the receipt`, ordinary
  English naming no code, `brief_bytes` 0 → 321 and `plain_bytes` 181 → 503, because `amount` is
  a 2-way exact collision (`Payment#amount`, `PaymentDto#amount`). **This one is paid on every
  prompt in every session, not on five benchmark tasks that can be re-run.** The golden was not
  re-baselined.
- **One real win, on the other corpus:** `order` in the generated corpus, where the ambiguous
  match *is* the fix — and it is the same 2-way shape as `amount`, so **no value of the cap
  separates them.** Kept as evidence for a successor; `debug_supply.json` is reverted with the
  rest.
- **Latency:** not measured, by instruction, because the gate already failed. Recorded as
  unmeasured risk, not a cleared one.
- **No Tier 2 sweep runs.** No further spend follows from this cycle.
- **Reverted in `2df198f`.** The instrument work — `measure.sh`'s prompt recording, and these
  write-ups — is kept.

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

And two conditions on whoever tries it, from "No value of the cap separates the win from the
harm" above. **The cap cannot be tuned into a fix** — `order`, `amount` and `framed` are all
2-way collisions, so every threshold that keeps the win keeps the harm; the distinguishing
feature is one concept in two formats versus N same-language siblings sharing an idiom name, and
reading that is a different design. **And the gate must name both instruments.** The cost that
decided this cycle was in `overhead`, not `debug_supply`, and a gate written against recall
alone would have waved through a change that fails `make check` on every prompt a session
sends.
