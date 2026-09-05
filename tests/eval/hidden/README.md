# Hidden tests

The primary grading gate (L1) for the Tier 2 benchmark. One directory per task, named by the
**short id** the fixture spec declares — `A1`, not `A1-idempotency-key-length`. The authority is
`hidden_tests` in `tests/fixtures/specs/*/fixture.toml`, which the grader reads via
`scripts/eval/task_lookup.py <task-id> hidden_tests`. Nothing derives the directory from the task id.

**They live here, not in the fixture.** The agent works in a generated repository under
`target/fixtures/`; these never enter it until grading, which happens after the agent is gone.
Contamination is prevented by construction rather than by discipline.

## The rule that matters

**Test the observable behaviour the task asks for, never the reference solution.** A hidden test
that asserts a particular method name, file layout or implementation strategy grades conformity, and
the benchmark then measures whether the agent guessed our design rather than whether it fixed the
problem. `required_sites` is the separate, honest check for completeness — an L3 result that
disagrees with L1 is a finding about the run, not a bug in the test.

If a test cannot be written without naming an implementation detail, the task's prompt is
underspecified — fix the prompt in the fixture spec, not the test.

Corollary, learned while writing these: an assertion is phrased as a *bound* ("nothing caps this
below 128", "some uniqueness constraint is still standing"), never as an equality against the value
the reference fix happens to use. Bounds accept fixes we did not think of; equalities do not.

## Package declarations, not paths

The corpus renames `mn.pay` to `mn.payments` at commit `c5`, so a task's package depends on the
commit it starts from — `A1` starts at `c2` and is `mn.pay`, `C1` starts at `c7` and is
`mn.payments`. Each file's `package` line is the single source of truth; the grader derives the
destination under `src/test/java/` from it. Getting the package wrong produces a compile error, and
a compile error is not a red test — it is a broken test that happens to be red.

## Two properties every test here must have

1. **It fails at its task's start commit**, with an assertion message about the missing behaviour —
   not a compile error and not a missing class. A test that passes before the agent has done
   anything grades nothing.
2. **It passes after a minimal correct fix** written by hand. Without this, "the test detects the
   bug" is indistinguishable from "the test always fails".

Check both, for every test added here, before it is allowed to gate a paid run:

```bash
COMMIT=$(python3 scripts/eval/task_lookup.py <task-id> | cut -d' ' -f2)
git -C target/fixtures/spring-payments checkout -q "$COMMIT"
# copy the test to src/test/java/<the package it declares>/ then:
docker run --rm --network=none -v "$PWD/target/fixtures/spring-payments:/w" -w /w nexus-bench:latest fixture-build
```

## What each test accepts on purpose

- **A1** — any migration set whose *last* declaration of the idempotency key column is 128 wide or
  wider, or unbounded; whether that is an edit to `V1__init.sql` or an appended `ALTER TABLE`
  migration. Any JPA mapping that does not cap the column below 128, including one that declares no
  length at all. Any validator that lets a 128-character key through, including one that never
  checks length. Only the schema assertion is red at `c2`; the other two are guards against a fix
  that widens one place and narrows another.
- **C1** — any migration set that leaves a uniqueness constraint standing on the idempotency key:
  `CREATE UNIQUE INDEX`, `ALTER TABLE ... ADD CONSTRAINT ... UNIQUE`, or an inline `UNIQUE` on the
  column, under any name, reached by deleting the offending `DROP`, deleting the migration that
  holds it, or appending a new one. A single statement may both drop and re-add — the idempotent
  `ALTER TABLE … DROP CONSTRAINT IF EXISTS x, ADD CONSTRAINT x UNIQUE (…)` idiom is accepted, and
  names are matched whole, so dropping `ux_foo_key` does not remove a live `ux_foo`.

## The coupling that is left

`A1` calls `PaymentValidator.check(String, BigDecimal)` — the only executable gate on key length, so
the only way to observe the behaviour rather than read it. An agent that deletes or renames that
method would break the test's compilation rather than its assertion. The task is a width change and
has no reason to touch that signature, but it is the one place these tests are coupled to the
fixture's API, and it is where to look first if a run fails L0 with a compile error in `HiddenTest`.
