# Verification

> The agent's statement "done" is a claim about the world. Verification is the only thing that
> makes it a fact.

New crate: `nexus-verify`. Today [`verification-engine.md`](../verification-engine.md) is 271
lines of design and zero lines of code, while `AGENTS.md` refers to the crate as though it
exists.

---

## 1. Scope: gate first, reproduce later

The existing design goes straight to the hard thing — generating a reproduction test, running it
against HEAD *and* against the baseline revision in a detached worktree, and judging the matrix.
That is the right long-term target and it is Phase 3.

**The MVP is the gate**, because it delivers most of the value for a fraction of the work and
requires no test generation, no sandbox and no worktree:

```
agent claims done
  → what changed          (rescan — already exists, already fast)
  → does it compile       (build command from the profile)
  → do the tests pass     (test command from the profile)
  → does it lint          (lint command from the profile)
  → what did it touch     (git diff — does it match what was claimed?)
  → what does it reach    (impact — was anything affected left untouched?)
  → verdict
```

Steps 1, 5 and 6 already exist. Steps 2–4 are process execution under the existing allowlist
discipline. `detect.rs` already identifies the build system, which is where the commands come
from.

## 2. The verdict

```rust
pub enum Verdict {
    Verified { checks: Vec<Check> },
    Failed { check: CheckKind, detail: String, evidence: Vec<CodeRef> },
    Inconclusive { why: String, checks_run: Vec<Check> },
}
```

**`Inconclusive` is the load-bearing variant.** An infrastructure failure — no build tool on
PATH, a network-dependent test, a suite that was already red before the change — yields
`Inconclusive`, never `Failed`. A test that would not compile says nothing about the hypothesis.

Collapsing `Inconclusive` into `Failed` is precisely how a gate earns a reputation for crying
wolf and gets switched off, after which it verifies nothing at all. This is the same rule
already applied to confidence: an infrastructure failure leaves confidence unchanged, never
lowered.

## 3. Baseline comparison

A test suite that was already failing before the change proves nothing about the change. Where
the tree is clean and the baseline commit is reachable, the suite runs at the baseline revision
too, and the judgement is over the **pair**:

| Baseline | HEAD | Verdict |
|---|---|---|
| pass | pass | `Verified` |
| pass | fail | `Failed` — the change did it |
| fail | fail | `Inconclusive` — already broken |
| fail | pass | `Verified`, with a note that the change *fixed* a pre-existing failure |

Halving this to save time destroys the ability to distinguish "this change introduced a bug"
from "this suite was already red" — which is the entire value of running it.

Where the tree is dirty or the baseline is unreachable (force-push, rebase, shallow clone —
`nexus-vcs` already detects all three), the baseline run is skipped and the verdict says so.

## 4. Execution safety

Inherited from [`security.md`](../security.md) §3–4, unchanged and non-negotiable:

- **Commands are argv, never strings.** Allowlist entries are templates with typed holes;
  `{test}` becomes exactly one argv element. **`sh -c` is never used, anywhere.**
- The allowlist lives in `.nexus/policy.toml`. It is the entire execution surface.
- `policy.execute = "none"` yields `permission_required` over MCP — a result, never an
  execution.
- Timeouts on every process; output captured and bounded.
- Writes only through `SafeWriter`, rooted at `.nexus/generated-tests/`, canonicalising the
  parent path **before** the prefix check. A jail comparing unresolved paths is not a jail.
  (Phase 3, when generation lands. No writes at all in the MVP.)

## 5. Persistence — reviving three dead tables

| Table | Currently | Written by |
|---|---|---|
| `test_runs` | dead | every verification run: command, exit code, duration, pass/fail counts, commit |
| `finding_verifications` | dead | a verification attempt against a specific finding |
| `test_coverage` | dead | parsed from test-run output where the runner emits it |

Append-only, like every other ledger.

`test_coverage` matters beyond verification: it replaces `impact::is_test`, a **path-name string
match** (`/test/`, `.test.ts`, `Test` suffix) that is currently the sole basis for Review's
flagship "nothing tests this change" finding. Real coverage from a real run turns that finding
from a heuristic into evidence.

## 6. Feeding the loop back

A verification result is not a terminal event:

- `Failed` on a finding → status `VERIFIED` (the bug is real; it was reproduced).
- A previously `FIXED` finding failing again → `REGRESSED`, with both histories attached.
- `Verified` where a finding predicted breakage → evidence the detector is noisy; recorded, and
  visible when tuning it.
- Every run appends to `test_runs`, which is what makes "this suite has been flaky for eleven
  scans" answerable — a question no single run can answer and every developer eventually asks.

This is the learning loop of [`workflows.md`](07-agent-integration.md) §5, and it is entirely deterministic:
ledger rows and status transitions, no model anywhere.

## 7. What verification will not do

- **Not fix anything.** It reports; the agent or the developer fixes.
- **Not generate tests in the MVP.** Deterministic templates per `(finding_type, framework)`
  are Phase 3, and they arrive with the SafeWriter jail, not before.
- **Not run in Docker in the MVP.** The sandbox profile in `security.md` §4 is Phase 3, host
  opt-in.
- **Not run on every keystroke.** The `Stop` hook, or explicitly. A verifier that runs
  constantly is a build server, and nobody asked for one of those.
