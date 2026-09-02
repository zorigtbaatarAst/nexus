# ADR-025 — Verification ships as a gate before it ships as a reproducer

**Status:** Accepted (2026-09-02)
**Amends:** the scope and sequencing of [`verification-engine.md`](../../verification-engine.md),
not its design.

## Why it is needed

`verification-engine.md` is 271 lines specifying reproduction planning, deterministic test
templates per `(bug_type, framework)`, a `SafeWriter` jail, a Docker sandbox, a baseline-revision
run in a detached worktree, and a full judgement matrix. `AGENTS.md` refers to the `nexus-verify`
crate as though it exists.

**No such crate exists. Not one line of it is built.**

Meanwhile the brief's requirement is simpler and more urgent: *an AI agent saying "done" is not
proof that the work is correct.* Most of that requirement is satisfied without generating a
single test.

## Decision

**Ship the gate first. Defer the reproducer.**

Phase 4 (`nexus-verify` v1) does only this:

```
"done" → changed set (rescan) → compile → test → lint → diff → impact → Verdict
```

Reproduction-test generation, the `SafeWriter` jail and the Docker sandbox move to Phase 5, and
they arrive **together** — the jail is the precondition for writing files into a project, never a
follow-up.

Two rules bind from v1:

- **`Verdict::Inconclusive { why }` is a first-class outcome.** An infrastructure failure — no
  toolchain, no network, a suite already red at baseline — is never reported as `Failed`.
- **The baseline-revision run stays in v1**, even though it is the most expensive part.

## Alternatives considered

**Build the full engine as designed.** It is the more valuable end state. Rejected as a *first*
increment: it front-loads the two highest-risk components (writing generated files into a
project, and sandboxed execution) before the cheap 80 % has proven anyone wants the gate at all.
If the gate turns out to be ignored, the reproducer was built for nothing.

**Skip the baseline run to halve execution time.** Tempting, and wrong. Without it, a suite that
was already failing is indistinguishable from a suite the change broke — which is the entire
question being asked. Halving the cost by removing the answer is not an optimisation.

**Collapse `Inconclusive` into `Failed`.** Simpler enum, simpler exit codes. Rejected: it is the
single decision that determines whether the gate survives contact with a real project. A gate
that reports failure when the toolchain is missing cries wolf, and a gate that cries wolf is
disabled — after which it verifies nothing.

**Reuse the existing `test_runs` schema without a crate.** Rejected on a boundary: verification
executes processes, which is a genuinely different risk surface from querying an index, and
mixing it into `nexus-core` would put process spawning inside the crate that must stay
deterministic and dependency-light.

## Costs

- Findings stay `UNVERIFIED` longer. Nothing is proven by reproduction until Phase 5, and every
  surface must keep saying so rather than letting anyone infer otherwise.
- `verification-engine.md` and `AGENTS.md` overstate what exists until Phase 4 lands. Both need a
  status line, and that is a documentation fix owed now, not later.
- Two shipping increments instead of one, with the integration work paid twice at the seam
  between them.

## The signal that should make you change it

1. **The gate is enabled and its verdicts are acted on.** That is the demand signal for the
   reproducer, and it justifies Phase 5.6 immediately.
2. **The gate is enabled and ignored.** Then the reproducer would not have helped either, and the
   right investigation is *why* — noise, latency, or verdicts that are not actionable.
3. **`Inconclusive` dominates real runs.** The profile-derived commands are wrong, and command
   detection needs work before any further verification investment.
