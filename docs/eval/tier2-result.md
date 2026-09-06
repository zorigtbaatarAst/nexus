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

## The finding that undercuts A1's own arm

`UserPromptSubmit` injected **zero bytes on 10 of A1's 35 prompts** — two entire tasks,
every repetition:

| task | A1 injected | A5 injected |
|---|---|---|
| A1-idempotency-key-length | **0 B on 5/5** | 892 B |
| B2-orphaned-field-diagnosis | **0 B on 5/5** | 941 B |
| A2-shared-type-change | 645 B | 1 033 B |
| B1-rename-crosses-the-seam | 1 168 B | 1 192 B |
| C1-regression-recognised | 286 B | 1 444 B |
| E1-untested-change | 1 001 B | 1 236 B |
| N1-null-task | 338 B | 1 122 B |

On two of seven tasks the Nexus arm *was* the bare agent. And the lexical fallback ADR-027
shipped for exactly this case — the graph anchors nothing, so guess rather than stay silent —
**fired zero times in 35 prompts**. Its gate demands two corroborating discriminating terms,
and on real benchmark prompts that bar is never cleared. A fallback that never fires is not a
fallback.

This cuts both ways and neither is comfortable:

- It weakens T7 as a test of the Context Engine, since 2 of 7 tasks are ties by construction.
- It is itself a product finding, and a worse one than the threshold miss: on 29 % of real
  prompts Nexus produces nothing, silently, and the mechanism built to prevent that is inert.

The clearest evidence that the remaining differences are noise: **B2 is one of A1's four
"wins" over A5 — and it is a task where A1 injected nothing at all.** An arm cannot win on
context it did not supply.

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
