#!/usr/bin/env bash
# The grader's own test. It is the one component whose bugs are invisible in the results: a
# grader stuck at `passed: false` reads as a devastating result for every arm rather than as a
# bug, and 95 paid runs would be spent before anyone noticed nothing was ever graded.
#
# So every case here has a known answer, and each asserts something the others cannot:
#
#   0. plan        not a grading at all: the sweep's own cell plan, which nothing else in this
#                  repository executes. Free, and first for that reason.
#   0c. image-artifacts  also not a grading: that every file the image bakes from the tree —
#                  nexus, nexus-hook, fixture-build — is still the tree's, by content. A stale
#                  image cost 95 paid runs once, and a stale fixture-build would cost grades.
#   0d. copy-table also not a grading: that the check's artifact table still covers every COPY
#                  in the Dockerfile. 0c proves the check refuses what it compares; this proves
#                  it compares everything.
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
#
# The expected count is computed from TASKS/ARMS/REPS as this process sees them — the same
# variables sweep.sh itself defaults from — rather than a hardcoded 95, so a validation run
# (Task 4 of the tier2-instrument-repair plan: ARMS=A0, checking the corpus before A1 exists to
# compare against) is checked on its own terms instead of being refused by a gate that only knows
# the full sweep. This is what lets `sweep.sh` call this file as its own pre-flight gate no
# matter what TASKS/ARMS/REPS the operator passed it — those env vars reach this subprocess the
# same way they reach sweep.sh's own defaults. Which tasks are ranking-only is duplicated here
# rather than asked of arms_for() itself: asking sweep.sh the same question this check exists to
# answer would let the exact arms_for() typo this file was written to catch slip straight through.
RANKING_ONLY_TASKS="E1-untested-change N1-null-task"
FULL_ARM_SET="A0 A1 A5"
RANKING_ARM_SET="A1 A5"
DEFAULT_TASKS="A1-idempotency-key-length A2-shared-type-change B1-rename-crosses-the-seam B2-orphaned-field-diagnosis C1-regression-recognised E1-untested-change N1-null-task"

expected_cell_count() {  # tasks (space separated), arms restriction ("" = none), reps
  local tasks="$1" arms="$2" reps="$3" total=0 task base_arms n arm
  for task in $tasks; do
    case " $RANKING_ONLY_TASKS " in
      *" $task "*) base_arms="$RANKING_ARM_SET" ;;
      *) base_arms="$FULL_ARM_SET" ;;
    esac
    n=0
    for arm in $base_arms; do
      if [ -z "$arms" ]; then
        n=$((n + 1))
      else
        case " $arms " in
          *" $arm "*) n=$((n + 1)) ;;
        esac
      fi
    done
    total=$((total + n * reps))
  done
  echo "$total"
}

REQ_TASKS="${TASKS:-$DEFAULT_TASKS}"
REQ_REPS="${REPS:-5}"
EXPECTED_CELLS="$(expected_cell_count "$REQ_TASKS" "${ARMS:-}" "$REQ_REPS")"

PLAN="$(DRY_RUN=1 "$ROOT/scripts/eval/sweep.sh" | tail -n +2)"  # line 1 is the stamp line
PLAN_CELLS="$(printf '%s\n' "$PLAN" | wc -l)"
[ "$PLAN_CELLS" = "$EXPECTED_CELLS" ] \
  || {
    echo "FAIL [plan] the sweep plans $PLAN_CELLS cells, not $EXPECTED_CELLS for" \
      "TASKS=${TASKS:-<default>} ARMS=${ARMS:-<none>} REPS=${REPS:-<default>}" >&2
    exit 1
  }
if printf '%s\n' "$PLAN" | grep -E '^(E1-untested-change|N1-null-task)/A0/'; then
  echo "FAIL [plan] a ranking-only task is planned at A0 (printed above); T4's task set would" >&2
  echo "silently become seven tasks instead of five and nothing in the output would say so." >&2
  exit 1
fi
echo "ok   plan $PLAN_CELLS cells (matches TASKS/ARMS/REPS as requested), neither ranking-only" \
  "task at A0"

# --- 0b. ARMS restricts per task, and only ever narrows arms_for() ------------------------------

# The A0-only validation run Task 4 exists for: the five three-arm tasks contribute one arm each,
# the two ranking-only tasks — which arms_for() never gives A0 in the first place — contribute
# none. 5 tasks x 1 arm x 5 reps = 25.
PLAN_A0="$(ARMS=A0 DRY_RUN=1 "$ROOT/scripts/eval/sweep.sh" | tail -n +2)"
PLAN_A0_CELLS="$(printf '%s\n' "$PLAN_A0" | wc -l)"
[ "$PLAN_A0_CELLS" = 25 ] \
  || { echo "FAIL [arms=A0] ARMS=A0 plans $PLAN_A0_CELLS cells, not 25 (5 tasks x 1 arm x 5 reps)" >&2; exit 1; }
if printf '%s\n' "$PLAN_A0" | grep -E '^(E1-untested-change|N1-null-task)/'; then
  echo "FAIL [arms=A0] a ranking-only task has cells under ARMS=A0 (printed above)" >&2
  exit 1
fi
echo "ok   ARMS=A0 plans 25 cells over the five three-arm tasks, zero on the ranking-only tasks"

# An ARMS value naming an arm arms_for() never returns for any task must be refused outright, not
# silently intersected down to nothing — a plan that silently narrows to zero cells for every task
# still exits 0 and reads as "ran, found nothing" rather than "you mistyped an arm."
if ARMS="A0 A9" DRY_RUN=1 "$ROOT/scripts/eval/sweep.sh" >/dev/null 2>&1; then
  echo "FAIL [arms=widen] ARMS='A0 A9' should be refused: A9 is not an arm arms_for() ever returns" >&2
  exit 1
fi
echo "ok   ARMS naming an arm arms_for() never returns is refused, not silently honoured"

# --- 0c. everything the image bakes from the tree must be the tree's copy, by content ----------

# The $37.62 case. Both binaries answered `nexus 0.3.0`; one of them was a day old. So the
# fixtures here are the image's own artifacts, and copies of them differing by one byte: they
# report an identical --version (asserted, because that is the point) and differ by content.
# Any check that compares version strings, timestamps, image ids or paths passes both, and the
# refusal cases below fail. That is what makes this unable to be satisfied by the weakened
# comparison it exists to forbid.
#
# `nexus` is not the only artifact and not the most dangerous one: `fixture-build` is the L0
# build-and-grade path, so drift there changes grades rather than context. It gets its own
# refusal case for that reason — and because the review that widened this check found the real
# `build.sh` already drifted from the real image on the machine it ran on.
CHECK_ARTIFACTS="$ROOT/scripts/eval/check_image_artifacts.sh"
IMAGE="${IMAGE:-nexus-bench:latest}"

# A tree root built out of the image itself, so "matches" is constructed rather than assumed —
# the real tree may legitimately differ from the image at any moment, which is the whole point.
mkdir -p "$TMP/tree/target/release" "$TMP/tree/scripts/eval"
docker run --rm --entrypoint cat "$IMAGE" /usr/local/bin/nexus > "$TMP/tree/target/release/nexus"
docker run --rm --entrypoint cat "$IMAGE" /usr/local/bin/nexus-hook > "$TMP/tree/scripts/eval/nexus-hook.sh"
docker run --rm --entrypoint cat "$IMAGE" /usr/local/bin/fixture-build > "$TMP/tree/scripts/eval/build.sh"
chmod +x "$TMP/tree/target/release/nexus"

cp -r "$TMP/tree" "$TMP/tree-bad-nexus"
printf '\0' >> "$TMP/tree-bad-nexus/target/release/nexus"   # still a valid ELF: trailing bytes are ignored
cp -r "$TMP/tree" "$TMP/tree-bad-build"
echo "# a comment that changes nothing about what this script does" >> "$TMP/tree-bad-build/scripts/eval/build.sh"

# Run both --version probes inside the image, not on the host: these are the image's own
# Debian-linked binaries and the host is whatever the operator runs. Executing them here worked
# but only by glibc luck, and a test that breaks on an unrelated host upgrade gets deleted.
VERSIONS="$(docker run --rm -v "$TMP:/t:Z" --entrypoint sh "$IMAGE" -c \
  '/t/tree/target/release/nexus --version && /t/tree-bad-nexus/target/release/nexus --version')"
[ "$(printf '%s\n' "$VERSIONS" | sort -u | wc -l)" = 1 ] \
  || { echo "FAIL [image-artifacts] the two nexus fixtures disagree on --version:" >&2
       printf '%s\n' "$VERSIONS" >&2
       echo "so the mismatch case below no longer proves a version string cannot detect it." >&2
       echo "Fix the fixtures, not the assertion." >&2
       exit 1; }

STAMPED="$("$CHECK_ARTIFACTS" "$IMAGE" "$TMP/tree")" \
  || { echo "FAIL [image-artifacts] the check refused a tree byte-identical to the image's" >&2; exit 1; }
# What it prints is what meta.json stamps as the sweep's provenance, so it is asserted, not
# assumed: one line per artifact, each carrying that file's real sha256.
[ "$(printf '%s\n' "$STAMPED" | wc -l)" = 3 ] \
  || { echo "FAIL [image-artifacts] the check stamped $(printf '%s\n' "$STAMPED" | wc -l) artifacts, not 3:" >&2
       printf '%s\n' "$STAMPED" >&2; exit 1; }
while read -r image_path sha; do
  case "$image_path" in
    /usr/local/bin/nexus)         tree_file="$TMP/tree/target/release/nexus" ;;
    /usr/local/bin/nexus-hook)    tree_file="$TMP/tree/scripts/eval/nexus-hook.sh" ;;
    /usr/local/bin/fixture-build) tree_file="$TMP/tree/scripts/eval/build.sh" ;;
    *) echo "FAIL [image-artifacts] unknown artifact stamped: $image_path" >&2; exit 1 ;;
  esac
  [ "$sha" = "$(sha256sum "$tree_file" | cut -d' ' -f1)" ] \
    || { echo "FAIL [image-artifacts] stamped '$sha' for $image_path, which is not its sha256" >&2; exit 1; }
done <<<"$STAMPED"

# Two refusals, because covering the binary and not the grader is the same failure with a
# different file name — and the second one cannot be dismissed as "it is about the binary".
for case in "tree-bad-nexus:target/release/nexus:one byte appended, identical --version" \
            "tree-bad-build:scripts/eval/build.sh:one comment line appended"; do
  bad_tree="$TMP/${case%%:*}"; rest="${case#*:}"; bad_file="${rest%%:*}"; how="${rest#*:}"
  if REFUSAL="$("$CHECK_ARTIFACTS" "$IMAGE" "$bad_tree" 2>&1)"; then
    echo "FAIL [image-artifacts] the check PASSED a tree whose $bad_file differs from the" >&2
    echo "image's ($how). This is the defect that cost 95 paid runs." >&2
    exit 1
  fi
  # Naming the file is the difference between a rebuild and a hunt.
  grep -q "$bad_file" <<<"$REFUSAL" \
    || { echo "FAIL [image-artifacts] the refusal for $bad_file does not name it:" >&2
         echo "$REFUSAL" >&2; exit 1; }
  # A refusal nobody can act on is how the operator ends up reaching for SKIP_GATE.
  grep -q 'make bench-image' <<<"$REFUSAL" \
    || { echo "FAIL [image-artifacts] the refusal does not name the fix:" >&2; echo "$REFUSAL" >&2; exit 1; }
done
echo "ok   image-artifacts stamps all three, and refuses both a one-byte-different nexus of the" \
  "same --version and a one-line-different build.sh, naming the file and the rebuild"

# --- 0d. the check's artifact table must cover every tree file the Dockerfile bakes in ----------

# Free, no container. 0c proves the check refuses what it compares; this proves it compares
# everything. The table in check_image_artifacts.sh is coupled to scripts/eval/Dockerfile by
# hand, and a COPY added there but not here narrows the guard silently — which is this branch's
# own defect, one level up: the first version of the check covered `nexus` alone and shipped
# while `build.sh` was already drifted.
DOCKERFILE="$ROOT/scripts/eval/Dockerfile"

# Sources are read as the whitespace-separated fields between `COPY` and the destination, so any
# form that puts something else there must be refused rather than guessed at: `--from=`/`--chown=`
# flags, the JSON-array form, a heredoc, and a quoted or spaced path — which whitespace-splitting
# would tear into two garbled fields, failing for the wrong reason with an unreadable name.
# Refused up front so the limitation is enforced rather than discovered. If the Dockerfile ever
# goes multi-stage, this case and check_image_artifacts.sh's table need revisiting together.
if grep -nE -e '^COPY[[:space:]]+(--|\[|<)' -e "^COPY[[:space:]].*[\"']" "$DOCKERFILE"; then
  echo "FAIL [copy-table] the Dockerfile uses a COPY form this case cannot parse (printed" >&2
  echo "above): a flag, the JSON-array form, a heredoc, or a quoted path. Revisit this case" >&2
  echo "and check_image_artifacts.sh's artifact table together." >&2
  exit 1
fi

# The table rows are the only lines in that file starting with an image path, so this reads the
# table itself rather than anywhere the path happens to be mentioned — a path named only in a
# comment must not count as covered.
COVERED="$(grep -E '^/usr/local/bin/' "$ROOT/scripts/eval/check_image_artifacts.sh" | awk '{print $2}')"
UNTRACKED=""
while read -r src; do
  # target/fixtures is excluded deliberately, and must stay excluded: the Dockerfile COPYs the
  # corpus to /warm and /verify and `rm -rf`s both inside the same RUN layers, so the final image
  # contains no copy of it — verified, `ls /warm /verify` exits 2 — and a run clones each fixture
  # from the host at run time instead. Adding it to the table would not be a stricter check, it
  # would be a comparison against a file that does not exist. See check_image_artifacts.sh's
  # header for the whole reason, including what image-side staleness that leaves uncovered.
  if [ "$src" = "target/fixtures" ]; then
    continue
  fi
  grep -qxF "$src" <<<"$COVERED" || UNTRACKED="$UNTRACKED $src"
  # EVERY source, not only field 2: `COPY a b /dest/` is legal and bakes in both. Reading the
  # first alone would leave `b` uncompared with this case green — the silent narrowing this case
  # exists to prevent, reproduced inside it. (A `COPY` with fewer than three fields is not a
  # valid instruction and fails `docker build`, so the empty loop it produces here needs no case.)
done < <(awk '/^COPY[[:space:]]/ { for (i = 2; i < NF; i++) print $i }' "$DOCKERFILE")

[ -z "$UNTRACKED" ] \
  || { echo "FAIL [copy-table] the Dockerfile bakes in files that check_image_artifacts.sh does" >&2
       echo "not compare:$UNTRACKED" >&2
       echo "A sweep would stamp clean provenance for an artifact nothing checked. Add each to" >&2
       echo "the ARTIFACTS table with the image path it is COPYed to." >&2
       exit 1; }
echo "ok   copy-table every tree file the Dockerfile bakes in is in the check's artifact table"

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
