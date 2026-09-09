# The retrieval oracle on tokio — measuring selection without paying for outcome

**Status:** design, 2026-09-09. An extension of `crates/nexus-core/tests/debug_supply.rs` to the
corpus of [`tier2-corpus-verdict.md`](../../eval/tier2-corpus-verdict.md), not a new mechanism.
That test already asks the right question; it asks it of an 8 kB corpus where the answer cannot
matter.

**One sentence.** Score the context package for each of the five tokio tasks against the
`required_sites` the grader already uses on the agent's diff, ratchet the result, and make
"did retrieval point at the right file" a free, deterministic check instead of a $28 inference.

---

## 1. Why this exists

Run `20260907T093144Z` cost $28.88 and produced one diagnosis worth having: on
`R2-lines-codec-invalid-utf8`, A1's entire package was three symbols, all wrong — two of them
seeded off the ordinary English words *"invalid"* and *"instead"* occurring inside unrelated
Rust identifiers. Required was `tokio-util/src/codec/lines_codec.rs`. Nothing from that file
was injected.

**That finding needed no model, no containers, and no money.** It is a property of the package,
computable in the time a scan takes. It cost $28.88 and two hours because the only instrument
that could see it was a 75-cell agent sweep.

`debug_supply.rs` was built to be the cheap instrument and states the argument exactly:

> if the package does not contain the files the fix touches, no token saving is possible.

It runs on the generated fixtures, over three planted bugs, and scores 0 of 3. Those fixtures
are 6–8 kB — [`tier2-corpus-verdict.md`](../../eval/tier2-corpus-verdict.md) established that a
bare agent reads them whole and that no retrieval question posed there means anything. The test
is right and its corpus is wrong, which is the same diagnosis that document reached about
Tier 2 and for the same reason.

The tokio corpus fixes the scale. This spec points the existing test at it.

## 2. What this can and cannot detect

Stated before the design, because an instrument that hides its resolution is worse than none.

| Quantity | At 5 tasks | Treatment |
|---|---|---|
| **Did the package name the required file** | Deterministic, exact | **The measurement.** |
| **Did it name it for the right reason** | Not visible at file granularity | Not claimed. See §8. |
| **Whether the agent then used it** | Requires a model | Out of scope — that is Tier 2's question |
| **Ranking quality among correct items** | Not asserted | `golden_packages.rs` covers drift on its own fixture |

A package scoring 1.0 here has cleared the floor below which no token saving is possible. It has
not been shown to be good. The two claims are different sizes and this document never conflates
them.

## 3. What is measured

For each task in `tests/fixtures/corpora/tokio/fixture.toml`:

1. Check out the task's pinned `sha` in the fixture clone.
2. `nexus scan` it.
3. Build the task package from the task's `prompt`.
4. **Site recall** = required sites the package names, over `required_sites`.

Ground truth is `required_sites`, already curated per task and already used by `grade.sh` to
score the agent's diff (`L3_sites_found` / `L3_sites_missed`). This spec introduces no new
ground truth; it points the existing oracle at the retrieval that preceded the diff instead of
at the diff.

This is a **stronger** ground truth than `debug_supply.rs` has today. That test's own header
records the weakness — only one of its three bugs has a `fixed_by` commit, and the other two
fall back to "the file the planted anchor names", which a package can satisfy while omitting
the file where the repair goes. Every tokio task has curated sites, so the fallback rule is not
needed and is not carried over.

File granularity throughout: `required_sites` are files, package items carry file paths.

## 4. Where it runs, and why not in `cargo test`

`debug_supply.rs` runs under `cargo test` because its fixtures are committed. The tokio corpus
is a clone built by `make tokio-fixture` and is absent on a fresh checkout — so the same
treatment would produce a test that skips silently on every machine that has not built it,
including CI. By the doctrine `golden_packages.rs` already states about re-baselining without
reading, a check that quietly does nothing is worse than no check, because it reports green.

Therefore:

- **`make tier1-retrieval`**, a target of its own. Not part of `make check`, which stays fast
  and needs no network.
- **Its own CI step**, after the existing smoke test — which already clones
  spring-petclinic, so a network-dependent step is precedented here rather than novel.
- **Refuses loudly** when `target/fixtures/tokio` is absent, naming `make tokio-fixture`.

The tokio cases live in the same test binary as `debug_supply.rs`, rather than in a second
harness that would drift from the first. That creates one tension worth naming, because the
obvious resolution is the wrong one: a test in that binary is also reached by
`cargo test --workspace`, where the corpus is usually absent, and a plain skip there is exactly
the silently-green check this section rejects.

So absence is fatal **or** skipped depending on who is asking, and the asking is explicit:

| invocation | corpus absent |
|---|---|
| `cargo test --workspace` / `make check` | skips, and says why |
| `make tier1-retrieval` and CI, which set `NEXUS_TIER1_REQUIRED=1` | **fails**, naming `make tokio-fixture` |

The skip can therefore never be the result of the run that gates. `make check` is not that run
and does not claim to be.

## 5. The ratchet, and why it is not a threshold

`debug_supply.rs` asserts no threshold, and its reasoning is correct and adopted here:

> a number chosen before the evidence exists is the folklore `11-risks.md` R8 names.

A ratchet is not that number. It asserts nothing about what recall *should* be — only that it
may not fall below what was measured. R2 is recorded at its true value today, which is 0.0. The
check goes green immediately, and cannot go quietly green after a ranking change destroys
recall.

- Baseline at `crates/nexus-core/tests/golden/retrieval_tokio.json`, beside the existing
  goldens.
- Per task: recall, which required sites were hit, and the package's item count.
- Failing condition: any task below its recorded recall.
- Raising one: `NEXUS_REBASELINE=1 make tier1-retrieval`, then read the diff. Same ritual,
  same wording, same warning as the two goldens already in that directory.
- The baseline carries `"target": 1.0` per task, so R2's gap stays visible in the file rather
  than being normalised into "what we score".

**Improvement requires a deliberate commit.** That is the property that makes this safe to
build before the seeding fix rather than after it.

## 6. Parity with the benchmark's A1 arm

The oracle must select what A1 selected, or it measures a different product. Verified against
`scripts/eval/arms/A1.json` and `scripts/eval/nexus-hook.sh`:

| | A1's hook | This oracle |
|---|---|---|
| entry | `nexus context --task <prompt> --budget 4000 --brief` | `TaskRequest` direct |
| budget | `4000` | `TASK_BUDGET_TOKENS`, which **is** 4000 (`context/mod.rs:26`) |
| rank | unset → `RankMode::default()` | `RankMode::default()` |
| purpose | unset → whatever the CLI declares | **must match; see below** |
| `--brief` | set | irrelevant — rendering only (`main.rs:1008`), not selection |

Budget parity is exact and `--brief` provably cannot move the selection.

**One divergence to close in implementation.** `debug_supply.rs` pins `purpose: Purpose::Debug`.
The hook passed no purpose flag, so the production path takes whatever `declared_purpose`
resolves to and lets the intent classifier run. The tokio cases must pass what the hook passed,
not a pinned purpose — intent is upstream of every weight in the ranker, so pinning it measures
a pipeline the benchmark never ran.

## 7. Non-goals

- **Not the rest of the package.** Freezing tokio's full item list would pin the ranker against
  five tasks. `golden_packages.rs` does that job on a fixture chosen for it.
- **Not weight tuning.** This makes tuning checkable. Roadmap 5.7 still wants a human citing
  ledger evidence in the commit message, and this spec does not relax that.
- **Not a replacement for Tier 2.** Selection is not outcome. That split is
  [`13-evaluation.md`](../../architecture/13-evaluation.md)'s and it stands.
- **Not a second package source, yet.** The baseline is keyed by variant so a graphify-seeded
  or BM25 arm can be added as one more key rather than a restructure. No such arm is built
  here — the key exists because leaving it out would make the first comparison a schema
  migration.

## 8. Risks

- **File-granularity recall is a floor, not a proof.** A package naming `lines_codec.rs` for
  entirely the wrong reason scores 1.0. Symbol granularity would be sharper and
  `required_sites` is not curated at that level; widening it is a corpus change, not a
  harness change.
- **Five tasks, the same n as the sweep.** A ratchet over five tasks is gameable by tuning to
  five tasks. The mitigation is structural: the ratchet prevents regression and rewards
  nothing, so there is no score to climb.
- **Clone cost in CI.** `tokio_fixture.sh` clones once and reuses; CI has no such cache, so
  this adds a tokio clone per run. `--filter=blob:none` or an actions cache is the answer;
  which one is an implementation measurement, not a design decision.
- **Scan cost.** tokio scans cold in 820 ms (`docs/eval/hook-latency.md`), five times over is
  seconds. Not a risk, recorded so nobody re-derives it.

## 9. Acceptance criteria

1. `make tier1-retrieval` runs the five tokio tasks and writes per-task recall.
2. It refuses, naming `make tokio-fixture`, when the corpus is absent — and does not skip.
3. `R2-lines-codec-invalid-utf8` reproduces the sweep's finding for $0 — expected recall
   **0.0**, since the sweep's package for that prompt named `time/error.rs` and
   `tests/test_clock.rs` and nothing from `codec/`. This is the acceptance test for the
   *instrument*, not a prediction to be satisfied: if the oracle scores R2 above 0.0, the
   parity claimed in §6 is wrong somewhere and must be found before any baseline is trusted.
4. A deliberate ranking change that lowers any task's recall fails the check.
5. `NEXUS_REBASELINE=1` raises the baseline and produces a readable diff.
6. The scoring logic has a self-test over synthetic packages, run before the real corpus —
   as `test_grade.sh` gates `sweep.sh`, so the oracle is tested before it grades anything.
7. CI runs it, and a fresh clone of this repository reproduces every number above.
