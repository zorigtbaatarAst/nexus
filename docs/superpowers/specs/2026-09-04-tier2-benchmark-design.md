# Tier 2 — does Nexus make the agent do better work more cheaply?

**Status:** design, 2026-09-04. A slice of [`13-evaluation.md`](../../architecture/13-evaluation.md),
not a replacement for it: that document is the full design and stays authoritative. This
narrows it to the smallest cut that can produce a defensible number, and says plainly what
that number can and cannot support.

**One sentence.** Run five real tasks through three configurations of the same agent, five
times each, grade the results deterministically, and report cost-per-success — with the
lexical control that decides whether the Context Engine has earned its complexity.

---

## 1. Why this exists

Every efficiency claim Nexus makes today is an estimate. The figure in both READMEs — ~34,000
tokens against ~1,500 — is arithmetic in a design document comparing *reading ten files* with
*one index query*: two lookups, not two bug fixes. `13-evaluation.md` names the real metric in
goal **G2** — *"cost per solved task drops materially"*, falsified below a 30 % median CPS
reduction — and its opening section records that the criterion is currently unmeasurable.

Two measurements landed on 2026-09-04 and neither closes this gap. `make eval` measures
whether a resolved edge points at the right symbol (precision 0.723). The debug-supply golden
measures whether a context package contains the files a fix must touch (0 of 3). Both are
about the *supply*. Neither asks whether an agent given that supply does better work for less
money, which is the product's actual claim.

**This is the first measurement that could falsify the product.** `13-evaluation.md` §5 states
the consequence and this spec adopts it without softening: if A1 does not beat A5, the Context
Engine has not earned its complexity, and the correct response is to ship BM25 and delete
several thousand lines.

---

## 2. What this can and cannot detect

Stated before the design, because a benchmark that hides its resolution is worse than none.

| Quantity | At 5 tasks × 5 runs | Treatment |
|---|---|---|
| **Cost** (tokens; continuous, low variance) | Usable. A large CPS shift is visible | **Headline. The only claim this slice supports.** |
| **Correctness** (binomial, ~5 effective units) | Very coarse — detects a collapse, nothing finer | Tripwire, reported, never a gate |
| **`false_done`** (binomial) | Very coarse | Reported |
| **L3 completeness** (did it find every site) | Per-task, deterministic | Reported; a systematic gap is a finding |

`13-evaluation.md` §9 puts correctness resolution at 15–20 pp with **17** tasks. At five it is
weaker still. So the write-up leads with cost, and every correctness statement carries the
sample size next to it. Anyone quoting a correctness delta from this slice is misusing it.

---

## 3. The tasks

Five, already defined in the corpus with prompts, pinned commits and `required_sites`. Three
families, chosen so the benchmark cannot flatter itself:

| Task | Family | Repo | Commit | Sites | Why this one |
|---|---|---|---|---|---|
| `A1-idempotency-key-length` | A | spring-payments | c2 | 3 | Three sites in three languages, none naming the others |
| `A2-shared-type-change` | A | acme-monorepo | c2 | 3 | A shared type changing across service boundaries |
| `B1-rename-crosses-the-seam` | B | next-storefront | c2 | 5 | Java → `.graphqls` → TypeScript → React. **The differentiated claim** |
| `B2-orphaned-field-diagnosis` | B | next-storefront | c3 | 3 | *"The orders page shows NaN."* Symptom-worded |
| `C1-regression-recognised` | C | spring-payments | c7 | 1 | *"Double-charged again."* The regression, in a SQL migration |

**A and B are where Nexus should look best; B2 and C1 are where it may look worst**, and they
are in on purpose. Both are symptom-worded, and the debug-supply golden recorded that
symptom-worded prompts select nothing at all. If that holds here, this benchmark will show it
rather than average it away.

C1's single `required_site` is a directory, and its anchor is a `.sql` migration no analyzer
indexes. That is a known limit, recorded rather than excluded.

---

## 4. The arms

Three of the design's six. Each is a Claude Code configuration, not a code path — which is the
point: the arms are the product's own integration surface.

| Arm | Configuration | Isolates |
|---|---|---|
| **A0** | No Nexus. No hooks, no MCP server, bare agent | The baseline |
| **A1** | Nexus full: `init --hooks --verify`, MCP server available | The product |
| **A5** | Same budget, same injection point, `nexus context --rank lexical` | **The value of ranking, as against the value of any context** |

**A5 is why this is worth running.** A win over A0 alone is ambiguous: it could mean the graph,
history and memory do work, or that injecting roughly the right *volume* of roughly related
text helps and the pipeline is decoration.

**Not in this slice:** A2 (context only), A3 (verification only), A4 (memory disabled). They
answer ablation questions that only matter once A1 beats A5.

---

## 5. The lexical control

`nexus context --rank lexical` — BM25 over file contents, same seeding entry point, same
budget accounting, same serialisation, same hook. Only the ranking function differs, which is
exactly the comparison.

It lives in the product binary rather than the harness, deliberately. A separate
implementation would have to reproduce the package format, the token budgeting and the hook
output, and any drift between the two would make A1-vs-A5 a comparison of two serialisers.
That bug would be invisible and would favour whichever side was better maintained.

The cost is a benchmark-only surface in a shipped binary, against `09-tooling.md`'s instinct.
It is undocumented, absent from `cli-spec.md`, and a test asserts it never appears in `--help`.

---

## 6. The runner

One Docker container per run. 75 runs: 5 tasks × 3 arms × 5 repetitions.

```
for task × arm × repetition:
  fresh container
    ├─ fixture generated, checked out at the task's pinned commit
    ├─ arm configuration applied (settings.json; scan+init for A1/A5)
    ├─ claude -p "<task prompt>"   --model claude-opus-5   timeout 900s
    └─ emit: git diff · transcript · token usage
  container destroyed
```

**Per-run containers, not per-arm.** Runs must be independent: a leftover `target/`, `.nexus/`
or cargo cache from run 3 reaching run 4 is contamination *inside* an arm, which is the worst
place to allow it.

Temperature is the harness default, not zero. Sampling variance is a real property of the
system under test; determinism comes from repetition.

**Token accounting must come from one source for every arm.** A0 has no hooks, so its input
arrives by a different path than A1's. If those are measured differently the comparison is
void — this is the single most dangerous implementation detail in the design, and §10 names it
as a risk with its own check.

---

## 7. Grading

Deterministic, automated, from the final git diff only, in a container that never saw the
agent. **No model decides pass or fail** — it would make the benchmark irreproducible, and a
model grading a model-context system has an obvious conflict.

| Level | Check | Role |
|---|---|---|
| **L0** | Project build exits 0 | Gate |
| **L1** | All hidden tests pass | **Gate — primary** |
| **L2** | Every test passing at the start commit still passes | **Gate** |
| **L3** | Every path in `required_sites` appears in the diff | Reported |
| **L5** | Final claim matches reality → `false_done` | Reported |

A run **passes** when L0 ∧ L1 ∧ L2. That triple is what CPS divides by.

`false_done` — the agent said done and it was not — is reported for every arm. If the
verification gate works, A1 must show a materially lower rate than A0, and that is an
engineering outcome no token count would reveal. Claim extraction is a fixed regex over the
final assistant message, hand-audited once.

### The hidden tests are the build

`tests/eval/hidden/<task-id>/` — written by hand, one directory per task, living in **this**
repo. The agent works in a generated fixture under `target/fixtures/`, so it never sees them;
contamination is solved by construction rather than by discipline.

They do not exist. Every task in the corpus already declares a `hidden_tests` path and every
one of those paths is empty, so L1 — the primary gate — is currently undefined for all five
tasks. **This is the bulk of the work and it has no shortcut.**

---

## 8. Output

One JSON record per run, under `docs/eval/runs/<timestamp>/`. Nothing is aggregated that was
not recorded per run.

```json
{
  "task": "B1-rename-crosses-the-seam", "arm": "A1", "repetition": 3,
  "model": "claude-opus-5", "nexus_version": "0.3.0", "fixture_sha": "…",
  "input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0,
  "wall_clock_s": 0, "turns": 0,
  "L0_build": true, "L1_hidden": true, "L2_collateral": true,
  "L3_sites_found": ["…"], "L3_sites_missed": ["…"],
  "claimed_done": true, "false_done": false
}
```

Cache reads are recorded separately from input tokens. Conflating them flatters whichever arm
caches more, and A1 injects a stable prefix every turn — exactly the shape that caches well.

**The threshold is pre-registered, not chosen afterwards.** `13-evaluation.md` §11 sets T4 at
a **median CPS reduction ≥ 30 % (A1 vs A0), paired bootstrap 95 % CI excluding zero**, on the
reasoning that below 30 % the maintenance burden is not obviously repaid. This slice does not
*gate* on T4 — five tasks cannot carry a release gate — but it reports against it, and the
number is fixed here so it cannot be moved once the result is known.

**Analysis:** medians and IQR, never means — token distributions have long right tails and one
thrashing run moves a mean without telling you anything. Comparisons are per-task deltas via a
paired bootstrap (10,000 resamples) over task-level medians, with a sign test reported
alongside because it assumes nothing and can be checked by hand.

---

## 9. What ships where

| Path | What |
|---|---|
| `tests/eval/hidden/<task-id>/` | Hand-written hidden tests. The build |
| `scripts/eval/run.sh` | One run: container, arm config, agent, capture |
| `scripts/eval/grade.sh` | L0–L3 from a diff, in a clean container |
| `scripts/eval/analyse.py` | Medians, IQR, paired bootstrap, sign test |
| `docs/eval/runs/` | Per-run records |
| `docs/eval/tier2.md` | The written result, with its sample size beside every claim |
| `crates/nexus-core` | `--rank lexical` |
| `Makefile` | `make bench` — never part of `make check` |

`make bench` is not `make check` and never will be: 75 containers and real money on the commit
path gets disabled inside a fortnight, which is what `13-evaluation.md` §2 says about exactly
this.

---

## 10. Risks

**R-a · The hidden tests encode one solution.** They are written by someone who knows the
intended fix, so they can accidentally grade conformity rather than correctness. Mitigation:
`required_sites` is the independent check, and L3 passes alongside L1 failures — or the
reverse — are the tell. Write each test against the task's *observable* behaviour, never
against the reference diff.

**R-b · Token accounting differs by arm.** §6. A0's input arrives without hooks; A1's includes
an injected package. If the two are counted from different sources the headline number is an
artefact. **Check:** run one task in A0 and A1 with a trivial prompt and assert the accounting
path is identical apart from the injected package's own tokens.

**R-c · The fixtures are small.** They are authored, unpublished — good for contamination,
bad for realism. A win on a 20-file fixture may not survive a 2,000-file repository, where the
selection problem is much harder and Nexus should look *better*. Stated as a limit, not
corrected here.

**R-d · Five tasks is a small n.** Handled by §2 rather than hidden: cost leads, correctness is
a tripwire, and the sample size travels with every number.

**R-e · The result may be that Nexus does not help.** That is a successful run of this
benchmark, not a failure of it. The design commits in advance: if A1 does not beat A5, that
finding is published in `docs/eval/tier2.md` with the same prominence a positive result would
get.

---

## 11. What this design does not do

Six arms, seventeen tasks, Tier 3, LLM-judged rubrics, and any threshold that gates a release.
It produces one number with its uncertainty stated, on five tasks, for three arms — enough to
know whether the expensive full ablation is worth running, and honest enough that the answer
might be no.
