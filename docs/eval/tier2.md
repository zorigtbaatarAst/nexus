# Tier 2 — cost per success

**Status: the measurement has not been taken.** The harness is built, tested and gated; no
sweep has run. Every result section below is empty on purpose. There are no placeholder
numbers in them, because a plausible placeholder is indistinguishable from a result once it is
copied into a README, and this document exists to be copied from.

The only measured numbers on this page are in [How the arms are counted](#how-the-arms-are-counted),
and they come from a two-run parity check on a cheap model, not from a sweep. They are labelled
as such where they appear.

Design: [`2026-09-04-tier2-benchmark-design.md`](../superpowers/specs/2026-09-04-tier2-benchmark-design.md),
a slice of [`13-evaluation.md`](../architecture/13-evaluation.md), which stays authoritative.
Build log: `.superpowers/sdd/2026-09-04-tier2-benchmark/progress.md`.

---

## Before any of this is published

**The product's per-prompt hook injects nothing, and this benchmark measures a corrected one.**

`crates/nexus-cli/src/hooks.rs:37` installs

```
nexus context --task "$CLAUDE_USER_PROMPT" --budget 4000 --brief 2>/dev/null || true
```

Claude Code 2.1.261 does not set `$CLAUDE_USER_PROMPT`. The prompt arrives on the hook's
**stdin, as JSON**. The variable expands to empty, `nexus context --task ""` selects nothing,
and the hook injects zero bytes on every prompt — while exiting 0, so nothing anywhere reports
a problem. This repository's own `.claude/settings.json:30` carries the same string. The doc
comment above the constant states both mechanisms in consecutive sentences, which is how the
wrong one survived.

The A1 arm therefore does **not** run the shipped hook. It runs `scripts/eval/nexus-hook.sh`,
a shim that reads the prompt from stdin and logs what it injected. That is the only way to
measure anything at all — with the shipped string, A1 and A5 would be byte-identical to A0 and
the sweep would return "Nexus makes no difference", which is indistinguishable from a real
negative and is the most expensive possible way to learn nothing.

The design says the arms are "the product's own integration surface" (§4). They currently are
not. **Until `hooks.rs` is fixed, any Tier 2 number describes a Nexus that no user has.** That
is a blocking prerequisite for publication, not a caveat to append. It was deliberately left
out of the benchmark's scope: a product fix folded into a measurement harness gets neither its
own review nor its own test.

---

## The run

Filled in from `meta.json` and `summary.json` when a sweep completes.

| | |
|---|---|
| Model | `claude-opus-5`, pinned; `sweep.sh` refuses anything else without `ALLOW_MODEL_OVERRIDE=1`, and refuses it unconditionally on resume |
| Tasks | 5 — `A1-idempotency-key-length`, `A2-shared-type-change`, `B1-rename-crosses-the-seam`, `B2-orphaned-field-diagnosis`, `C1-regression-recognised` |
| Arms | A0 bare · A1 Nexus · A5 BM25 lexical control |
| Repetitions | 5 per (task × arm) — 75 paid runs |
| Stamp | *(from `meta.json`)* |
| Nexus version | *(from `meta.json`)* |
| Image id | *(from `meta.json`)* |
| Wall clock / total cost | *(from the sweep)* |

## What this can support

**Cost, and cost alone.** Correctness at five tasks is a tripwire: it would show a collapse and
nothing finer. `13-evaluation.md` §9 puts the minimum detectable difference in pass rate at
15–20 pp with **seventeen** tasks; at five it is weaker still. Every correctness figure
`analyse.py` prints carries its own sample size, and quoting one without it is a misuse of this
document.

---

## How the arms are counted

A0 has no hooks. A1 injects a context package on every prompt. If the two were counted from
different fields, the headline number would be an artefact of the harness, and nothing in the
results would reveal it — each arm would still look internally consistent. This is risk R-b,
and the design calls it the single most dangerous implementation detail in the build.

`run.sh` writes `usage.json` from one code path with no branch on the arm, reading `claude -p
--output-format json`'s single `usage` object. `scripts/eval/parity.sh` is the empirical check
on that: it runs one trivial prompt in A0 and one in A1 and asserts that the injecting arm
really injected, that the bare arm did not, that both records carry the same fields with every
counter genuinely populated, and that A1's extra input-side tokens are the package.

Measured 2026-09-06 on `claude-haiku-4-5`, two runs, both replying `ok` in one turn with no
tool use — **not** a sweep result:

| | A0 | A1 |
|---|---|---|
| `input_tokens` | 10 | 10 |
| `cache_creation_tokens` | 7,027 | 3,619 |
| `cache_read_tokens` | 13,615 | 17,729 |
| input-side total | 20,652 | 21,358 |
| `output_tokens` | 98 | 99 |
| `num_turns` | 1 | 1 |
| `total_cost_usd` | $0.0169 | $0.0095 |
| injected | — | 2,073 bytes (258 at `SessionStart`, 1,815 at `UserPromptSubmit`) |

The input-side difference is 706 tokens against 2,073 injected bytes — 2.9 bytes per token,
which is what dense identifier-and-path text costs. **The package is charged to A1 exactly as
A0's prompt is charged to A0.** R-b is retired empirically, not by argument.

The same two rows carry the caching warning below in miniature: A1 read more tokens in total
and paid **44 % less in dollars**, because more of its prefix landed in cache reads. On a
single trivial prompt that is noise; across a sweep it is the reason both numbers are reported.

Re-run it with `./scripts/eval/parity.sh` (two paid runs on a cheap model), or
`REUSE=1 ./scripts/eval/parity.sh` to re-assert against an existing output directory for free.

### Cache reads are counted at full weight

`analyse.py`'s token CPS counts a cache read as one input token. That is **conservative against
A1**: A1 injects a stable prefix on every turn and is therefore the arm that caches best, and
the real price of a cache read is roughly a tenth of an input token. The choice is deliberate —
where the honest weighting was unclear, the plan erred against the product, so that a win
survives the objection that the accounting produced it.

The correction is reported alongside, not left to the reader: `analyse.py` prints a
price-weighted **`CPS ($)`** column straight from `total_cost_usd`, which is the API's own
figure and already prices cache reads at their real rate, plus a median cache-read column per
arm. A reader who thinks tokens are the wrong unit should read the dollar column; a reader who
distrusts pricing should read the token column. T4 is a 30 % threshold that could plausibly be
met on one and missed on the other, which is why neither is presented alone.

---

## What the grades actually mean

Three constraints on wording, each established during the build. The gates are narrower than
their names suggest, and the difference matters when the numbers are quoted.

**`L1_hidden` is `L0 ∧ L2 ∧ hidden`, not an independent measurement.** The grader runs the
hidden test inside the project's own build, so a run whose collateral damage breaks the build
scores `L1_hidden: false` for a reason that has nothing to do with the hidden test. An
"L1-only rate" is therefore only meaningful over runs that are otherwise clean; `analyse.py`
computes it that way and labels it a tripwire rather than a purity claim.

**`L2_collateral` means "the tests in the tree the agent left all pass".** It does not mean the
design's stronger "every test passing at the start commit still passes" — nothing in the
harness enumerates the start-commit tests. An agent that deletes a test, or loosens an
assertion in place, passes L2.

**A1's L1 gate is one site, and A2's is one module.** The design's task table sells
`A1-idempotency-key-length` as "three sites in three languages, none naming the others". At its
start commit only the migration actually constrains the key: the entity carries no `length`
attribute and the validator carries no length check, so only one of the three sites is
behaviourally enforceable. `A2-shared-type-change` has the same shape at module granularity.
The corpus was deliberately left alone rather than edited to make the gate match the prose —
changing the blobs would change what the task is. **`L3_*` is where the multi-site signal
lives**: it reports independently, from the diff, whether every `required_site` was touched. It
is reported, never gated. An L3 pass beside an L1 failure, or the reverse, is the tell the
design asks for — and any claim about multi-site reach must be read off L3, not off L1.

---

## Result

*(paste `analyse.py`'s table here — arm statistics, cost detail, flagged runs, infra failures,
the A1-vs-A0 and A1-vs-A5 comparisons, and the T4 line. `summary.json` in the same run
directory holds the same numbers machine-readably.)*

## A1 vs A5 — did ranking earn its complexity

Pre-registered, unchanged since the design, and reported against rather than gated on:

| | Threshold |
|---|---|
| **T4 — cost** | median CPS reduction **≥ 30 %** (A1 vs A0), paired bootstrap 95 % CI excluding zero |
| **T7 — ranking** | A1 CPS **< A5 CPS**, sign test p < 0.10 across tasks |

Five tasks cannot carry a release gate, so `analyse.py` prints `MEETS T4` / `does not meet T4`
as a reported fact and refuses to evaluate it at all below three surviving tasks.

**The falsifier stands.** `13-evaluation.md` §5: if A1 does not beat A5 — if ranked context is
no better than BM25 over file contents at the same budget and the same injection point — then
the Context Engine has not earned its complexity, and the correct response is to ship BM25 and
delete several thousand lines. That outcome gets this page with the same prominence a positive
result would get.

## What went wrong

*(each sweep records its own failures here: runs that timed out, diffs that did not apply,
tasks whose hidden tests turned out to grade conformity. A benchmark with no defects section is
one nobody read closely.)*

### What the method got wrong before any money was spent

Evidence about the instrument, kept because it is the strongest thing this build produced.
Every one of these was in the plan text as a finished artefact, and every one was caught only
by writing a *different* correct fix and running it:

- **The B2 hidden test was green at its own start commit.** It never read the Java side. It
  would have graded nothing across 15 paid runs and reported a perfect score for every arm.
- **The A2 hidden test would have failed every correct fix.** It demanded the word "scale" in
  two services that need no change at all — conformity-grading, written into the plan.
- **The C1 hidden test graded which SQL idiom the agent chose.** A single combined
  `ALTER … DROP CONSTRAINT IF EXISTS …, ADD CONSTRAINT … UNIQUE` restores the constraint in any
  real Postgres and was graded red; the two-statement form of identical intent passed.
- **The A1 draft assertion would have failed a correct minimal fix**, for the same reason.
- **The shipped hook injects nothing** (above), which would have made all three arms identical.
- **The fixtures did not build at all** when the plan reached them: no dependency versions, no
  JUnit, no lockfile. Grading is `L0 ∧ L1 ∧ L2`, so every run in every arm would have scored
  `passed: false` and CPS would have been infinite for all three arms.

The pattern: a check that *runs* proves nothing. Only a check shown to go red against a
deliberately wrong input, and green against a correct one that is *shaped differently from the
author's own*, is evidence. Read `tests/eval/hidden/README.md` before touching a hidden test.

---

## How to run it

```bash
make bench-image          # release binary + fixtures + the pinned run image
./scripts/eval/parity.sh  # 2 cheap paid runs: token accounting is identical across arms
make bench                # 75 paid runs on claude-opus-5. Hours. Real money.
python3 scripts/eval/analyse.py docs/eval/runs/<stamp>
```

**Pre-flight.** `sweep.sh` runs `scripts/eval/test_grade.sh` before it spends anything and
refuses to start if it fails. A grader stuck at `passed: false` reads as a devastating result
for every arm rather than as a bug, and finding that out after 75 paid runs is the single most
expensive mistake available here. `--skip-gate` exists for a re-run where nothing about
`grade.sh` changed.

**Resuming.** `sweep.sh` is resumable, and re-running it is the expected way to finish an
interrupted sweep — **but it resumes only when you pass the original stamp**:

```bash
STAMP=20260906T101500Z ./scripts/eval/sweep.sh
```

`make bench` takes no stamp and `STAMP` defaults to the current UTC time, so `make bench` after
an interruption starts a *new* run tree and pays for every cell again. Note the stamp the first
invocation prints.

Within a stamp, each cell is one of:

| State on disk | What happens |
|---|---|
| non-empty `grade.json` | skipped |
| `usage.json`, `diff.patch` **or** `.run-started`, no complete grade | re-graded offline, free — the agent already ran and the money is already spent |
| nothing | run and graded |
| `.run-started` **only**, no `diff.patch` | **halts.** The host died mid-container; whether that cell was billed cannot be determined from the run tree |

That last state is the one that needs a person. `grade.sh` refuses for lack of a `diff.patch`,
and the message you see says only that. Check the account's billing for the run, then either
delete `.run-started` to pay for the cell again or leave the cell out of the sweep. Guessing
either way silently double-charges or silently drops a run, which is why it stops.

`meta.json` pins the image id, the nexus version and the model at the top of the run tree, and
a resume that disagrees with any of them is refused rather than half-mixed in.
