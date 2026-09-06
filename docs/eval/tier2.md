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
a problem. This repository's own `.claude/settings.json:30` carries the same defect — the same dead
`$CLAUDE_USER_PROMPT`, without the `--brief`. The doc
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

## Pre-sweep measurement: A1 receives less context than A5 on all five tasks

**Measured 2026-09-06, before any money was spent. Not a sweep result.** Each task's own prompt,
run through both rankers at that task's real start commit, with `target/release/nexus` and the
arms' own hook arguments (`--budget 4000 --brief`, plus `--rank lexical` for A5; SessionStart is
`--session --budget 800`, and A5 has no SessionStart hook):

| task | A1 (engine) | A5 (BM25) | A1's SessionStart |
|---|---|---|---|
| `A1-idempotency-key-length` | **0 B** | 893 B | 259 B |
| `A2-shared-type-change` | 646 B | 1,034 B | 273 B |
| `B1-rename-crosses-the-seam` | 1,169 B | 1,193 B | 117 B |
| `B2-orphaned-field-diagnosis` | **0 B** | 942 B | 117 B |
| `C1-regression-recognised` | 287 B | 1,445 B | 259 B |

**On two of the five tasks the product arm's per-prompt hook injects nothing at all.** For
`A1-idempotency-key-length`, `nexus context --task "<the prompt>" --budget 4000` reports
`considered 0 · included 0 · excluded 0`: the Context Engine seeds from identifiers, that prompt
names none ("The idempotency key column is too short for the new upstream provider. Widen it to
128 characters everywhere it is constrained."), so nothing anchors and the package is empty. BM25
has no such dependency: it returns all eight files, `V1__init.sql` — the exact file this task's
hidden test grades — ranked 5th of 8.

Two consequences for how the result must be read:

- **A1 vs A5 is not a same-volume contrast.** The design's premise (§5) is "same budget, same
  injection point, only the ranking function differs". At fixture scale the 4,000-token budget
  binds neither arm — the largest package here is about 400 tokens — so the shared budget
  constrains nothing, and the arms differ in how much text they inject as well as in how it was
  chosen. On the two zero-byte tasks A1 is not a ranking treatment at all; it is A0 with a
  SessionStart summary.
- **This is a fact about the product, not a harness defect.** Seeding from identifiers is how the
  Context Engine works. The corpus, the seeding and the hidden tests were deliberately left
  unchanged: tuning any of them until the product arm had something to say would be tuning the
  instrument to the answer.

`analyse.py` carries this into every sweep rather than leaving it to this page. Each run
directory keeps the `injected.log` its hooks wrote; the arm table reports **median injected
bytes**, an **empty-package count** and a **lexical-package count**, the A1-vs-A5 comparison line
and the T7 line both repeat them, and `summary.json` carries them as `median_injected_bytes`,
`n_empty_packages`, `n_lexical_packages`, `n_prompt_packages` and `n_with_injection_log`. An arm
with no hooks (A0) has no log and reads `— (no hooks)`, never `0` — "no hooks" and "the ranker
selected nothing" are different facts.

The two counts are separate because the lexical fallback changed what silence looks like. A1 now injects something on almost every prompt, so a count of zero-byte
packages reads as good news while the condition it was watching for — the graph anchored nothing
— is exactly as frequent as before. `n_lexical_packages` counts packages whose every item is a
BM25 hit: **for A1 that is the fallback firing**, and those tasks are a tie with A5 by
construction rather than a contrast between two rankings; for A5 every package is lexical by
design, so `A5 lexical == all` is that arm's sanity check. `n_empty_packages` survives alongside
it because nothing-at-all is still a real outcome: the fallback demands two corroborating terms
between the prompt and the corpus before it will guess, and below that bar the empty package
remains the answer.

---

## The run

Filled in from `meta.json` and `summary.json` when a sweep completes.

| | |
|---|---|
| Model | `claude-opus-5`, pinned; `sweep.sh` refuses anything else without `ALLOW_MODEL_OVERRIDE=1`, and refuses it unconditionally on resume |
| Tasks | 7 — `A1-idempotency-key-length`, `A2-shared-type-change`, `B1-rename-crosses-the-seam`, `B2-orphaned-field-diagnosis`, `C1-regression-recognised` at all three arms; `E1-untested-change`, `N1-null-task` at A1 and A5 only |
| Arms | A0 bare · A1 Nexus · A5 BM25 lexical control |
| Repetitions | 5 per (task × arm) — 95 paid runs (5×3×5 + 2×2×5) |
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

## What the arms actually are

The design (§4) defines A1 as `nexus init --hooks --verify` with the MCP server available. The
built arm is narrower, and the gap changes what some of the reported numbers are evidence about.

**A1 is context injection only.** `scripts/eval/arms/A1.json` installs `SessionStart`,
`UserPromptSubmit` and a `PostToolUse` rescan. It has no `mcpServers` block, so **no Nexus MCP
tool is reachable** to the agent, and no `Stop` hook, so **`nexus verify` never runs**. The
verification gate was left out deliberately rather than by omission: it runs a full build with a
600 s timeout on every turn, inside a container the sweep already caps at `TIMEOUT_S=900`, so
installing it would turn a cost benchmark into a measurement of how often the build fits in the
remaining budget. If the gate is ever wanted here, it needs its own timeout budget and its own
review; it is not a line to add to the arm file.

**Consequence for `false_done`.** The design (§7) attributes a reduction in false completion
claims to the verification gate. No gate is installed, so **`false_done` in this sweep is not
evidence about `nexus verify`** — it is what the injected context alone did to the agent's
willingness to claim success. `analyse.py` prints `false_done` per arm regardless, because it is
still a useful tripwire; quoting it as a verification result would be wrong.

`false_done` also owes a hand-audit before it is published. It is derived from `claimed_done`,
which is a whole-word `done` match against `result` (`run.sh`) — narrow enough to reject
"abandoned", "undone" and "nothing to be done", and still wide enough to accept "not done" and
"done nothing". Read the transcripts of the runs it flags before quoting the number.

**A1 fires its per-prompt hook exactly once.** `claude -p` submits a single user prompt, so
`UserPromptSubmit` runs once per run. A1's entire treatment is one SessionStart package plus one
task package — the multi-turn accumulation the design imagines is not what is being measured. The
`PostToolUse` rescan runs on every edit, but nothing consumes its output in a single-prompt run.

**A1 vs A5 differs in three things, not one.** Beyond the ranking function, A1 has a
`SessionStart` package that A5 does not, and a `PostToolUse` rescan that A5 does not — two
structural advantages A5 lacks. But the [pre-sweep
measurement](#pre-sweep-measurement-a1-receives-less-context-than-a5-on-all-five-tasks) above
shows A1's own per-prompt package is smaller than A5's on every task, so the three differences do
not all point the same way: two favour A1, the ranking function's measured effect favours A5. This
is not a clean single-variable contrast in either direction:

- A **negative** result (A1 no better than A5) is therefore safe to act on, and the
  pre-registered consequence stands: it would mean ranked context did not beat BM25 *even with
  two extra advantages*.
- A **positive** result is **not attributable to ranking alone**. It would have to be
  disentangled by a follow-up that varies one thing at a time.

The obvious configuration fix does not work, and it fails silently. Giving A5 a matching
`--session --rank lexical` hook would produce an **engine-ranked** package, not a lexical one:
`Engine::context` dispatches on purpose first (`crates/nexus-core/src/engine/query.rs:128-131`),
and only `task_package` honours `--rank`, so `Purpose::Session` never reaches the lexical path.
The control would be contaminated by the thing it is controlling for, with nothing in the output
to say so. The difference is documented here instead.

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

Re-run it with `./scripts/eval/parity.sh` — which always pays for two fresh runs on a cheap
model, because reusing whatever is on disk would return `parity ok` for a `run.sh` it never
ran. `REUSE=1 ./scripts/eval/parity.sh` re-asserts against an existing output directory for
free, and `./scripts/eval/parity_selftest.sh` mutation-tests the assertions themselves without
spending anything.

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

**C1 grades one solution family, and the prompt does not say which.** `C1-regression-recognised`
asks about "payments are being double-charged"; its hidden test grades a **schema-level** fix —
the unique constraint restored in the migration. An agent that instead fixes the same symptom at
the application level (a transaction plus a lock, an idempotency check in the service) has
answered the prompt and is graded **red**. That constrains the solution family across 15 paid
runs, and it constrains it identically in every arm, so it does not bias the A1-vs-A5 contrast —
but a low pass rate at C1 must not be read as "no arm could fix double-charging".

---

## Result

*(paste `analyse.py`'s table here — arm statistics **including the injected-bytes, empty-package
and lexical-package columns**, cost detail, flagged runs, infra failures, the A1-vs-A0 and
A1-vs-A5 comparisons, and the **T4 and T7** lines. `summary.json` in the same run directory holds
the same numbers machine-readably. Read the empty and lexical counts against the pre-sweep table
above before reading anything else: a task where A1 was handed nothing, or handed the same BM25
ranking A5 uses, is not a contrast between two rankings.)*

## A1 vs A5 — did ranking earn its complexity

Pre-registered, unchanged since the design, and reported against rather than gated on:

| | Threshold |
|---|---|
| **T4 — cost** | median CPS reduction **≥ 30 %** (A1 vs A0), paired bootstrap 95 % CI excluding zero |
| **T7 — ranking** | A1 CPS **< A5 CPS**, sign test p < 0.10 across tasks |

**The two thresholds rest on different task sets, and `analyse.py` says so beside each number.**
T4 is computed over the five tasks that run all three arms; T7 over the seven that run A1 and A5,
because `E1-untested-change` and `N1-null-task` joined for the ranking comparison only — A0
contributes nothing to a ranking comparison and running it would cost five runs a task for no
statistical power. Each threshold line names its own task set and size, the arm table carries a
`tasks` column per arm, and `summary.json` carries `task_sets` plus a `task_set`/`n_task_set` on
each threshold. One corpus, two sample sizes.

Five tasks cannot carry a release gate, so `analyse.py` prints `MEETS T4` / `does not meet T4`
and `MEETS T7` / `does not meet T7` as reported facts, and refuses to evaluate either below three
surviving tasks. T7's p-value is a **one-sided** exact binomial over the non-tied per-task CPS
deltas — one-sided because the threshold is directional ("A1 CPS *<* A5 CPS"), not "the two
differ". Both rules were written into `analyse.py` before any sweep ran, so neither was chosen
after seeing the numbers.

Read T7 next to the empty and lexical counts on the same line. A task where A1's per-prompt
package was empty contributes a delta that measures A0-plus-a-session-summary against BM25; a
task where it came from the lexical fallback contributes a delta between BM25 and itself, which
is a tie by construction rather than a ranking result — see the
[pre-sweep measurement](#pre-sweep-measurement-a1-receives-less-context-than-a5-on-all-five-tasks).

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

Evidence about the instrument, kept because it is the strongest thing this build produced. Each
was a finished artefact — four of them written into the plan text — and none was caught by
reading it. The first four were caught by writing a *different* correct fix and running it; the
last two by a smoke run and by opening the `pom.xml`:

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
make bench                # 95 paid runs on claude-opus-5. Hours. Real money.
python3 scripts/eval/analyse.py docs/eval/runs/<stamp>
```

**Credentials — prefer `ANTHROPIC_API_KEY` for a sweep.** `run.sh` copies
`~/.claude/.credentials.json` into a throwaway per-run directory and mounts it read-write at
`/root/.claude`, because Claude Code needs a writable state directory. A read-only mount was
tested (2026-09-06, invalid token, no spend) and **breaks authentication outright**: the agent
reports `Not logged in · Please run /login` instead of even attempting the token, and the
read-write control reaches `OAuth session expired and could not be refreshed`. So `:ro` is not
available as a mitigation. It would not be the right one either — the container's copy is a copy,
and the exposure is that a **refresh inside any of 95 root containers rotates the token
server-side**, which a read-only mount does not prevent. If that matters for your account, export
`ANTHROPIC_API_KEY` before the sweep: `run.sh` passes it into the container, an API key is not
rotated by use, and it can be revoked on its own.

**Pre-flight.** `sweep.sh` runs `scripts/eval/test_grade.sh` before it spends anything and
refuses to start if it fails. A grader stuck at `passed: false` reads as a devastating result
for every arm rather than as a bug, and finding that out after 95 paid runs is the single most
expensive mistake available here. `--skip-gate` exists for a re-run where nothing about
`grade.sh` changed.

**Resuming.** The sweep is resumable, and re-running it is the expected way to finish an
interrupted one — **but only with the original stamp**:

```bash
make bench STAMP=20260906T101500Z
```

Without a stamp the sweep mints a fresh one, starts a *new* run tree, and pays again for every
cell that was already done. `sweep.sh` prints its stamp as its first line, on every invocation,
for exactly this reason.

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

`meta.json` pins the image id, the nexus version and the model at the top of the run tree. A
resume that disagrees with **the image or the model** is refused rather than half-mixed in; the
nexus version is stamped and never compared, and the resume invocation above skips
`bench-image`, so that is precisely the path where the host binary can differ from the version
the tree claims.
