# Seeding reads prose as code — three defects the oracle exposed

**Status:** design, 2026-09-09. Acts on
[`2026-09-09-retrieval-oracle-tokio-design.md`](2026-09-09-retrieval-oracle-tokio-design.md),
whose baseline recorded four of five tasks at `0.0` site recall.

**One sentence.** Stop treating a sentence's first word as an identifier, stop deleting the
words a codebase is actually about, weight a seed by how strong its evidence is — then let the
free oracle say whether it worked before any sweep is paid for.

---

## 1. What the oracle found

The baseline at `crates/nexus-core/tests/golden/retrieval_tokio.json` records R1 at `1.0` and
R2–R5 at `0.0`. The zero rows carry `items_included` of 3, 16, 52 and 6 — the packages were
populated, with the wrong files. `nexus context --explain` on R5's prompt names the mechanism:

```
Code (6)
  tokio::tests::time_interval::reset_after   seed: 'After' in the request
  tokio::time::interval::Interval#reset_after seed: 'After' in the request
  tokio::runtime::io::registration_set::NOTIFY_AFTER seed: 'After' in the request
```

The prompt is *"**After** a semaphore has been closed, forgetting permits…"*. The only word that
seeded was the first one. `semaphore` seeded nothing.

## 2. The three defects

**D1 — a leading capital is read as "typed as code".** `is_plain_word`
(`crates/nexus-core/src/context/seeds.rs:196`) disqualifies any word whose first character is
uppercase, on the stated theory that such a word "was typed *as* code". That is true of
`StreamMap` and false of the first word of every English sentence. A disqualified word never
reaches `is_ordinary_word`, so it skips the stopword list **and** the uniqueness rule. `after`
is in `STOPWORDS`; `After` is not filtered by it. All five tokio prompts open with a capital —
`Summing`, `Feeding`, `Rebuilding`, `Binding`, `After` — so all five are affected.

**D2 — the token-family cap is an absolute count, and the corpus is 217× the one it was tuned
on.** `TOKEN_FAMILY_NAME_CAP = 6` (`seeds.rs:254`) makes `token_family` return nothing when a
word is a token of more than six names. Its own comment records the calibration: the
`spring-payments` fixture, **39 symbols**, where `payment` is a token of 13. tokio holds
**8,470**. Measured there: `semaphore` is a token of 20 names, `framed` of 11 — both deleted.
The cap is anti-correlated with relevance at scale: the more central a word is to a domain, the
more names contain it, the more certainly it seeds nothing.

**D3 — a prose word that happens to name one symbol seeds it at full strength.** `exactly_named`
admits a single match unconditionally, and the ranker gives every seed `seed_proximity: 1.0` —
"a seed is 1.0 by definition" (`context/rank.rs:17`). In R2 the prose word `invalid` named
`tokio::time::error::Error#invalid` exactly, and anchored the package on `tokio/src/time/`.

## 3. What changes

**C1 — code shape, not sentence position.** Replace the leading-capital test with one for
*internal* evidence of a naming convention: an interior uppercase, an interior underscore, or a
separator (`.`, `/`, `#`, `::`). `StreamMap` and `NOTIFY_AFTER` remain code. `After`, `Summing`,
`Binding` become prose and face the stopword list and the uniqueness rule. The test is
position-independent, so it cannot be fooled by where a word sits in a sentence.

**C2 — the family cap becomes a fraction of the index.** `max(6, ceil(0.01 × symbol_count))`.
The floor preserves the existing calibration exactly where it was measured: at 39 symbols the
cap is still 6, so `payment` (13) stays excluded and `idempotency` (4) stays admitted. At 8,470
it is 85, admitting `semaphore` (20) and `framed` (11). A word is a *theme* when it saturates
its index — a fraction, not a count.

**C3 — seed strength follows evidence strength.** `seed_proximity` stops being 1.0 for every
seed. A code-shaped token keeps 1.0; a prose word naming exactly one symbol seeds lower; a prose
word matching a token family seeds lowest. R2's `invalid` is still admitted and no longer
outranks a real anchor. The ranker and the density budget do the cutting, which is what they
are for — this adds no new refusal.

## 4. What does not change

- **No new refusals.** Every change either admits more (C2), reclassifies (C1), or reweights
  (C3). Nothing that seeds today stops seeding except by losing a rank contest.
- **`STOPWORDS` gains no entries.** Adding `invalid` and `instead` after seeing them fail is
  fitting the instrument to the result — the move
  [`tier2-corpus-verdict.md`](../../eval/tier2-corpus-verdict.md) condemns. C1 is what makes the
  existing list reach the words it always should have.
- **Intent classification is untouched.** R3 and R5 classify as `unknown` and rank on balanced
  weights. That is a real lead and it is roadmap 5.7's, which wants a weight change to cite
  ledger evidence in its commit message.
- **The Tier 1 oracle is not adjusted.** Its baseline is the thing being moved; editing the
  ruler and the measurement together would make both meaningless.

## 5. How each change is judged

Per defect, not per bundle. After each change the oracle is re-run and its per-task recall
recorded, so a regression is attributable to one change rather than to three.

The baseline ratchet is what makes this safe: recall may not fall. Raising it requires
`NEXUS_REBASELINE=1 make tier1-retrieval` and reading the diff.

## 6. The pre-registered gate on spending

Fixed here, before any implementation, because a threshold chosen after seeing the numbers is
the folklore `11-risks.md` R8 names:

> **The Tier 2 sweep is re-run only if the oracle's site recall rises on at least 3 of the 5
> tokio tasks, relative to the committed baseline (R1 1.0, R2–R5 0.0).**

If the gate is not met, the correct outcome is a written negative result and no spend. Retrieval
that did not improve cannot produce an end-to-end cost improvement, and paying $29 to confirm a
number already in hand buys nothing.

If the gate is met, the sweep runs as **all 75 cells on a fresh stamp** — one stamp, one image,
one corpus, directly comparable to `20260907T093144Z`. Not a partial re-run: this repository has
already been burned once by a sweep whose reported provenance was wrong, and
`check_image_artifacts.sh` exists because of it.

## 7. Risks

- **Hook latency.** C2 admits more seeds and `UserPromptSubmit` already breaches its 150 ms
  budget on spring-boot at 302 ms (`docs/eval/hook-latency.md`). The measurement is re-run on
  the same four repositories, and **a further breach on spring-boot blocks the change** rather
  than being footnoted. tokio at 21 ms has the headroom; spring-boot is the case that decides.
- **C3 has no oracle signal of its own.** Site recall is a set membership test — it cannot see
  that a wrong item ranked lower. C3 is judged by `--explain` output on R2's prompt, read by a
  human, not by a number. Stated plainly because an unmeasurable change in a measured bundle is
  how a bundle gets credit it did not earn.
- **The floor in C2 is inherited, not re-derived.** `6` remains correct only because it was
  measured on `spring-payments`. If that fixture changes, the floor is stale and nothing detects
  it — `golden_packages.rs` runs on that fixture and would move, which is the nearest thing to a
  tripwire.
- **A new store method crosses a policed boundary.** C2 needs a symbol count, and no query for
  one exists. SQL belongs in `nexus-store` — `crates/nexus-cli/tests/boundaries.rs` enforces it.

## 8. Acceptance

1. `After`, `Summing`, `Binding`, `Feeding`, `Rebuilding` are classified as prose; `StreamMap`
   and `NOTIFY_AFTER` as code.
2. At 39 symbols the family cap is 6; at 8,470 it is 85.
3. The oracle's R1 does not fall below 1.0.
4. The oracle's per-task recall is recorded after each change, and the combined result is
   compared against the gate in §6.
5. `docs/eval/hook-latency.md`'s measurement is re-run and spring-boot's `UserPromptSubmit` has
   not regressed.
6. `make check` and `make tier1-retrieval` pass.
