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

In a multi-module fixture the package does not name the module, so the grader resolves the module
too: find the one existing `**/src/main/java/<package path>` directory and write the test to that
module's `src/test/java/<package path>/`. `mn.acme.common` lands in `libs/common`, `mn.shop.api` in
`api`. There is exactly one match in every fixture here.

## Where a TypeScript test goes, and why not under `src/`

`hidden.test.ts` is copied to **`web/hidden.test.ts`** — the directory holding `package.json`, the
one `fixture-build` runs `npm ci && npm run build && npm test` in. Not under `web/src/`, on
purpose: `tsconfig.json` has `include: ["src"]`, and `npm run build` is `tsc --noEmit`, so a hidden
test inside `src` would be type-checked as part of **L0**. A hidden test must never be able to fail
the build it is grading. Outside `src` it is invisible to `tsc` and still collected by vitest,
whose default include is `**/*.test.ts` from the package root.

Paths inside such a test are relative to `web/`, because that is vitest's working directory.

## A directory may hold more than one file

`B1` ships a `HiddenTest.java` and a `hidden.test.ts`. The grader copies **every** file in the
directory, routing by extension — `.java` by its package declaration, `.test.ts` to `web/`. It must
not assume one file per task.

One consequence of `fixture-build` being `set -e`: for `next-storefront` the Maven half runs first,
so a red Java hidden test short-circuits before `npm test` and the frontend result is never
printed. That is harmless for a gate that is an AND, but a grader wanting per-file detail has to
run the two halves itself rather than read one combined log.

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
- **B1** — any rename that leaves both `Order` and `OrderDto` publishing `grossAmount` and neither
  publishing `totalAmount`, where "publishes" is the union of record components, declared fields and
  no-argument accessors read by reflection: a record or a class, `getGrossAmount()` or a
  record-style `grossAmount()`, all pass. Any schema file under `classpath:graphql/` declaring the
  new field on `type Order`, reordered or re-commented or split across files. On the frontend, any
  `orders.ts` and `OrderSummary.tsx` naming `grossAmount` and not `totalAmount` — an `interface` or
  a `type` alias, member access or destructuring. `web/src/generated/graphql-generated.ts` is never
  read: nothing in this fixture regenerates it, so it still carries the old name after a *correct*
  fix, and grading it would fail every one of them.

  A leftover `getTotalAmount()` beside a renamed field is graded **red**. That is a deliberate
  reading of "throughout" — it is a public accessor still carrying the old name, and it is the exact
  half-rename this corpus plants as a bug at `c3`.
- **B2** — any end state in which the seam agrees, in either direction and under any name. Three
  correct fixes were written and all pass: finishing the rename forward on the schema and the
  frontend; reverting the Java side back to `totalAmount` and touching no frontend file at all; and
  keeping both sides as they are while adding an `@SchemaMapping(typeName = "Order", field =
  "totalAmount")` resolver that serves the orphaned field. Nothing here names a field, because the
  prompt names none — asserting `grossAmount` would grade which direction the agent guessed.
- **A2** — any `Money` that keeps a fourth decimal place and carries at least four: `setScale(4, …)`
  under any rounding mode, in the constructor or in the accessor, behind a constant or a parameter.
  The bound is "at least four", never "exactly four", for the same reason A1's is "at least 128".
  Deleting the rounding entirely does not pass: an amount would then carry whatever scale its caller
  supplied rather than the scale the task asks for.

## Where L0 does not reach, and what that decided

The plan gave `B1` a single `hidden.test.ts`. It ships two files instead, because the build covers
neither side of the rename:

- `mvn -o test` in `api/` is green at `c2` and green after a rename that touches only the frontend.
  `OrderDtoTest` builds the record positionally on purpose, so it survives any rename. The Java half
  of `B1` is graded by nothing unless a Java test grades it.
- `tsc --noEmit` keeps `OrderSummary.tsx` agreeing with the `Order` type in `orders.ts`, and that is
  all it keeps agreeing. The GraphQL query is a template literal.

Both negative controls were run and each is caught by exactly one half: a backend-and-schema-only
rename is red only in `hidden.test.ts`, a frontend-only rename red only in `HiddenTest.java`. One
file would have passed one of them.

`B2` went the other way and ships **one** file, on the Java side. The plan's draft for it read only
the schema, the query and the view — all three of which still agree at `c3` — so it was green at its
own start commit and graded nothing. The disagreement at `c3` is between the Java type and the
schema, and no test that cannot see `OrderDto` can observe it. So `B2` reads the published Java
contract by reflection and the two frontend files from the repository root, found by walking up
rather than assumed, because Maven runs it with `api/` as the working directory.

`A2` is a one-module gate and says so. `Money`'s behaviour is executable and is what the task asks
for; neither service mentions a scale at the start commit, and neither needs to change for a correct
fix. Its third test is a guard, vacuous at `c2`, against the shape where the shared type is widened
and a consumer rounds the precision straight back off. `required_sites` remains the separate check
that the agent looked at all three modules.

## The coupling that is left

`A1` calls `PaymentValidator.check(String, BigDecimal)` — the only executable gate on key length, so
the only way to observe the behaviour rather than read it. An agent that deletes or renames that
method would break the test's compilation rather than its assertion. The task is a width change and
has no reason to touch that signature, but it is the one place these tests are coupled to the
fixture's API, and it is where to look first if a run fails L0 with a compile error in `HiddenTest`.
