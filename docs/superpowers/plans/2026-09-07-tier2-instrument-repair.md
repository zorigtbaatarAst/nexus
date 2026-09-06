# Plan — repair the Tier 2 instrument, then the arm it was measuring

**Spec / authority:** [`docs/eval/tier2-result.md`](../../eval/tier2-result.md) and the
analyser output at `docs/eval/runs/20260906T131608Z/REPORT.md`. The original design is
[`2026-09-04-tier2-benchmark.md`](2026-09-04-tier2-benchmark.md); its pre-registered
thresholds T4 and T7 are unchanged by this plan.

## Why this plan exists

The sweep ran. 95 paid runs, $37.62, both thresholds missed. But the result cannot be read
as evidence about Nexus, because three things broke underneath it:

1. **The corpus does not discriminate.** A0 — no Nexus, no injection — passed 25/25,
   including `B1-rename-crosses-the-seam` and `B2-orphaned-field-diagnosis`, the two tasks
   written specifically to require cross-file knowledge.
2. **The A1 arm was not intact.** `UserPromptSubmit` injected zero bytes on 10 of 35
   prompts — two whole tasks, every repetition. On those, A1 *was* A0.
3. **The lexical fallback is inert.** ADR-027 shipped it for exactly the case in (2), and it
   fired zero times in 35 prompts.

## Global Constraints

- **The two product defects are fixed before the instrument.** (2) and (3) are real bugs
  that exist whether or not a benchmark ever runs; they are worth fixing on their own terms.
  Fixing the corpus first would only re-measure a broken arm.
- **No change may be justified by "it makes A1 look better."** Every change lands with a
  reason that would hold if Nexus did not exist.
- **Nothing in this plan re-runs the paid sweep.** The re-run is a separate, authorised act.
- Existing behaviour stays covered: `make check` must pass at every task boundary, and no
  existing test may be weakened to accommodate a change.
- The published `ResolveScope` work and the hook-latency measurement on this branch's
  ancestors are not in scope and must not be modified.

## Ruling: changing the corpus after seeing the result

Editing a benchmark's tasks *after* reading its output is how a measurement becomes a
justification. It is also unavoidable here — a corpus the control solves perfectly measures
nothing, and shipping the result as-is would be worse. The hazard is managed, not waved
away, by three rules that bind Task 3 and 4:

- **The acceptance criterion is stated before the work and is about A0 alone:** a task
  belongs in the corpus only if the bare agent fails it at least twice in five attempts.
  Nexus's performance is not a criterion and is not consulted.
- **The criterion is validated by running A0 only** (Task 4), before any A1 or A5 run
  exists to compare against. There is no A1 number in the room while the corpus is being
  decided.
- **Tasks are made harder by removing information from the fixture, never by editing the
  hidden test to catch more.** A hidden test tuned after the fact grades conformity to a
  known answer, which is design risk R-a and the exact trap already documented in this
  repo's history.

*Cost if wrong:* a corpus that is harder but still not measuring cross-file reasoning — in
which case the re-run misses again and the next diagnosis is cheaper than this one, because
the A0-only validation isolates the corpus from the arm.

## Tasks

> Task bodies are filled in from `diagnosis.md` before dispatch. The structure and the
> constraints above are fixed.

### Task 1 — the prompt path returns nothing on real prompts

Two of seven benchmark prompts anchor no seed and produce an empty package. Fix the cause
named in the diagnosis. A regression test must use the real failing prompt text.

### Task 2 — the lexical fallback's gate never clears

`query_overlap(&req.text, &docs) >= 2` admitted nothing across 35 prompts. Either the gate
is mis-tuned or the discriminating-term ceiling cannot admit a term at fixture corpus sizes.
Fix per the diagnosis' arithmetic. A fallback that cannot fire is worse than no fallback,
because it reads as a shipped mitigation.

### Task 3 — make the corpus discriminate

Strengthen `B1` and `B2` (and any other task the diagnosis shows A0 can solve by grep alone)
by removing the information that makes them locally solvable. Hidden tests are not edited.

### Task 4 — validate the corpus against A0 only

**First, a harness change.** `sweep.sh` cannot run one arm: `arms_for()` (sweep.sh:45)
hardcodes the arm list per task, and `test_grade.sh:85` hard-asserts the plan is exactly 95
cells, so a single-arm run is refused by the pre-flight gate — verified this session, where a
`TASKS=` override was refused with "the sweep plans 19 cells, not 95". Add an `ARMS` override
and make the gate validate the plan against the *requested* configuration rather than a
constant, with a test. Routine `SKIP_GATE=1` is not the answer: that gate is what stands
between a typo and 95 wasted paid runs.

Then run the A0 arm, 5 reps, over the revised tasks. Accept a task only if A0 fails it at
least twice in five. This is the pre-registration for any re-run, and the only paid work in
this plan (~25 runs, ~$10).
