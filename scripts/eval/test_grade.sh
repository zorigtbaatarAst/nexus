#!/usr/bin/env bash
# The grader's own test. It is the one component whose bugs are invisible in the results: a
# grader stuck at `passed: false` reads as a devastating result for every arm rather than as a
# bug, and 75 paid runs would be spent before anyone noticed nothing was ever graded.
#
# So every case here has a known answer, and each asserts something the others cannot:
#
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
#
# Runs six gradings, twelve containers, offline. Costs nothing but a couple of minutes.
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

echo "the grader fails an empty diff, passes a correct one, and says why in between"
