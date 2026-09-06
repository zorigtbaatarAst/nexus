# Tier 2 — the result

Run `20260906T131608Z`, 2026-09-06. 95 paid runs, $37.62, claude-opus-5.
Full analyser output: [`runs/20260906T131608Z/REPORT.md`](runs/20260906T131608Z/REPORT.md).

**Both pre-registered thresholds fail, and correctness produced no signal at all.**

## The two thresholds

**T4** — median CPS reduction ≥ 30 % (A1 vs A0), bootstrap 95 % CI excluding zero.
Observed **−0.3 %**, CI [−13.7 %, +16.3 %], favourable on 2 of 5 tasks. **Fails.**

The aggregate ratio-of-sums is kinder — CPS($) is $0.4567 for A0 against $0.3699 for A1,
a 19 % reduction — and it is worth stating because it is the number a less careful reading
would quote. It does not rescue the threshold: the pre-registered statistic is the median
per-task delta, and even the flattering aggregate is under 30 %.

**T7** — A1 CPS < A5 CPS, sign test p < 0.10. Observed **4.2 %** median reduction,
CI [−8.3 %, +12.3 %], favourable on 4 of 7 tasks, **p = 0.50**. **Fails.**

T7 is the one with teeth. Its pre-registered consequence is to ship BM25 and delete the
Context Engine, on the grounds that a thing which cannot beat a lexical control is not
earning its several thousand lines.

## Correctness measured nothing

| arm | runs | passed |
|---|---:|---:|
| A0 — bare agent | 25 | 25 |
| A1 — full Nexus | 35 | 35 |
| A5 — BM25 control | 35 | 35 |

95 of 95. No infra drops, no adjudicator flags, no false-done. A grading pipeline that was
built to separate arms separated nothing, because every arm solved every task. The tasks do
not discriminate on correctness, so the whole sweep reduces to a token-cost comparison —
and the tasks written specifically to need cross-file knowledge, `B1-rename-crosses-the-seam`
and `B2-orphaned-field-diagnosis`, are among the ones the bare agent solved 5 times out of 5.

## Correction: the arm ran a stale binary

**The first version of this document said the lexical fallback's `query_overlap >= 2` gate
never cleared. That was wrong, and the truth is worse.**

The fallback did not fire because **it was not in the binary the sweep ran.**

| | |
|---|---|
| `nexus-bench:latest` built | 2026-09-05 17:11 |
| fallback landed (`0215372`) | 2026-09-06 17:42 |
| `"no symbol anchored"` in the image binary | **0 occurrences** |
| same string in the HEAD binary | 1 occurrence |
| `nexus --version`, both | `nexus 0.3.0` |

The gate is fine. Measured directly on the two failing prompts, `query_overlap` returns
**exactly 2** for both — `idempotency` (df 3) and `column` (df 1) over 12 docs, ceiling 6;
`orders` (df 6) and `total` (df 1) over 15 docs, ceiling 8. It passes, on the bar.

Three things had to line up:

1. `make bench` rebuilds the image before sweeping (`Makefile:79`). **The sweep was started
   with `bash scripts/eval/sweep.sh` directly, which does not.** That was operator error —
   mine — and `sweep.sh` prints `make bench STAMP=…` as the resume command, so the right
   entry point was on screen the whole time.
2. `sweep.sh` pins the image id so a *changing* image cannot be half-mixed into a resumed
   sweep (`sweep.sh:186`), but nothing checks the image against the tree it is reported
   against.
3. The provenance line it does stamp is the **host** binary's `--version`. The version was
   not bumped between those commits, so host and image both read `nexus 0.3.0` and the stamp
   matched a binary a day older than itself.

So `meta.json` recorded a provenance that was true of the wrong binary. A version string
cannot detect this class of drift and never could.

**What it changes.** The A1 arm in run `20260906T131608Z` is not HEAD's A1. On the two tasks
where seeding anchors nothing it injected nothing, where HEAD would have injected a BM25
package. The other five tasks are unaffected — the harness supplies its own `nexus-hook.sh`
wrapper that reads the prompt from stdin in shell, so the `--task-stdin` fix (`7778592`,
also missing from the image) did not matter to them.

**What it does not change.** A0 passed 25/25 on a binary-independent path — the control uses
no Nexus at all. The corpus failure below stands on its own and is fatal to the sweep by
itself.

## Why the packages were empty (the real cause)

Seeding anchors nothing on those two prompts for three stacked reasons, none of them the gate:

- **`find_symbols` matches by suffix only** (`crates/nexus-store/src/lib.rs:1651`), so
  `idempotency` cannot reach `idempotencyKey` and `total` cannot reach `getTotalAmount`.
- **`uniquely_named_symbol` bails at arity ≥ 2** (`crates/nexus-core/src/context/seeds.rs:238`):
  `orders` ties `graphql:api:Query.orders` against `OrderController#orders()`, so B2's best
  word seeds nothing.
- **The ≥ 4-character floor** (`seeds.rs:62`) deletes `key` (A1) and `nan` (B2) before any
  lookup happens.

## What this does and does not license

The analyser says it plainly and it is right: five tasks cannot carry a release gate, and
correctness at seven tasks is a tripwire that detects a collapse, not a regression. So:

- **Licensed:** Nexus did not demonstrate the effect it was built to demonstrate. Neither
  threshold was met, on a corpus and a protocol fixed before the runs.
- **Licensed:** two concrete defects — empty packages on 29 % of prompts, and a lexical
  fallback that never fires.
- **Not licensed:** "Nexus is worse than BM25." p = 0.50 is the absence of evidence, not
  evidence of absence, and 2 of the 7 comparisons were ties by construction.
- **Not licensed:** deleting the Context Engine on this evidence alone. T7's consequence was
  pre-registered against a corpus that turned out not to discriminate — the honest reading is
  that the *instrument* failed first, and a falsifier fed a broken instrument has not fired.

## What to do next, in order

1. **Fix the corpus before re-running anything.** A0 passing 25/25 means the tasks are too
   easy; nothing measured through them can be trusted. This is design risk R-c, realised.
2. **Fix the empty packages.** Two tasks, ten prompts, nothing injected. Find out why the
   seeding anchors nothing on those prompts before blaming the ranking.
3. **Re-examine the fallback gate.** `query_overlap >= 2` never cleared on 35 real prompts.
4. Only then re-run, and only then is T7's consequence meaningful.
