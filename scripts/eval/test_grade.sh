#!/usr/bin/env bash
# The grader's own test. It is the one component whose bugs are invisible in the results: a
# grader stuck at `passed: false` reads as a devastating result for every arm rather than as a
# bug, and 95 paid runs would be spent before anyone noticed nothing was ever graded.
#
# So every case here has a known answer, and each asserts something the others cannot:
#
#   0. plan        not a grading at all: the sweep's own cell plan, which nothing else in this
#                  repository executes. Free, and first for that reason.
#   1. empty       an empty diff must fail — and must fail on L1 *only*. L0 and L2 true proves
#                  the container, the mount and the build actually worked; a mechanical failure
#                  would zero them too and be indistinguishable from a bad agent.
#   2. a1-fix      a real correct fix must pass. L3 is partial on purpose: `passed` must not
#                  depend on it.
#   3. b1-fix      a correct cross-stack fix must pass, and both of B1's hidden files must be
#                  routed — the .java by its package, the .test.ts beside package.json.
#   4. b1-java     a rename that stops at the Java side must fail, and must be caught by the
#                  TypeScript half. Without this, case 3 would pass just as well if the
#                  .test.ts were copied somewhere vitest never looks.
#   5. a1-sig      a hidden test that cannot compile must be a distinct outcome from one that
#                  failed an assertion, and must say so in `adjudicate`.
#   6. c1-fix      the one required site that names a directory, reached by a patch that only
#                  adds a file. Neither shape appears in any other case.
#   7-9.           the three distinguishable outcomes of a red baseline: a project test that
#                  genuinely failed, main sources that would not compile, and a build that never
#                  ran. The third must not wear the costume of the first.
#   10-13.         A2, B2, E1 and N1 on an empty diff — the two tasks no case above reaches at
#                  all (the only Gradle toolchain, the only reflection-based hidden test), and
#                  the two that joined the sweep last and whose hidden tests have never been run
#                  through the grader.
#
# Runs thirteen gradings, twenty-six containers, offline. Costs nothing but a few minutes.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GRADE="$ROOT/scripts/eval/grade.sh"
LOOKUP="$ROOT/scripts/eval/task_lookup.py"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP" 2>/dev/null || echo "test_grade.sh: could not remove $TMP" >&2' EXIT

# Synthesise one run directory: a checkout of the task's start commit, edited by `$3`, and the
# diff of those edits. Building the diffs from the fixture rather than committing patch files
# keeps them from rotting the next time the corpus is regenerated.
synthesize() {  # task, case, edits (a shell snippet run inside the checkout)
  local task="$1" name="$2" edits="$3" repo commit tree
  read -r repo commit _ < <(python3 "$LOOKUP" "$task")
  tree="$TMP/$name.src"
  git clone -q "$ROOT/target/fixtures/$repo" "$tree"
  git -C "$tree" checkout -q "$commit"
  ( cd "$tree" && eval "$edits" )
  mkdir -p "$TMP/$name"
  # Staged, and against the start commit — the same two steps run.sh takes, so a file the edits
  # *added* is in the patch here exactly as it would be in a real run's.
  git -C "$tree" add -A
  git -C "$tree" diff --cached "$commit" > "$TMP/$name/diff.patch"
}

assert_grade() {  # case, expression...
  python3 - "$TMP/$1" "$@" <<'PY'
import json
import sys

grade = json.load(open(sys.argv[1] + "/grade.json"))
for expression in sys.argv[3:]:
    if not eval(expression, {"g": grade}):  # noqa: S307 - literals from this file only
        sys.exit(f"FAIL [{sys.argv[2]}] {expression}\n{json.dumps(grade, indent=2)}")
print(f"ok   {sys.argv[2]}")
PY
}

A1=A1-idempotency-key-length
B1=B1-rename-crosses-the-seam

# --- 0. the sweep's own cell plan ---------------------------------------------------------------

# Free, so it runs first — ahead of every container, and ahead of anything that could spend.
# Nothing else in this repository executes sweep.sh, and its per-task arm split is exactly the
# mistake this file exists to catch before money moves: a typo in either arm of `arms_for`
# silently hands an added task three arms, T4 quietly becomes a seven-task threshold instead of
# a five-task one, and analyse.py — which derives each comparison's task set from the run tree —
# reports an asymmetry that no longer exists. The error lands in the result, not in the harness.
# DRY_RUN=1 prints the plan and exits before the guards, the gate and any docker call.
PLAN="$(DRY_RUN=1 "$ROOT/scripts/eval/sweep.sh" | tail -n +2)"  # line 1 is the stamp line
PLAN_CELLS="$(printf '%s\n' "$PLAN" | wc -l)"
[ "$PLAN_CELLS" = 95 ] \
  || { echo "FAIL [plan] the sweep plans $PLAN_CELLS cells, not 95 (5x3x5 + 2x2x5)" >&2; exit 1; }
if printf '%s\n' "$PLAN" | grep -E '^(E1-untested-change|N1-null-task)/A0/'; then
  echo "FAIL [plan] a ranking-only task is planned at A0 (printed above); T4's task set would" >&2
  echo "silently become seven tasks instead of five and nothing in the output would say so." >&2
  exit 1
fi
echo "ok   plan 95 cells, neither ranking-only task at A0"

# --- 1. an empty diff must not pass ------------------------------------------------------------

mkdir -p "$TMP/empty"
: > "$TMP/empty/diff.patch"
"$GRADE" "$A1" "$TMP/empty" >/dev/null
assert_grade empty \
  'g["passed"] is False' \
  'g["L1_hidden"] is False' \
  'g["L0_build"] is True' \
  'g["L2_collateral"] is True' \
  'g["diff_empty"] is True' \
  'g["L3_sites_found"] == []' \
  'len(g["L3_sites_missed"]) == 3' \
  'g["hidden_tests_placed"] == ["src/test/java/mn/pay/HiddenTest.java"]' \
  'g["adjudicate"] == []'

# --- 2. a correct fix must pass ------------------------------------------------------------------

# The schema is the only place the key is actually constrained at this commit: the entity
# declares no length and the validator checks none, so widening `V1__init.sql` is a complete
# correct fix and neither of the other two sites has to change. L3 therefore reports one site
# found and two missed on a run that passes — which is the point. `passed` is L0 and L1 and L2.
synthesize "$A1" a1-fix \
  "sed -i 's/idempotency_key VARCHAR(64)/idempotency_key VARCHAR(128)/' src/main/resources/db/migration/V1__init.sql"
"$GRADE" "$A1" "$TMP/a1-fix" >/dev/null
assert_grade a1-fix \
  'g["passed"] is True' \
  'g["L0_build"] is True' \
  'g["L1_hidden"] is True' \
  'g["L2_collateral"] is True' \
  'g["diff_empty"] is False' \
  'g["diff_applied"] is True' \
  'g["L3_sites_found"] == ["src/main/resources/db/migration/V1__init.sql"]' \
  'len(g["L3_sites_missed"]) == 2' \
  'g["adjudicate"] == []'

# --- 3. a correct cross-stack fix must pass, and route both hidden files ---------------------------

RENAME='s/totalAmount/grossAmount/g; s/TotalAmount/GrossAmount/g'
# One line each: `synthesize` runs these through `eval`, where a newline ends the command.
JAVA_SIDE='api/src/main/java/mn/shop/api/Order.java api/src/main/java/mn/shop/api/OrderDto.java api/src/main/resources/graphql/order.graphqls'
WEB_SIDE='web/src/lib/orders.ts web/src/components/OrderSummary.tsx'

synthesize "$B1" b1-fix "sed -i '$RENAME' $JAVA_SIDE $WEB_SIDE"
"$GRADE" "$B1" "$TMP/b1-fix" >/dev/null
assert_grade b1-fix \
  'g["passed"] is True' \
  'g["L1_hidden"] is True' \
  'g["L3_sites_missed"] == []' \
  'g["hidden_tests_placed"] == ["api/src/test/java/mn/shop/api/HiddenTest.java", "web/hidden.test.ts"]' \
  'g["adjudicate"] == []'

# The .java went under the module that already owns `mn.shop.api`, and the .test.ts beside
# package.json rather than under web/src — where tsconfig's include: ["src"] would have made the
# grading artefact part of the build it grades.
grep -q 'hidden.test.ts.*(2 tests)' "$TMP/b1-fix/grade-hidden.log" \
  || { echo "FAIL [b1-fix] vitest never collected web/hidden.test.ts" >&2; exit 1; }
echo "ok   b1-fix vitest collected the hidden .test.ts"

# --- 4. the TypeScript half must be load-bearing ----------------------------------------------------

# A rename that stops at the seam. `mvn test` is green and `tsc --noEmit` is green, so L0 sees
# nothing; only the frontend hidden test can fail this, and if it fails then case 3's pass was
# not an accident of where the file landed.
synthesize "$B1" b1-java "sed -i '$RENAME' $JAVA_SIDE"
"$GRADE" "$B1" "$TMP/b1-java" >/dev/null
assert_grade b1-java \
  'g["passed"] is False' \
  'g["L1_hidden"] is False' \
  'g["L0_build"] is True' \
  'g["L2_collateral"] is True' \
  'g["adjudicate"] == []'

grep -q 'src/lib/orders.ts still names totalAmount' "$TMP/b1-java/grade-hidden.log" \
  || { echo "FAIL [b1-java] the frontend hidden test is not what failed" >&2; exit 1; }
echo "ok   b1-java caught only by the TypeScript half"

# --- 5. a hidden test that cannot compile is its own outcome ------------------------------------------

# A1's hidden test calls `new PaymentValidator().check(...)`, the only executable gate on key
# length. An agent that renames that method writes a fix that may well be correct and cannot be
# graded — that must reach a human, not be scored as a silent zero.
synthesize "$A1" a1-sig "
  sed -i 's/idempotency_key VARCHAR(64)/idempotency_key VARCHAR(128)/' src/main/resources/db/migration/V1__init.sql
  sed -i 's/public void check(/public void validate(/' src/main/java/mn/pay/PaymentValidator.java
  sed -i 's/validator.check(/validator.validate(/' src/main/java/mn/pay/PaymentService.java
"
"$GRADE" "$A1" "$TMP/a1-sig" >/dev/null
assert_grade a1-sig \
  'g["passed"] is False' \
  'g["L0_build"] is True' \
  'g["L2_collateral"] is True' \
  'g["L1_hidden"] is False' \
  '"hidden-test-compile-error" in g["adjudicate"]'

# --- 6. a required site that names a directory, and a diff that only adds files -----------------------

# C1 is the only task whose `required_sites` is a directory rather than a file, and appending a
# migration is the commonest correct fix for it — a patch with no `--- a/` line at all. Both the
# prefix match and the added-file half of the header parse are exercised nowhere else.
synthesize C1-regression-recognised c1-fix \
  "printf '%s\n' 'CREATE UNIQUE INDEX ux_payment_idem ON payment (idempotency_key);' > src/main/resources/db/migration/V4__restore_payment_unique_index.sql"
"$GRADE" C1-regression-recognised "$TMP/c1-fix" >/dev/null
assert_grade c1-fix \
  'g["passed"] is True' \
  'g["L1_hidden"] is True' \
  'g["L3_sites_found"] == ["src/main/resources/db/migration"]' \
  'g["L3_sites_missed"] == []' \
  'g["hidden_tests_placed"] == ["src/test/java/mn/payments/HiddenTest.java"]'

# --- 7-9. three distinguishable outcomes for a red baseline -------------------------------------------
#
# `fixture-build` compiles and tests in one command, so a red baseline has to be attributed. The
# dangerous attribution is the one made from an *absence*: concluding "a test failed" because no
# compiler banner was found records a grading run that fell over as an honest grade, and claims
# `L0_build: true` for a build that never ran. These three cases are the whole point of the
# distinction, and until they existed nothing here had ever produced `L0_build: false`.

# 7. A collateral test genuinely broken, with the task itself correctly fixed. This is the case
#    that must NOT regress into a flag: the project compiled, a test ran, a test failed.
synthesize "$A1" l2-red "
  sed -i 's/idempotency_key VARCHAR(64)/idempotency_key VARCHAR(128)/' src/main/resources/db/migration/V1__init.sql
  sed -i 's/setScale(2, RoundingMode.HALF_UP)/setScale(3, RoundingMode.HALF_UP)/' src/main/java/mn/pay/PaymentService.java
"
"$GRADE" "$A1" "$TMP/l2-red" >/dev/null
assert_grade l2-red \
  'g["L0_build"] is True' \
  'g["L2_collateral"] is False' \
  'g["passed"] is False' \
  'g["adjudicate"] == ["l1-not-isolated"]'

# 8. Main sources that do not compile: `check` renamed with its production caller left behind.
synthesize "$A1" l0-red \
  "sed -i 's/public void check(/public void validate(/' src/main/java/mn/pay/PaymentValidator.java"
"$GRADE" "$A1" "$TMP/l0-red" >/dev/null
assert_grade l0-red \
  'g["L0_build"] is False' \
  'g["L2_collateral"] is False' \
  'g["passed"] is False' \
  '"collateral-unknown-build-failed" in g["adjudicate"]' \
  '"baseline-failure-unrecognised" not in g["adjudicate"]'

# 9. A build that never ran at all — `fixture-build` finds no build file and exits 1 with no
#    compiler banner and no test tally. Before the positive-signal rule this was recorded as
#    `L0_build: true, L2_collateral: false`, byte-identical to case 7. The same branch now
#    catches an unmounted /work, a container the OOM killer takes, and the 600s timeout.
synthesize "$A1" unrecognised "rm pom.xml"
"$GRADE" "$A1" "$TMP/unrecognised" >/dev/null
assert_grade unrecognised \
  'g["L0_build"] is False' \
  'g["L2_collateral"] is False' \
  'g["passed"] is False' \
  '"baseline-failure-unrecognised" in g["adjudicate"]' \
  '"graded-failure-unrecognised" in g["adjudicate"]'

# --- 10-13. the four tasks the cases above never touch -------------------------------------------------

# A2 is the only Gradle toolchain and the only multi-module placement; B2 is the only hidden test
# that reads its subject by reflection. A placement or collection regression in either yields
# `passed: true` on an empty diff — a false green on 30 of the 95 runs, and nothing would look
# wrong. All four are empty-diff cases: red on L1 alone, hidden test in the right place.
#
# E1 and N1 route exactly like case 1 — `mn.pay` in single-module spring-payments — so they add
# no placement coverage, and while they were outside the sweep that made them not worth a case.
# They are in the sweep now, at ten paid runs each, and what these two cases actually assert is
# not routing but that each task's hidden test *compiles and fails red at its own start commit*.
# Neither file had ever been compiled by the grader — they arrived with the tasks — and both say
# in their own javadoc that they are written to compile where the fix does not exist yet and fail
# on a named assertion instead, which is not a claim reading can check. Were it wrong, every one
# of that task's ten runs would come back `hidden-test-compile-error`: unscoreable, and
# discovered after paying. `adjudicate == []` below is the assertion that costs nothing and
# buys that.
for case in "A2-shared-type-change:libs/common/src/test/java/mn/acme/common/HiddenTest.java" \
            "B2-orphaned-field-diagnosis:api/src/test/java/mn/shop/api/HiddenTest.java" \
            "E1-untested-change:src/test/java/mn/pay/HiddenTest.java" \
            "N1-null-task:src/test/java/mn/pay/HiddenTest.java"; do
  task="${case%%:*}"
  mkdir -p "$TMP/$task"
  : > "$TMP/$task/diff.patch"
  "$GRADE" "$task" "$TMP/$task" >/dev/null
  assert_grade "$task" \
    'g["passed"] is False' \
    'g["L1_hidden"] is False' \
    'g["L0_build"] is True' \
    'g["L2_collateral"] is True' \
    "g[\"hidden_tests_placed\"] == [\"${case#*:}\"]" \
    'g["adjudicate"] == []'
done

echo "the grader fails an empty diff, passes a correct one, and says why in between"
