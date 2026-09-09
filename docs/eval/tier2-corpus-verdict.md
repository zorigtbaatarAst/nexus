# The Tier 2 corpus cannot measure retrieval, at any task difficulty

Run `20260907T024229Z`, 2026-09-07. 25 paid A0 runs, $10.81, claude-opus-5.
Pre-registered gate, fixed before the runs: **a task belongs in the corpus only if the bare
agent fails it at least twice in five attempts.**

## Result

| task | A0 passed | `required_sites` hit | verdict |
|---|---:|---:|---|
| A1-idempotency-key-length | 5/5 | 3 | drops |
| A2-shared-type-change | 5/5 | 5 | drops |
| B1-rename-crosses-the-seam | 5/5 | 25 | drops |
| B2-orphaned-field-diagnosis | 5/5 | 10 | drops |
| C1-regression-recognised | 5/5 | 5 | drops |

**Every task drops. The corpus is empty.**

## Why this is not "the tasks are too easy"

That was the first reading, and the numbers refute it.

`B2-orphaned-field-diagnosis` was hardened for this run — the fixture stopped carrying both
sides of the seam, so the one-word `api/`-only revert that used to pass no longer does
(verified under the real grader: it now fails 3 of 3 hidden assertions where it previously
passed 5 of 5). The repair worked. And then:

| B2, A0 | rep 0 | rep 1 | rep 2 | rep 3 | rep 4 |
|---|---|---|---|---|---|
| passed | ✓ | ✓ | ✓ | ✓ | ✓ |
| sites found | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 |
| files touched | 2 | 2 | 2 | 2 | 2 |
| `api/`-side edits | 0 | 0 | 0 | 0 | 0 |

Not one run took a shortcut. The bare agent crossed the seam and made exactly the right
two-file change, five times out of five. `B1` is starker still: 25 site hits across 5 runs is
**all five rename sites, found every time**, by an agent with no index, no injection and no
retrieval of any kind.

A0 is not slipping past these tasks. It is solving them correctly.

## The actual cause: the corpus is smaller than a context window

The fixtures are 6–8 kB. A bare agent reads the whole repository.

There is no retrieval problem when the entire codebase fits in context. This benchmark measures
*"can an agent read 8 kB of code and reason about it"*, and the answer is yes — for every arm,
on every task, at every difficulty. No task design changes that, because the difficulty being
adjusted is not the difficulty being tested.

## What this explains about the paid sweep

Run `20260906T131608Z` (95 runs, $37.62) now has a single mechanism behind every result:

- **All three arms passed 95/95** — the corpus fits in context, so nothing depended on retrieval.
- **A0's median token cost was within 1 % of A1's** — an agent that reads everything spends the
  same tokens whether or not something hands it a package first.
- **A1 was indistinguishable from the BM25 control (p = 0.50)** — with the corpus already in
  context, a ranker has nothing left to rank. That is why the falsifier could not fire: T7 asks
  which of two rankings is better on a corpus where ranking is irrelevant.

**T4 and T7 were never measurable here.** Not "not met" — not measurable, at any task difficulty.
The $37.62 did not buy a failed result; it bought the discovery that the instrument's scale was
wrong.

## The decision this forces

**Do not make the tasks harder again.** The gate has said the corpus *scale* is the instrument
and it is wrong by roughly three orders of magnitude. For contrast, this branch measured Nexus's
hook budgets on real repositories: petclinic 132 files, tokio 868, spring-boot 11,519 —
877 KLOC, where a rescan breached its budget and the fallback path read 371 MB into memory.
Those are the sizes where retrieval either pays or does not. 8 kB is not.

Another round of task tuning would be fitting the instrument to a result. That is the same move
as tuning a hidden test after seeing the grades, one level up, and it is what the pre-registered
A0-only gate existed to prevent. The gate did its job by returning an answer nobody wanted.

## What a measurable Tier 2 needs

1. **Fixtures at a scale where the repository does not fit in context.** The measured latency
   work on this branch suggests the interesting range starts around 10³ files, not 10¹.
2. **Tasks whose answer is not derivable by reading everything** — which is a consequence of (1),
   not a separate requirement.
3. **The A0 gate re-run first**, before any A1 or A5 number exists, exactly as it was here.

Until (1) exists, no Tier 2 number means anything, and none should be quoted.

**(1) now exists.** `tests/fixtures/corpora/tokio` is that corpus — 868 files, 181 KLOC,
replayed from tokio's own history — and [`tier2-result.md`](tier2-result.md) is the sweep it
carried: 75 runs on which the arms do separate on cost. The verdict above stands for the
generated corpus it condemns. It is not a standing embargo on every Tier 2 number.

## Instrument defects found on the way, and their status

- **Empty context packages** on 2 of 7 prompts — fixed (seeding could not reach `idempotencyKey`
  from `idempotency`, and `orders` tied two symbols).
- **The sweep ran a binary a day older than the tree**, undetected because both reported
  `nexus 0.3.0` — fixed; the sweep now refuses unless every artifact the image bakes matches the
  tree by content hash.
- **`A1-idempotency-key-length` has two vacuous hidden assertions** — [#39](https://github.com/zorigtbaatarAst/nexus/issues/39), open.
  Its 5/5 above is therefore doubly uninformative.
- **`tests/eval/hidden/B2`'s javadoc quotes a superseded prompt** — assertions unaffected; needs
  someone with authority over `hidden/**`.
