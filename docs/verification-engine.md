# BugHunter — Verification Engine

This is the feature that separates BugHunter from a static analyzer with a chat window.
Anything can *suspect* a bug. Verification is the claim that BugHunter made the bug happen.

Owned by `bh-verify`. Never called implicitly — verification executes code, so it runs only
from `bughunter verify`, from `bughunter_verify_bug`, or from `rescan --verify` where the
user asked for it.

---

## 1. The pipeline

```
   UNVERIFIED bug
        │
   1. PLAN      ReproductionPlan { hypothesis, target, preconditions,
        │                          expected_failure, isolation, repetitions }
        ▼
   2. EMIT      test written under .bughunter/generated-tests/BUG-104/
        │       through SafeWriter — production code is unreachable
        ▼
   3. RUN NOW   current revision, in the sandbox
        │
        ▼
   4. RUN BEFORE  same test, baseline revision, detached read-only git worktree
        │
        ▼
   5. JUDGE     outcome + confidence delta, appended to bug_verifications
```

Steps 1 and 2 may involve an AI agent. Steps 3, 4 and 5 are entirely deterministic — and
they are the ones that decide the outcome. The model proposes; the test runner disposes.

---

## 2. Plan

A `ReproductionPlan` is a structured object, not prose:

```rust
struct ReproductionPlan {
    bug_id:           BugId,
    hypothesis:       String,          // one sentence: what will fail and why
    target:           SymbolRef,       // the symbol to exercise
    preconditions:    Vec<Precondition>,  // fixture state the test must establish
    trigger:          Trigger,         // Sequential | Concurrent{threads, iterations}
                                       // | Boundary{inputs} | Failure{inject}
    expected_failure: ExpectedFailure, // Assertion{..} | Exception{type} | Timeout | Invariant{..}
    isolation:        Isolation,       // Unit | Slice | Integration
    repetitions:      u8,              // 1 normally, 3 for concurrency
}
```

`isolation` is chosen by cost, cheapest first: `Unit` (no framework context), `Slice`
(`@DataJpaTest`, `@WebMvcTest`, a testcontainers-backed repository slice), `Integration`
(full context). A plan that demands `Integration` when `Unit` suffices makes verification
slow enough that people stop running it.

`expected_failure` is required. A verification that only asserts "something goes wrong"
cannot distinguish a reproduced bug from a broken test, and would happily "verify" a bug by
writing a test with a syntax error.

---

## 3. Emit

The generated test is written **only** under `.bughunter/generated-tests/BUG-<uid>/`:

```
.bughunter/generated-tests/BUG-104/
├── BugHunter_BUG104_DuplicatePaymentTest.java
├── plan.json          # the ReproductionPlan that produced it
└── README.md          # what this is, why it exists, how to delete it
```

Conventions that make a generated test unmistakable:

- Class name prefixed `BugHunter_` and containing the bug uid.
- A header comment carrying `BUGHUNTER-GENERATED`, the bug uid, the scan uid, the
  fingerprint and `DO NOT EDIT — regenerate with 'bughunter verify BUG-104'`.
- `tests.origin = 'bughunter'` in the store, so generated tests never pollute coverage
  statistics or the project's own test counts.
- The directory is gitignored by default. `bughunter verify --promote BUG-104` copies the
  test into the project's real test tree — an explicit, human-initiated action, never
  automatic.

### The SafeWriter jail

Constraint 10 — never modify production code during verification — is enforced by
construction rather than by care:

```rust
impl SafeWriter {
    fn new(root: &Path) -> Result<Self>;          // canonicalized at construction
    fn write(&self, rel: &Path, bytes: &[u8]) -> Result<PathBuf> {
        let target = self.root.join(rel);
        let canonical = canonicalize_parent(&target)?;   // resolves .. and symlinks
        if !canonical.starts_with(&self.root) {
            return Err(VerifyError::PathEscape { attempted: target });
        }
        // ...
    }
}
```

`bh-verify` exposes no other write path. Symlinks and `..` are resolved *before* the prefix
check, because a jail that compares unresolved paths is not a jail. An escape attempt is a
hard error and an `audit_events` row, not a warning.

The build system may still need to be told where the test lives. That is done by passing a
source root on the command line (`--tests`, `-Dtest=`, `testpath`) rather than by editing
`build.gradle` — BugHunter never writes to a build file.

---

## 4. Run — and why it runs twice

This is the part most bug-finding tools skip, and it is what makes the confidence number
mean something.

```
run_current  = execute(test, revision = HEAD)
run_baseline = execute(test, revision = baseline.commit)   # detached worktree, read-only
```

The baseline run happens in a **detached git worktree** under
`.bughunter/cache/worktrees/<sha>/`, created with `git worktree add --detach`, with the
generated test copied in. The primary working tree is never checked out, stashed, or
otherwise touched — a verification run that disturbs uncommitted work would be an
unforgivable thing for a tool to do to a developer.

If the baseline revision cannot be materialized (shallow clone, missing objects, dirty
baseline), the baseline run is skipped and the outcome is capped at `reproduced` without
the regression classification. The engine says so; it does not pretend.

**Repetitions.** `repetitions = 3` by default for `bug_type = 'concurrency'`. A race that
reproduces one time in three *is reproduced* — treating a single green run as disproof is
how flaky concurrency bugs get closed. Conversely 3/3 green is meaningfully stronger
evidence of non-reproduction than 1/1.

---

## 5. Judge

```
           run_current   run_baseline   → outcome                  confidence
           ───────────   ────────────     ──────────────────────   ──────────
           FAIL          PASS             reproduced               → ≥ 0.95, regression
           FAIL          FAIL             reproduced_preexisting   → ≥ 0.90, not introduced here
           FAIL          (unavailable)    reproduced               → ≥ 0.85
           PASS          PASS             not_reproduced           → × 0.5, stays UNVERIFIED
           PASS          FAIL             inconclusive             → unchanged, flag for human
           mixed n-of-m  any              flaky                    → capped at 0.75, flag
           error/timeout any              inconclusive/error       → unchanged
```

Rows worth dwelling on:

- **`reproduced_preexisting`** — the bug is real and now proven, but this change did not
  introduce it. Reporting it as a regression would send someone hunting through a diff that
  contains nothing relevant. The distinction costs one extra test run and saves an afternoon.
- **`PASS` now / `FAIL` before** is not "fixed". It might be, but the same evidence is
  produced by a test that is sensitive to something unrelated that also changed. It goes to
  a human. The status machine's rule that `FIXED` requires the *stored* reproduction test
  passing on a later revision is deliberately narrower than this cell.
- **`error`** — the test did not compile, the sandbox failed, the build broke. Confidence is
  left **unchanged**. Lowering it would punish the bug for the harness's failure; raising it
  would be absurd. Errors are surfaced with the captured output, never swallowed.

`not_reproduced` multiplies confidence rather than zeroing it. One failed attempt to
reproduce a concurrency bug is weak evidence of absence; three are strong. The multiplier
compounds naturally across attempts, and each attempt is its own immutable
`bug_verifications` row.

---

## 6. Sandbox

Decided with you: **Docker when available, host only with explicit opt-in.**
See [ADR-009](architecture-decisions.md#adr-009-docker-preferred-sandbox-with-explicit-host-opt-in)
and [security.md](security.md) §4.

```
policy.execute = "docker"    → container required; refuse if unavailable
policy.execute = "host"      → host allowed; explicit, committed, auditable
policy.execute = "none"      → generate the test, never run it (default until configured)
```

Container defaults: repository mounted **read-only**, a writable overlay for
`.bughunter/generated-tests` and the build cache, `--network=none` unless
`allow_network = true`, memory and CPU caps, and a wall-clock timeout (default 600 s) after
which the container is killed and the outcome is `inconclusive`.

Host execution is not a lesser citizen — testcontainers, GPU tests and licensed toolchains
make it necessary — but it is opt-in, recorded in `test_runs.sandbox`, and audit-logged.

**Commands are never strings.** They come from the allowlist in `policy.toml` as templates
with typed holes, expanded into an explicit argv. See [security.md](security.md) §3.

---

## 7. Test generation without an AI provider

Verification does not require an LLM. `bh-verify` ships deterministic templates per
`(bug_type, framework)` that cover the common shapes:

| bug_type | template |
|---|---|
| `concurrency` | N threads × M iterations against the target, invariant asserted after a latch |
| `null-safety` | boundary inputs from the signature: null, empty, absent optional |
| `transaction` | invoke inside a rolled-back transaction, assert the effect did not persist |
| `api-contract` | call with the previous signature's arguments, assert the response shape |
| `resource-leak` | run N iterations, assert handle/connection count is stable |

A template is filled from the `ReproductionPlan` — no model involved. An AI agent produces
*better* tests for logic and data-consistency bugs, which is why the agent path exists; but
`policy.ai = "off"` still gets a working verification engine for the mechanical bug classes.
That is constraint 3 applied to the feature most tempting to make AI-only.

---

## 8. Worked example

```
Potential bug                              BUG-104
Duplicate payment under concurrency
Confidence: 71 %                           status UNVERIFIED
detector: ai:agent  ·  anchor: mn.pay.PaymentService#createPayment

  plan      hypothesis: two concurrent createPayment calls with the same
            idempotency key both pass the exists() check and both insert
            trigger: Concurrent { threads: 8, iterations: 200 }
            expected: Invariant { count(payments where key=K) == 1 }
            isolation: Slice (@DataJpaTest + testcontainers postgres)
            repetitions: 3

  emit      .bughunter/generated-tests/BUG-104/
            BugHunter_BUG104_DuplicatePaymentTest.java

  run now   ./gradlew test --tests '*BugHunter_BUG104*'   → FAIL 3/3
            expected 1 payment, found 2

  run base  worktree a81f92c^                             → PASS 3/3

  judge     FAIL now + PASS before  →  reproduced, regression
            confidence 0.71 → 0.97 ; status UNVERIFIED → VERIFIED
            introduced_commit = a81f92c
```

The jump from 71 % to 97 % is not a model raising its own grade. It is: the predicted
failure occurred, it occurred every time, and it did not occur before the change.

---

## 9. Failure modes and how each is handled

| Failure | Handling |
|---|---|
| Generated test does not compile | `error`; compiler output attached; one automatic retry with the diagnostic fed back if an AI provider is active, then stop |
| Test times out | container killed, `inconclusive`, partial log kept |
| Build system not detected | verification refuses to start; `doctor` explains what is missing |
| Docker unavailable, policy `docker` | `permission_required`-style refusal with the exact `policy.toml` change needed |
| Baseline revision unavailable | baseline run skipped, outcome capped, stated in the report |
| Test passes but for the wrong reason | mitigated by `expected_failure` matching, not just exit code |
| Flaky project suite | only the generated test is run (`--tests` filter), never the whole suite |
| Bug in an unbuildable module | `error`, and the module recorded so it is not retried every scan |

Nothing here degrades to a shrug. Every path produces a recorded outcome with its evidence,
because a verification engine that sometimes silently gives up is worse than none — you
would stop being able to tell "not reproduced" from "never actually ran".
