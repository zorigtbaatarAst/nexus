# Evaluation

How we prove Nexus works — or find out that it does not.

This document exists because [`10-roadmap.md`](10-roadmap.md) Phase 2 has a success criterion
that is currently unmeasurable: *"the token cost of a task drops, measurably."* Against what
baseline, on which tasks, judged how, and at what point do we conclude the idea was wrong?

---

## 1. What must be proven, and the failure this guards against

The product claim has two halves, and **only the pair is worth anything**:

> Nexus makes an agent's engineering outcomes **at least as good**, at **materially lower cost**.

The obvious trap: token cost alone is trivially optimised by sending nothing, or by the agent
giving up early. A benchmark that measures only tokens would score both as wins. So would a
context package that is cheap, confident and wrong (R1) — the failure mode this whole evaluation
is really designed to catch.

Hence three gates. **All three must pass.** A failure on any one is a failure overall, and no
amount of margin on the others compensates.

| Gate | Claim | Falsified by |
|---|---|---|
| **G1 — Outcome** | Engineering outcomes do not get worse | Correctness collapse on any task family |
| **G2 — Cost** | Cost per *solved* task drops materially | < 30 % median reduction in CPS |
| **G3 — No new harm** | Nexus introduces no new failure mode | Overhead on null tasks, or adjudicated harmful-context failures |

And one number carries the headline, because it fuses G1 and G2 and cannot be gamed:

```
                    total tokens across all runs of a task family
  cost-per-success = ─────────────────────────────────────────────
                        number of runs that passed L1 and L2
```

**Failing is expensive in CPS.** An arm that halves its tokens by giving up half the time scores
*worse*, not better. That property is the reason this is the headline metric and raw token count
is not.

---

## 2. Two tiers, because they answer different questions

| | **Tier 1 — Golden packages** | **Tier 2 — End-to-end benchmark** |
|---|---|---|
| Question | *Does the Context Engine select the right things?* | *Does that make the agent do better work more cheaply?* |
| Model involved | **None** | Yes |
| Deterministic | Yes, byte-exact after normalisation | No — stochastic, needs repetition |
| Cost | Free, milliseconds | Expensive, hours, real money |
| Runs | Every commit, in CI | Nightly on `main`; full ablation per release |
| Gate | Hard: any diff fails the build | Advisory per-run; hard at release |

Conflating them is how evaluation dies: an expensive stochastic suite run on every PR gets
disabled inside a fortnight. Tier 1 is what keeps the ranker honest day to day; Tier 2 is what
tells us whether the ranker is worth having at all.

---

## 3. The corpus

Built on the four fixtures already specified in [`testing-strategy.md`](../testing-strategy.md)
§3, which have scripted histories with planted bugs at known commits — exactly the property an
evaluation needs and that a scraped benchmark cannot provide.

| Repo | Stack | Role |
|---|---|---|
| `spring-payments` | Java 21 · Spring Boot · JPA | Primary. 7 commits: baseline → refactor → bug → reformat → rename → fix → regression |
| `next-storefront` | TypeScript · Next.js · Prisma | The cross-stack seam partner |
| `acme-monorepo` | Java · Gradle · 3 modules | Multi-service: sibling-vs-external resolution, narrow-scan detection. Families A, D |
| `legacy-billing` | Java · Spring | Legacy/deceptive: three plausible invoice calculators, one live. Family H's specialist |
| `fastapi-orders` | Python · FastAPI | Phase 5 language coverage — deferred, nothing can index it yet |
| `cargo-ledger` | Rust · axum · sqlx | Phase 5, and the dogfooding proxy — deferred for the same reason |
| **`spring-petclinic`** | Java · Spring | **Realism control.** Third-party, pinned by sha, not authored by us |

`acme-monorepo` and `legacy-billing` were added while building the corpus: families A, D and H
need a multi-service layout and a deceptive one, and neither is available in the four repositories
this table originally named. `fastapi-orders` and `cargo-ledger` are deferred to Phase 5, because
they exist to exercise Python and Rust analyzers that do not exist yet — generating them now would
produce repositories nothing can index.

The corpus is generated, not committed: see [`tests/fixtures/README.md`](../../tests/fixtures/README.md)
for the specification format, and `make fixtures` to build it.

`spring-petclinic` is not optional. Fixtures we wrote can accidentally encode our assumptions
about what matters; a repository we did not write cannot. It is already cloned by `make smoke`,
so the infrastructure exists.

### Contamination

Every fixture is authored for this purpose and unpublished, so it cannot be in any model's
training data. `spring-petclinic` **is** public and almost certainly memorised — which is why it
is the realism control rather than a primary measurement, and why its tasks are novel edits at a
pinned sha rather than reproductions of its own history.

### Task definition format

Machine-readable, hashed, and version-controlled at `tests/eval/tasks/*.toml`:

```toml
id            = "A3-dto-field-rename"
family        = "A"                       # impact / blast radius
repo          = "spring-payments"
commit        = "a1b2c3d"                 # pinned; the task's starting state
start_state   = "clean"                   # clean | dirty:<patch-file>
prompt        = "Rename the `amount` field on PaymentDto to `grossAmount`."
turns         = 1
required_sites = [                        # L3 completeness — every site that must change
  "src/main/java/mn/pay/PaymentDto.java",
  "src/main/java/mn/pay/PaymentMapper.java",
  "src/main/resources/graphql/payment.graphqls",
  "web/src/components/PaymentSummary.tsx",
]
hidden_tests  = ["tests/eval/hidden/A3/*.java", "tests/eval/hidden/A3/*.test.ts"]
convention_rules = []                     # L4, optional
timeout_s     = 900
```

`required_sites` is the honest half of grading. Hidden tests prove the change *works*; the site
list proves the agent found everything rather than getting lucky on what the tests happened to
cover.

---

## 4. Task families

Each family exists to falsify one specific claim. A family with no corresponding claim in
[`01-problem.md`](01-problem.md) does not belong in the corpus.

| | Family | Falsifies | n | Grading |
|---|---|---|---:|---|
| **A** | **Blast radius** — change something with non-local consequences | F2: reading finds relationships | 3 | Hidden tests + `required_sites` |
| **B** | **Cross-stack seam** — a backend contract change reaching frontend code | F3: the seam is invisible in source text | 2 | Backend tests + frontend typecheck + tests |
| **C** | **Regression memory** — a bug shaped like one fixed at an earlier commit | F1/F4: history is unavailable | 2 | Hidden test + prior-fix approach match |
| **D** | **Convention adherence** — the project enforces something non-obvious | F1: conventions are not in the file being edited | 2 | `convention_rules` (ArchUnit-style, deterministic) |
| **E** | **Verification honesty** — a change nothing tests; and a suite already red | F5: "done" is accepted | 2 | L5 honesty + correct `Verdict` |
| **N** | **Null** — a task Nexus genuinely cannot help with (a self-contained one-file fix) | *Overhead*: does Nexus cost when it cannot help? | 2 | Hidden test; **cost is the measurement** |
| **H** | **Harmful context** — a plausible-but-wrong region exists (a deprecated duplicate of the live path) | R1: the ranker is confidently wrong | 2 | Hidden test + **ledger adjudication** |
| **M** | **Multi-turn** — scripted referential turn sequences | Q2: single-prompt hooks lose the referent | 2 | Per-turn package + final hidden tests |

**17 tasks.** Two families are the ones most evaluations omit and both are essential:

- **N (null)** measures the cost of Nexus being present when it is useless. A system that helps
  on 60 % of tasks and taxes the other 40 % may be net-negative, and only this family shows it.
- **H (harmful)** is a direct probe of the defining risk. Without it, the benchmark cannot
  distinguish "the ranker is good" from "the hidden tests were forgiving".

---

## 5. Arms

Six arms. A comparison against no-Nexus alone cannot tell us *why* anything improved.

| Arm | Configuration | What it isolates |
|---|---|---|
| **A0** | No Nexus. Bare agent, standard tools | The baseline |
| **A1** | Nexus full — context + memory + verification hooks | The product |
| **A2** | Context hooks only, no verification | Value of the package alone |
| **A3** | Verification hook only, no context | Value of the gate alone |
| **A4** | Full, but memory disabled (no facts, no prior findings) | Value of persistence — the F1 claim |
| **A5** | **Lexical control.** Same token budget, same injection point, items chosen by BM25 over file contents instead of ranked by the Context Engine | **Value of *ranking*, as opposed to value of *any* context** |

**A5 is the scientifically load-bearing arm.** Without it, a win over A0 is ambiguous: it could
mean the graph, history and memory are doing work, or it could mean that injecting roughly the
right *volume* of roughly related text helps and the entire pipeline is elaborate decoration.

If A1 does not beat A5, the Context Engine has not earned its complexity, and the correct
response is to ship BM25 and delete several thousand lines. That must be a real possible outcome
of this evaluation or the evaluation is theatre.

---

## 6. Protocols

### 6.1 Baseline protocol (A0)

```
1. Fresh container from the pinned image
2. git clone <fixture> && git checkout <commit>
3. Apply start_state patch if dirty
4. NO .nexus/ directory. Nexus binary absent from PATH.
5. Standard agent harness, pinned version, pinned system prompt
6. Deliver `prompt` as the single user message (or the turn script for family M)
7. Run until the agent stops, or timeout_s
8. Capture: full transcript, all tool calls, token counts per turn, wall-clock, final diff
9. Grade (§7) in a separate container that never saw the agent
```

### 6.2 Nexus-enabled protocol (A1–A4)

Identical to A0 except steps 4 and 5:

```
4. nexus init && nexus scan       # baseline index built BEFORE the clock starts
   nexus init --hooks <subset per arm>
   Restore .nexus/ from a pinned snapshot where the task requires prior memory
   (families C, D and the memory-enabled arms) — see §6.4
5. Same harness, same system prompt, plus the arm's hooks and MCP server
```

**Indexing cost is measured but excluded from the per-task comparison**, and reported
separately as amortised setup. Charging every task the full scan cost would misrepresent
steady-state use, where a project is indexed once and rescanned incrementally; hiding it
entirely would misrepresent first contact. Both numbers get published.

### 6.3 Control arm (A5)

Same as A2, but `nexus context` is replaced by a shim that:
- takes the same prompt,
- ranks files by BM25 over their contents,
- emits the top files' first `WINDOW_LINES` up to the **same token budget**,
- injects at the **same hook point**.

Same volume, same position, no graph, no history, no memory. The only variable is selection.

### 6.4 Prior-memory snapshots

Families C and D require that a previous session already learned something — that is the entire
point of F1. Faking it with a hand-written `.nexus/` would test a fixture, not the product.

So the snapshot is **generated by a scripted prior session**, recorded once, and pinned:

```
tests/eval/snapshots/<task-id>/
  nexus.db            # produced by running the scripted prior session, then frozen
  provenance.md       # the exact commands that produced it, and the commit it was built at
```

The prior session is itself run through the harness and its transcript is kept. A snapshot whose
provenance cannot be re-executed is not a valid snapshot.

---

## 7. Correctness criteria

Six levels. **All grading is deterministic and automated.** Grading runs in a container that
never saw the agent, from the final git diff only.

| | Level | Check | Gate? |
|---|---|---|---|
| **L0** | Compiles | project build command exits 0 | Yes |
| **L1** | **Correct** | all `hidden_tests` pass | **Yes — primary** |
| **L2** | **No collateral damage** | every test passing at the start commit still passes | **Yes — primary** |
| **L3** | Complete | every path in `required_sites` appears in the diff | Reported |
| **L4** | Conventional | `convention_rules` hold | Gate for family D only |
| **L5** | **Honest** | the agent's final claim matches reality | Gate for family E only |

**A run "passes" when L0 ∧ L1 ∧ L2 hold.** That triple is what CPS divides by.

### On L3 — why a site list and not diff similarity

Comparing the agent's diff to a reference solution punishes correct alternative implementations,
which makes the benchmark measure conformity rather than correctness. `required_sites` asks a
weaker and more honest question: *did the agent find every place that had to change?* An agent
that passes L1 but misses a site got lucky on test coverage, and that gap is exactly what
Nexus's impact analysis claims to close. **L3 is reported, not gated**, because a genuinely
different approach may legitimately touch different files — but a systematic L3 gap alongside L1
passes is a finding in its own right.

### On L5 — honesty as a measurable outcome

Extract the agent's final assertion from the transcript and compare it to the graded result:

| Agent said | Reality | Outcome |
|---|---|---|
| done | L1 ∧ L2 | **honest-pass** |
| done | ¬(L1 ∧ L2) | **false-done** ← the failure verification exists to eliminate |
| blocked / uncertain | ¬(L1 ∧ L2) | honest-fail |
| blocked / uncertain | L1 ∧ L2 | under-confident |

**`false-done` rate is a first-class reported metric across every family**, not just E. If
Nexus's verification gate works, A1 and A3 must show a materially lower `false-done` rate than
A0 — and that is an engineering-outcome improvement that no token count would ever reveal.

Claim extraction is a fixed regex over the final assistant message against a published pattern
list, hand-audited once per release. Crude, deterministic, and auditable — which beats accurate
and irreproducible.

### No LLM judge in the gate

Pass/fail is never decided by a model. Three reasons, in order of weight: it makes the benchmark
irreproducible; a model grading a model-context system has an obvious conflict; and a stochastic
grader turns every regression investigation into an argument about the grader.

An LLM rubric may run as a **secondary diagnostic**, reported separately and never aggregated
into a verdict.

---

## 8. Metrics

Every run emits one JSON record. Nothing is aggregated that was not recorded per run.

### Cost

| Metric | Notes |
|---|---|
| `input_tokens`, `output_tokens`, `cache_read_tokens` | Separately — cache reads are cheaper and conflating them flatters whichever arm caches more |
| `tool_calls_total`, `tool_calls_by_name` | `Read`/`Grep` counts are the direct measure of exploration behaviour |
| `bytes_read` | Total bytes returned by file reads. The cleanest proxy for F2 |
| `turns` | Assistant turns to completion |
| `wall_clock_ms` | End to end |
| `nexus_overhead_ms` | Hook time only, summed. Reported separately — it is R2's measurement |
| `nexus_injected_tokens` | What Nexus added. Must be counted *against* Nexus |

`nexus_injected_tokens` is non-negotiable: a package that saves 5,000 exploration tokens by
spending 4,000 of its own has saved 1,000, and any accounting that omits it is dishonest.

### Outcome

`l0`…`l5` booleans · `false_done` · `sites_found / sites_required` · `pass@1` · `pass@5`

### Derived

- **`cost_per_success`** — the headline (§1)
- `tokens_per_task` — reported, never used as a gate on its own
- `exploration_ratio` = `bytes_read` / repo bytes — how much of the project the agent had to read

---

## 9. Statistical treatment, stated honestly

**N = 5 runs per (task × arm).** 17 tasks × 6 arms × 5 = **510 runs** for a full ablation.

Temperature is pinned at the harness default rather than 0: sampling variance is a real property
of the system under test, and measuring at temperature 0 would measure a configuration nobody
uses. Determinism comes from repetition, not from suppressing it.

**Report medians and IQR, not means.** Token distributions have long right tails — one run that
thrashes for 400,000 tokens moves a mean and tells you nothing about typical behaviour.

**Paired analysis.** Tasks are the pairing unit; comparisons are on per-task deltas via a paired
bootstrap (10,000 resamples) over task-level medians. A sign test across the 17 tasks is reported
alongside, because it makes no distributional assumption and is easy to check by hand.

### What this design can and cannot detect

This is the part benchmark documents usually omit.

| Measurement | Resolution at N = 5 × 17 |
|---|---|
| **Cost** (continuous, low variance) | Reliable. A 30 % CPS shift is comfortably detectable |
| **Correctness** (binomial, ~17 effective units) | **Coarse.** Detects a collapse, not a small regression. Minimum detectable difference in pass rate is roughly **15–20 pp** at 80 % power |
| **`false_done`** | Coarse, same reason. A large effect is visible; a 5 pp shift is not |

So the correctness gate is written as a **tripwire, not a precision instrument** (§11), and we
say so rather than implying a confidence the design does not support.

**To resolve 5 pp on correctness** would need roughly 30+ tasks × 10 runs ≈ 300 runs per arm.
That is Tier 3, scoped in §14, and it is not affordable until the coarse result justifies it.

---

## 10. Golden context packages (Tier 1)

Deterministic, model-free, and the thing that actually keeps the ranker honest between
benchmark runs.

### Fixture format

```
tests/eval/golden/<task-id>/
  request.json     # the exact TaskRequest
  package.json     # the normalised expected ContextPackage
  ledger.json      # the expected decision for EVERY candidate
  README.md        # why this package is right — prose, for the human re-baselining it
```

### Normalisation before comparison

Strip everything that varies without meaning: `duration_ms`, timestamps, absolute paths, the
`scan_uid`, and the package cache key.

### What is asserted, and what deliberately is not

| Asserted | Not asserted | Why |
|---|---|---|
| Exact **item set** (kind + anchor) | — | The selection is the product |
| **Rank order** of items | Exact float scores | Float equality is brittle and re-baselines for no reason |
| Score **band** (`high` ≥ 0.7 · `mid` 0.4–0.7 · `low` < 0.4) | — | Catches a real drift; survives a harmless one |
| **A decision for every candidate**, with a reason | — | An unexplained exclusion is a ledger bug |
| `tokens_estimated` within ± 10 % | Exact token count | It is an estimate and the doc says so |

**Rank order but not exact scores** is the single most important choice here. Asserting floats
produces a suite that fails on every weight nudge and is therefore re-baselined without being
read — at which point it is checking nothing.

### Re-baselining protocol

A golden diff **fails the build**. Updating requires:

1. `nexus eval golden --update <task-id>`, which rewrites the fixture **and** the `README.md`
   diff summary;
2. a commit message naming which weight or stage changed and **why the new package is better**;
3. review by someone who did not make the change.

Weight changes are cheap to make and expensive to evaluate, which is exactly the asymmetry that
produces folklore (R8). This protocol restores the balance.

---

## 11. Pass/fail thresholds

Evaluated per release, on the full ablation.

### Hard gates — any failure blocks the release

| # | Gate | Threshold | Rationale |
|---|---|---|---|
| **T1** | Golden packages | **100 % exact match** after normalisation | Deterministic. A miss is a bug, never noise |
| **T2** | Hook overhead | `nexus_overhead_ms` p95 **≤ 150 ms** | R2. Already the roadmap's number |
| **T3** | **Correctness tripwire** | A1 pass@1 ≥ A0 pass@1 **− 10 pp** overall, **and** no family where A1 loses on ≥ 4 of 5 runs while A0 passes | G1. 10 pp is set at what §9 can actually resolve; the per-family clause catches a localised collapse that an average hides |
| **T4** | **Cost** | Median CPS reduction **≥ 30 %** (A1 vs A0), paired bootstrap 95 % CI excluding 0 | G2. Below 30 % the maintenance burden is not obviously repaid |
| **T5** | **Null-task overhead** | Family N token increase **≤ 5 %** (A1 vs A0) | G3. Nexus must not tax tasks it cannot help |
| **T6** | **Harmful context** | **Zero** adjudicated harmful-context failures | G3, R1. See §12 |
| **T7** | **Ranking earns its keep** | A1 CPS **< A5 CPS**, sign test p < 0.10 across tasks | If ranking does not beat BM25, ship BM25 |
| **T8** | **Verification works** | A1 `false_done` **< A0 `false_done`**, absolute reduction ≥ 10 pp | The gate's entire purpose |

### Reported, not gated

`pass@5` · L3 completeness · `exploration_ratio` · per-arm ablation deltas · indexing cost ·
wall-clock · the LLM rubric.

Gating on a metric we cannot resolve (§9) would manufacture false confidence in one direction
and false alarms in the other.

---

## 12. Harmful-context adjudication (T6)

A harmful-context failure cannot be detected automatically — it requires knowing *why* the agent
went wrong. The protocol:

**Trigger.** Any task where A1 fails and A0 passes on the same task, in ≥ 3 of 5 paired runs.

**Evidence.** The failing run's `ContextPackage`, its `InclusionLedger`, and the transcript.

**Question.** Did the agent's wrong work follow from something the package included, or exclude
something the package should have included?

**Verdicts.**

| Verdict | Meaning | Consequence |
|---|---|---|
| `harmful` | The package surfaced a wrong region and the agent acted on it | **Blocks the release.** Ranking defect |
| `insufficient` | The right item was ranked but fell below budget | Budgeting defect. Reported, does not block |
| `unrelated` | The failure has no traceable link to the package | Noise. Recorded and re-run |

Adjudication is done by someone who did not write the ranker, and the verdict is committed with
its evidence to `tests/eval/adjudications/`. Family H exists to make sure this process is
exercised on every release rather than only when something goes wrong.

---

## 13. Dirty git trees — resolved

**Q3 from Phase 0.** The design assumed a clean tree; reality inverts that, because agent work is
uncommitted by definition. Two distinct problems, resolved separately.

### 13.1 The product problem: verification needs a baseline

Skipping the baseline run whenever the tree is dirty means skipping it in the common case,
leaving the four-cell matrix ([`08-verification.md`](08-verification.md) §3) unreachable almost
always.

**Resolution: a detached worktree, plus a per-sha cache.**

```
verify --changed on a dirty tree:
  baseline_sha = the commit HEAD points at
  if test_runs has a suite result for (baseline_sha, tool_versions):
      reuse it                                     # free
  else:
      git worktree add --detach <tmp> <baseline_sha>
      run the suite there                          # working tree untouched
      append the result to test_runs
      git worktree remove <tmp>
  run the suite at HEAD (the dirty tree, in place)
  judge the pair
```

Three properties that make this the right answer:

- **`git stash` is never used.** Mutating a developer's uncommitted work to run a check is an
  unacceptable risk of loss, and no amount of care makes it acceptable.
- **The baseline is computed once per commit, not once per verification.** An agent making
  twenty edits against one `HEAD` pays the baseline cost once — which is what makes this
  affordable on a `Stop` hook.
- **`test_runs` is already an append-only ledger** with the schema for this, currently dead. The
  cache is a `SELECT`, not new machinery.

Cache key is `(baseline_sha, tool_versions_json)`. Including tool versions is the same trap as
`scans.tool_versions_json`: a toolchain upgrade must invalidate the cached suite result, or the
baseline silently reflects a compiler nobody is using any more.

### 13.2 The measurement problem: dirty is a variable, not an accident

Two distinct starting states, and the corpus must name which it uses:

| Start state | Meaning | Used by |
|---|---|---|
| **`clean`** | Fresh checkout at the pinned commit | Default for A, B, C, D, N, H |
| **`dirty:<patch>`** | A pinned uncommitted patch is applied first — "work in progress" | Family E, family M, and a mirrored subset |

**Every task also becomes dirty as the agent works.** That is inherent, identical across arms,
and therefore not a confound — it is the condition under which the `PostToolUse` and `Stop` hooks
must function.

**The dirty mirror.** Three tasks (one each from A, B, D) are duplicated with a `dirty` start,
giving a paired clean-vs-dirty comparison on identical work. This isolates the cost and
correctness effect of starting mid-stream, which is otherwise entangled with everything else.

**Two additional assertions, both Tier 1 and both about silence:**

- **Cache correctness (R9).** Edit a file without committing; assert the package cache **misses**.
  A dirty-tree cache hit serves context describing code that no longer exists, and is
  indistinguishable from a correct answer — the exact profile of a bug that survives for months.
- **Basis disclosure.** Every `ContextPackage` built on a dirty tree carries `dirty: true`.
  Asserted, because an agent reasoning from a package must be able to tell what it was true of.

Family E includes the **already-red** scenario: start dirty, with a pre-existing failing test.
The required verdict is `Inconclusive`, never `Failed`. That single assertion is what
[ADR-025](decisions/ADR-025-verification-ships-as-a-gate-before-a-reproducer.md) identifies as
deciding whether the gate survives contact with a real project, so it is graded on every run.

---

## 14. Multi-turn tasks — resolved

**Q2 from Phase 0.** `UserPromptSubmit` sees one prompt. *"Now do the same for orders"* has no
anchors, no referent, and no useful seeds.

### 14.1 The product resolution: session state belongs to the harness

**Nexus stays stateless.** It does not store conversations (N14), does not track sessions, and
does not become session-aware — that path leads to a daemon (N10) and to exactly the transcript
storage the memory design refuses.

The harness already *has* the conversation. So the hook supplies what Nexus cannot know:

```bash
nexus context --task "$PROMPT" \
              --carry-seeds "mn.pay.PaymentService,mn.pay.PaymentController" \
              --recent "$PREVIOUS_USER_PROMPT"
```

| Parameter | Supplied by | Nexus's use |
|---|---|---|
| `--carry-seeds` | the hook, from the previous package's top-ranked anchors | Prior seeds enter stage 2 at reduced weight (`w_carry`, default 0.5) |
| `--recent` | the hook, the previous user message only | Intent classification only. **Never stored, never indexed** |

Three properties make this acceptable rather than a slide into session state:

1. **Nexus remains a pure function** of (request, index, memory). Same inputs, same package —
   which is what keeps golden packages meaningful.
2. **Nothing is persisted.** `--recent` reaches the intent verb table and is discarded. It never
   touches `facts` and never reaches the store.
3. **Agent-agnosticism holds.** Any harness that has a conversation can pass these flags; a
   harness that cannot simply omits them and gets the single-turn behaviour.

**Referential-prompt detection** is deterministic and stays in the verb table: a prompt whose
target extraction is empty *and* which contains a referential marker (`the same`, `that`, `it`,
`those`, `also`, `now do`) sets `Intent::Referential`, which raises `w_carry` to 1.0 and — when
carry-seeds are absent — reports `Unknown` rather than guessing. An empty-anchored prompt with no
carried seeds is a case where Nexus genuinely does not know, and saying so beats inventing seeds.

### 14.2 The measurement resolution

**Turn scripts.** Family M tasks define a fixed sequence delivered as separate user messages:

```toml
turns = [
  "Add optimistic locking to PaymentService.",
  "Now do the same for OrderService.",            # referential — no anchors
  "Are the tests still passing?",                 # verification-shaped
]
```

Scripted, not adaptive: an adaptive follow-up would differ per arm and destroy the comparison.
The trade is realism for controllability, and controllability wins — a benchmark that changes its
questions per arm measures nothing.

**Attribution.** In multi-turn, no metric can honestly attribute the final outcome to a single
turn's context. So the measurement is split, and neither half is presented as the other:

| Level | Metric | Meaning |
|---|---|---|
| **Task** | L0–L5 after the final turn, total cost across all turns | Did the whole exchange succeed, and at what cost |
| **Turn** | **Anchor retention** — of the anchors *required* for turn N, the fraction present in turn N's package | Did the package survive the referential prompt |

`required_anchors` per turn is declared in the task file, exactly as `required_sites` is for the
whole task. For turn 2 above, the required anchors are `OrderService` **and** the
`PaymentService` locking change from turn 1 — the second being the thing a stateless
single-prompt package would lose, which is precisely the failure this measures.

**Threshold (Tier 1, deterministic — no model needed):** anchor retention **≥ 0.8** on referential
turns. This is asserted as a golden-package test, because carry-seed handling is pure selection
and needs no agent to evaluate.

**Ablation.** Family M is additionally run with `--carry-seeds` suppressed. If retention and CPS
do not degrade, carry-forward is not earning its place and should be deleted. The mechanism has
to justify itself on the same terms as everything else.

---

## 15. Reproducibility

**A run without a complete manifest is not a result.** It is not evidence, it is not quoted, and
it does not enter an aggregate.

```json
{
  "run_id": "2026-09-02T14:03:11Z-A3-A1-r2",
  "task_id": "A3-dto-field-rename",  "task_hash": "blake3:…",
  "arm": "A1",  "repetition": 2,
  "repo_commit": "a1b2c3d",  "start_state": "clean",
  "nexus_version": "0.4.0",  "nexus_git_sha": "…",
  "tool_versions_json": "…",
  "snapshot_id": null,
  "model_id": "…",  "model_version": "…",  "temperature": 1.0,
  "harness_version": "…",  "system_prompt_hash": "blake3:…",
  "hooks_enabled": ["SessionStart","UserPromptSubmit","PostToolUse","Stop"],
  "weights_hash": "blake3:…",
  "container_image": "sha256:…",
  "started_at": "…", "ended_at": "…",
  "metrics": { }, "grades": { },
  "artifacts": { "transcript": "…", "diff": "…", "packages": ["…"], "ledgers": ["…"] }
}
```

### Requirements

1. **Fresh container per run.** No state crosses runs. The image is pinned by digest.
2. **Network denied except the model endpoint.** Otherwise a task can be solved by fetching an
   answer, and the benchmark quietly measures search.
3. **`.nexus/` is rebuilt or restored from a pinned snapshot** — never inherited from a previous
   run.
4. **Every package and ledger the run produced is archived.** Without them, §12 adjudication is
   impossible after the fact, and adjudication after the fact is the only kind there is.
5. **`weights_hash` is recorded.** A cost comparison across different ranking weights is not a
   comparison.
6. **Grading is a separate container** that receives only the final diff and the repo. It cannot
   see the transcript, so it cannot be influenced by what the agent said about its own work.
7. **Model non-determinism is acknowledged.** Provider-side changes can shift results without any
   local change, so `model_version` is recorded and a **quarterly A0 re-baseline** is mandatory.
   A cost improvement measured against a six-month-old baseline may be measuring the provider.

### Honest limits, stated in the report

- Fixtures are small; conclusions may not transfer to a 500 KLOC monorepo.
- `spring-petclinic` is public and likely memorised.
- Four of five repos were authored by us and can encode our assumptions.
- Correctness resolution is coarse (§9).
- Grading rests on hidden tests: a task whose tests are weak scores an incomplete fix as a pass.

---

## 16. What this evaluation cannot tell us

Naming these keeps the results from being over-claimed later:

- **Whether developers like it.** Adoption is a different measurement, and a system can win every
  gate here and still be switched off (R2, R10).
- **Whether it works on a large monorepo.** Requires a large monorepo. Tier 3.
- **Whether it generalises across models.** Every number is per-model. Cross-model comparison
  needs the suite re-run per model, and results must not be quoted without naming one.
- **Long-horizon memory value.** The strongest F1 claim — a conclusion retrieved months later —
  cannot be simulated in a 15-minute run. Snapshots (§6.4) approximate it; only field use proves
  it.
- **Whether the agent *used* the context or merely received it.** Injection is not attention.
  `exploration_ratio` and L3 are proxies; neither is proof.

---

## 17. Phasing and cost

Ordered so the cheapest evidence arrives first, and so that a negative result stops the spend
early.

| Tier | Scope | Runs | When | Blocks |
|---|---|---:|---|---|
| **T1 — Golden** | Packages, ledgers, cache-miss, anchor retention | 0 model runs | Every commit | The build |
| **T2 — Core** | A0 vs A1, all 17 tasks × 5 | 170 | Nightly on `main` | Nothing; trend-tracked |
| **T3 — Full ablation** | A0–A5 × 17 × 5 | 510 | Per release | The release (§11) |
| **T4 — Expansion** | 30+ tasks × 10, large monorepo, second model | 600+ per arm | Only if T3 passes | Nothing |

**Build order.** T1 lands with Phase 2 — the golden fixtures *are* Phase 2's definition of done,
not a follow-up. T2 needs the fixture repositories to exist, which
[`testing-strategy.md`](../testing-strategy.md) §3 already specifies and which nothing has yet
built; that is the real prerequisite and it should be scheduled as such.

---

## 18. Kill criteria

The point of naming these before running anything is that we cannot argue our way out of them
afterwards.

**Stop and redesign if, on a passing-quality run of T3:**

1. **T4 fails badly** — CPS reduction under 10 %. The core hypothesis is wrong: better selection
   does not pay for itself at this scale.
2. **T7 fails** — A1 does not beat A5. Ranking does not beat BM25 at equal budget. Ship the
   lexical shim, delete the pipeline, keep the index for impact queries.
3. **T3 fails** — correctness collapses. Better-cheaper-worse is not a product, and no cost
   result rescues it.
4. **T6 fails repeatedly** across releases. The ranker is structurally prone to confident wrong
   context, and the mitigation has to change, not the weights.

**Reduce scope, do not stop, if:**

- Only A2 (context) wins and A3 (verification) does not — ship the Context Engine, defer the
  gate.
- Only A3 wins and A2 does not — ship verification, and reconsider whether the Context Engine
  earns its complexity at all.
- A4 ≈ A1 — memory contributes nothing measurable yet. Keep it (its value is long-horizon and
  §16 says this evaluation cannot see it) but stop investing until field evidence appears.

Writing the kill criteria down while we still like the idea is the only time they can be written
honestly.
