# Risks

Ranked by expected damage — likelihood × cost of discovering it late. Each carries a **detection
signal**, because a risk you cannot notice materialising is not managed, only listed.

---

## R1 — The ranker is confidently wrong

**Likelihood:** high · **Impact:** severe · **Phase:** 2 onward

Bad context arrives looking exactly like good context. The agent has no way to tell it was
misled, so it reasons well from the wrong premises and produces confident, wrong work. This is
strictly worse than no context, because no context at least prompts the agent to go and look.

**Detection:** golden-package fixtures whose contents are asserted, plus **task family H and the
adjudication protocol** in [`13-evaluation.md`](13-evaluation.md) §12 — a harmful-context verdict
blocks a release. Gate T6.

**Mitigation:** the inclusion ledger is **mandatory output, not a debug flag**, and it records
*every* candidate's decision, not only the winners. The failure that matters most is the right
file that never made it in, and a ranker that only explains its inclusions cannot surface it.

**Residual:** real. Ranking quality is an empirical property and cannot be fully established
before real use. This is why Phase 2 ships the ledger *before* weight tuning.

---

## R2 — Hook latency drives adoption to zero

**Likelihood:** medium · **Impact:** severe · **Phase:** 1 onward

A `UserPromptSubmit` hook sits on the developer's critical path. At 400 ms it is noticeable; at
1 s it is intolerable. It gets disabled once, and with it the entire deterministic-invocation
tier — which is the mechanism that distinguishes this design from the status quo.

**Detection:** p95 asserted in CI at **< 150 ms** on the 880-file fixture.

**Mitigation:** hard timeout, fail-open with `exit 0`, package cache keyed on
`(intent, seeds, HEAD, dirty hash, budget, weights)`, and **off by default** until measured on
the developer's own project.

**Note:** if latency proves unfixable by caching, N10's daemon trigger has effectively fired
from a direction ADR-006 did not anticipate. That would be evidence, not defeat.

---

## R3 — Verification executes project commands

**Likelihood:** low · **Impact:** severe · **Phase:** 4

The largest security surface in the system. Running a project's build and test commands means
executing code from the repository.

**Detection:** boundary tests plus an explicit test that `policy.execute = "none"` yields
`permission_required` over MCP and never an execution.

**Mitigation:** entirely inherited, nothing invented — allowlist templates with typed holes,
argv only, **`sh -c` never**, timeouts on every process, bounded output capture, and
`SafeWriter` rooted at `.nexus/generated-tests/` canonicalising the parent path *before* the
prefix check.

**Residual:** a malicious repository can still cause execution of its own build scripts — but
only what the developer's own `make test` would have run. Nexus adds no privilege.

---

## R4 — Scope collapse under ten pillars

**Likelihood:** high · **Impact:** high · **Phase:** all

Ten capabilities is enough scope to build all of them badly. The characteristic failure is a
system where every subsystem is 60 % done and none is trustworthy.

**Detection:** each phase has a definition of done that is a *test*, not a feeling. A phase is
not complete because it feels complete.

**Mitigation:** Phase 1 ships the smallest useful thing. Phase 2 ships three pillars. Everything
else is triggered, and the triggers are already written down in
[`12-non-goals.md`](12-non-goals.md).

---

## R5 — Memory rots and reads as authority

**Likelihood:** **certain — it is happening now** · **Impact:** high · **Phase:** 1

`facts.invalidated_at` is read in the retrieval query and **written nowhere in the workspace**. A
fact anchored at `PaymentService#pay():48` survives that method's deletion and is served forever
as established knowledge, pointing at a line that no longer means what it did.

This is not a projected risk. It is a live correctness bug, and the exact trap
[`memory-model.md`](../memory-model.md) §2 warns against.

**Detection:** a test that records a fact, edits the anchored symbol, and asserts the fact stops
surfacing while the row still exists.

**Mitigation:** invalidation-on-change, ~50 lines, scheduled in **Phase 1** rather than with the
rest of the memory work — because every later memory improvement compounds on top of rot until
it lands.

---

## R6 — Nexus cannot index itself

**Likelihood:** certain · **Impact:** medium · **Phase:** until 5

`nexus scan` on this repository reports **113 files, 0 symbols, 0 edges**. There is no Rust
analyzer, so every architectural decision in these documents is untested against the codebase
that implements them, and the team cannot dogfood its own product.

**Detection:** the number above is the acceptance test for Phase 5.2.

**Mitigation:** Graphify covers the structural gap meanwhile
(`graphify update crates --no-cluster`, free, seconds). `nexus-lang-rust` closes it.

**Honesty note:** this is stated rather than hidden because it bounds the confidence of every
claim made here.

---

## R7 — `nexus-core` becomes a larger god object

**Likelihood:** medium · **Impact:** medium · **Phase:** 1

`engine.rs` is already 2,069 lines with a 522-line `rescan` and a 239-line `analyze`. Four
subsystems are about to land on it.

**Detection:** a line-count ceiling per file in CI is crude but effective, and crude is fine here.

**Mitigation:** splitting `engine.rs` is **Phase 1.1** — a precondition, not a follow-up. `context`,
`history` and `query` are separate modules from their first line.

---

## R8 — Ranking weights become folklore

**Likelihood:** medium · **Impact:** medium · **Phase:** 2 onward

Weights tuned once by feel, never revisited, eventually defended by "it broke when we changed it"
with nobody able to say why.

**Detection:** golden packages fail when weights move, forcing the change to be deliberate.

**Mitigation:** weights are **data in `policy.toml`, not code**, and `--explain` decomposes every
score into its terms. Tuning is a config change with visible consequences.

---

## R9 — The package cache serves stale context

**Likelihood:** low · **Impact:** high · **Phase:** 2

A cache hit on a dirty working tree returns context describing code that no longer exists. Silent,
and indistinguishable from a correct answer.

**Detection:** a test that edits a file *without committing* and asserts a cache miss.

**Mitigation:** the dirty hash is part of the cache key. `nexus-vcs` already computes dirty state
correctly, including untracked files. Asserted as a Tier 1 test in
[`13-evaluation.md`](13-evaluation.md) §13.2.

---

## R10 — Nexus becomes another thing to configure

**Likelihood:** medium · **Impact:** high · **Phase:** all

The mission is "the developer keeps working normally". Every required flag, config file and setup
step erodes it, and a tool with a setup guide is a tool most people never finish setting up.

**Detection:** count the steps between `cd project && claude` and useful output. The target is
zero.

**Mitigation:** `detect.rs` infers the profile from evidence; `.nexus/` is created automatically
and self-gitignored; hooks are one opt-in command; weights have working defaults nobody must
read.

---

## R11 — The seam decays silently

**Likelihood:** low · **Impact:** high · **Phase:** all

The GraphQL/HTTP seam is the single capability nothing else provides. It rests on schema
indexing, resolver matching and operation extraction — all of which can degrade without any
error appearing. The `sibling`-vs-`external` incident is precedent: **6,247 of 9,514 "external"
edges were the project's own code**, and `impact` answered "no symbol matches" for a base class
half the codebase extends.

**Detection:** resolution-rate assertions per tier on the golden fixtures. A drop is a failing
test, not a slow decline nobody notices.

**Mitigation:** already partly built — `sibling` counts in the denominator, `external` does not,
and a high `sibling` count is reported as *the scan is too narrow* rather than as a broken
analyzer.

---

## R12 — Hooks fight the harness

**Likelihood:** low · **Impact:** medium · **Phase:** 1

Hooks are a Claude Code interface, and interfaces move. A hook contract change, a conflict with
another plugin's hooks, or an ordering surprise could break sessions.

**Detection:** hooks fail open — the failure is invisible to the developer *by construction*,
which is also what makes it hard to notice. `nexus doctor` must therefore report hook health
explicitly.

**Mitigation:** hooks contain **no logic** — each is `nexus <verb>` with a timeout. The
intelligence is in the binary, so a hook regression costs the automatic path and nothing else;
MCP and commands still work.

---

## Risk summary

| | Risk | Likelihood | Impact | Phase |
|---|---|---|---|---|
| R1 | Ranker confidently wrong | high | severe | 2+ |
| R2 | Hook latency kills adoption | medium | severe | 1+ |
| R3 | Verification executes project code | low | severe | 4 |
| R4 | Scope collapse | high | high | all |
| R5 | **Memory rot — already happening** | certain | high | 1 |
| R6 | Cannot index itself | certain | medium | →5 |
| R7 | Core god object | medium | medium | 1 |
| R8 | Weights become folklore | medium | medium | 2+ |
| R9 | Stale cache | low | high | 2 |
| R10 | Becomes a thing to configure | medium | high | all |
| R11 | Seam decays silently | low | high | all |
| R12 | Hooks fight the harness | low | medium | 1 |

**The two that decide whether this succeeds are R1 and R2.** Everything else is engineering that
can be corrected once noticed. A ranker that is quietly wrong and a hook that is quietly slow are
the two failures that would make Nexus worse than not having it — and both are silent by default,
which is why both are given explicit detection signals above.
