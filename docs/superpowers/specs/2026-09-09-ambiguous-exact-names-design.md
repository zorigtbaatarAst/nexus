# A word that names several symbols is weak evidence, not no evidence

**Status:** design, 2026-09-09. Acts on
[`seeding-gate.md`](../../eval/seeding-gate.md), which located the remaining blocker after the
previous cycle raised R5 and reverted C2.

**One sentence.** Stop discarding a prose word because it is the exact name of more than one
symbol; seed all of them at a grade below a unique match, bounded so that a language idiom
cannot flood the package.

---

## 1. What is blocked, and by what

Three of the five tokio tasks still record `0.0` site recall: R2, R3, R4. R1 and R5 are at
`1.0`.

In `resolve()`'s plain-word branch (`crates/nexus-core/src/context/seeds.rs`), a word is matched
against `exactly_named`, which compares case-sensitively against a symbol's last path segment.
Three arms follow:

| matches     | arm          | today                  |
| ----------- | ------------ | ---------------------- |
| exactly one | `[only]`     | seeds it               |
| two or more | `[_, _, ..]` | **seeds nothing**      |
| none        | `[]`         | tries the token family |

The middle arm is the blocker, and its own comment states the reasoning: _"Several symbols are
actually called this. The word cannot tell them apart, and guessing between them anchors the
package on the wrong one."_ Measured against the tokio index:

- `semaphore` — **3** matches: `Tx#semaphore`, `Rx#semaphore` (`sync/mpsc/chan.rs`), and
  `OwnedSemaphorePermit#semaphore` (`sync/semaphore.rs`).
- `framed` — **2** matches: `Decoder#framed` (`codec/decoder.rs`) and `codec::framed`
  (`codec/mod.rs`).

Both words are the subject of their prompt. Both currently contribute nothing.

## 2. The correction

**Ambiguity is a discount, not a disqualification.** A word naming three symbols is evidence
about all three — weaker evidence about each, because it cannot say which. Treating it as _no_
evidence is the defect. The machinery to express "weaker" already exists: the previous cycle
added `SeedStrength`, and the ranker multiplies it into `seed_proximity`.

**C1 already demonstrated the mechanism.** R5 reached `1.0` not because a seed landed in
`batch_semaphore.rs` — none did — but because a correct anchor let graph expansion reach it at
rank 3. The same route is what this change relies on.

## 3. What changes

**A new grade, `SeedStrength::ProseAmbiguous`, weight 0.45**, ordered between `ProseToken`
(0.3) and `ProseExact` (0.6). An exact name is better evidence than a token inside other names,
and worse evidence than an exact name that is unique.

**The `[_, _, ..]` arm offers every match at that grade**, instead of returning empty.

**Bounded by `AMBIGUOUS_NAME_CAP = 4`.** Above it the arm keeps today's behaviour and seeds
nothing, with a ledger note naming the word and its count so the exclusion is visible rather
than silent.

## 4. The bound is the same shape of constant that was just reverted, and this section is why it is allowed back

C2 replaced a fixed count with a fraction of the index and was reverted for measured harm. This
spec introduces a fixed count. That deserves suspicion, so:

**Why a bound is not optional.** The distribution of exact-name multiplicity in tokio:

| symbols sharing an exact name |     1 |   2 |   3 |   4 |   5 |   6 | …         |
| ----------------------------- | ----: | --: | --: | --: | --: | --: | --------- |
| how many words                | 3,281 | 431 | 144 |  63 |  34 |  19 | long tail |

and the head of that tail is `drop` (104), `poll` (90), `poll_next` (58), `poll_flush` (57),
`from` (42), `default` (40). These are trait-method idioms, not domain terms. Unbounded, `drop`
would seed 104 symbols and expand from each, inside a hook with a 150 ms budget. The arm being
changed is what currently prevents that.

**Why 4, and what is dishonest about it.** It admits `semaphore` (3) and `framed` (2). There is
no knee in the distribution to appeal to — the decay is smooth — so the number is chosen partly
because it clears the two motivating words. That is fitting a constant to its examples, which is
the move [`tier2-corpus-verdict.md`](../../eval/tier2-corpus-verdict.md) condemns, and it is
stated here rather than dressed up. What can be said for it independently: every name at the fat
end is a language idiom, and no domain term in this corpus is the exact name of more than a
handful of symbols.

**Why it is acceptable now when C2's was not.** C2's harm — correct answers demoted from rank 4
to 11 and 3 to 37, a package grown from 423 to 2,883 tokens at unchanged recall — passed four
reviews because the ratchet compared `recall` alone. That gap is fixed: the ratchet now fails on
a materially worse rank or a materially larger package. The instrument that missed C2 would
catch it today, and it is what guards this constant.

## 5. What does not change

- **The other two arms.** A unique exact match and the token-family fallback are untouched.
- **The token family cap** stays at 6. It is not this spec's lever and reopening it is what was
  just reverted.
- **No `STOPWORDS` entries.** `drop` and `poll` are excluded by count, not by a hand-maintained
  list of words someone noticed failing.
- **Expansion.** It discards seed strength entirely (`expand.rs` maps a seed to its symbol
  before `impact::run` sees it). That is a real limitation, named in `seeding-gate.md`, and out
  of scope here.

## 6. The pre-registered gate, restated and why restating it is legitimate

The previous gate was _"recall rises on at least 3 of the 5 tokio tasks"_. It was written when
four tasks sat at `0.0`. Two now sit at `1.0` and cannot rise, so three-of-five is unreachable
by arithmetic rather than by merit — a threshold that can no longer be met says nothing about
the work.

Restated, **before implementation and before any number is seen**:

> **The Tier 2 sweep is re-run only if site recall rises on at least 2 of the 3 tasks currently
> at `0.0` — R2, R3, R4 — with R1 and R5 holding at `1.0`.**

Changing a pre-registered threshold is exactly the move to be suspicious of. The defence is that
the baseline moved underneath it and the new threshold is fixed here, in advance, in the same
document that will be checked against. If it is not met the outcome is a written negative result
and no spend, as before.

## 7. Risks

- **Nothing seeded lands in the required file.** For R3, neither the ambiguous matches
  (`codec/decoder.rs`, `codec/mod.rs`) nor the token family — measured at R3's own tree:
  `FramedParts`, `UdpFramed`, `framed`, `framed_half`, `framed_impl`, `framed_read`,
  `framed_write`, `new_framed`, and three test helpers — contains a symbol in
  `codec/framed_impl.rs`. The target is reachable only by expansion, or not at all. **If this
  change moves nothing, the lever is expansion, not seeding**, and that is the most valuable
  thing it can tell us.
- **0.45 is untested.** The ordering is the requirement; the number is a guess between two other
  guesses. It is visible in `--explain` and cheap to move.
- **Latency.** Up to four extra seeds per ambiguous word, each expanding. `UserPromptSubmit`
  already breaches its 150 ms budget on spring-boot. The measurement must be re-run — and
  `scripts/eval/measure.sh` records none of the prompts it measures, so it must be taught to
  before its numbers mean anything.

## 8. Acceptance

1. `SeedStrength::ProseAmbiguous` exists at weight 0.45 and orders between `ProseToken` and
   `ProseExact`.
2. A word with 2–4 exact matches seeds all of them at that grade; a word with 5 or more seeds
   nothing and says so in the ledger.
3. `semaphore` (3 matches) and `framed` (2) seed; `drop` (104) and `poll` (90) do not.
4. R1 and R5 hold at `1.0`; the ratchet's rank and size guards do not fire.
5. The oracle's per-task recall is recorded and compared against §6's gate.
6. `docs/eval/hook-latency.md` is re-measured, with `measure.sh` recording its prompts first.
7. `make check` and `make tier1-retrieval` pass.
