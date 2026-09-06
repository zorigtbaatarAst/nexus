#!/usr/bin/env bash
# Grade one benchmark run, from its diff alone, in a container that never saw the agent.
#
#   grade.sh <task-id> <run-directory>       # the directory holding diff.patch
#
# writes <run-directory>/grade.json, beside the two build logs the verdict was read from.
#
# No model decides pass or fail. A stochastic grader turns every regression investigation into
# an argument about the grader, and a model grading a model-context system has an obvious
# conflict of interest. Every judgement below is an exit code, a file that exists, or a
# substring.
#
# The input is `diff.patch` and nothing else. The grader is not told which arm produced it and
# has no way to find out, which is what keeps A0, A1 and A5 graded by the same yardstick.
#
# Two container runs over two trees that differ *only* by the hidden tests:
#
#   baseline   start commit + the agent's diff                     -> L0, L2
#   graded     start commit + the agent's diff + the hidden tests   -> L1
#
# `fixture-build` compiles and tests in one command, so the baseline run answers "does this
# still build" and "do the project's own tests still pass" together. Which of the two failed is
# attributed by looking for a compiler's own failure banner in the log — a heuristic, and
# deliberately one that cannot change the verdict: `passed` is the AND of all three, so a
# mis-attribution moves a `false` between two fields that are both required anyway.
#
# The graded tree is a *copy* of the baseline tree, so the hidden tests are the only difference
# between them. That is what makes the exit-code delta attributable to the hidden tests, and it
# is also what makes a compile error inside a hidden test recognisable: the baseline compiled.
set -euo pipefail

TASK="${1:?task id}"
OUT="${2:?run directory, containing diff.patch}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${IMAGE:-nexus-bench:latest}"
LOOKUP="$ROOT/scripts/eval/task_lookup.py"

[ -f "$OUT/diff.patch" ] || { echo "no $OUT/diff.patch to grade" >&2; exit 1; }

# A task that starts from a dirty working tree would be graded against a different start state
# than the one this script builds, and a multi-turn task has no single prompt at all. run.sh
# refuses both; a grader that accepted them would quietly grade the wrong thing.
if python3 "$LOOKUP" "$TASK" start_state >/dev/null 2>&1; then
  echo "$TASK declares start_state; grade.sh does not apply the working-tree patch yet" >&2
  exit 1
fi
read -r REPO COMMIT _ < <(python3 "$LOOKUP" "$TASK")

FIXTURE="$ROOT/target/fixtures/$REPO"
[ -d "$FIXTURE" ] || { echo "run make fixtures first" >&2; exit 1; }

# A missing image would otherwise surface as `docker run` exit 125 on both builds, and a run
# that was never graded at all would be recorded as one that failed every gate.
docker image inspect "$IMAGE" >/dev/null 2>&1 || {
  echo "no image $IMAGE: build it with scripts/eval/Dockerfile first" >&2; exit 1; }

WORK="$(mktemp -d)"
# The container writes target/, build/ and node_modules/ as root and chowns them back before it
# exits. If docker never started there is nothing to chown and rm fails — say so rather than
# leaving a silent orphan under /tmp.
cleanup() { rm -rf "$WORK" 2>/dev/null || echo "grade.sh: could not remove $WORK" >&2; }
trap cleanup EXIT

# --- the tree the agent left -----------------------------------------------------------------

BASELINE="$WORK/baseline"
git clone -q "$FIXTURE" "$BASELINE"
git -C "$BASELINE" checkout -q "$COMMIT"

# `git apply` refuses an empty patch outright, and an empty patch is a common real result — a
# run that timed out, or one that decided there was nothing to do. It is graded rather than
# skipped: the hidden tests are red at every start commit, so an untouched tree fails L1.
DIFF_EMPTY=true
DIFF_APPLIED=true
if grep -q '^diff --git ' "$OUT/diff.patch"; then
  DIFF_EMPTY=false
  git -C "$BASELINE" apply --whitespace=nowarn "$OUT/diff.patch" >"$OUT/grade-apply.log" 2>&1 \
    || DIFF_APPLIED=false
fi

GRADED="$WORK/graded"
cp -a "$BASELINE" "$GRADED"

# --- the hidden tests, which enter only now, after the agent is long gone ----------------------

# The directory comes from the manifest, never from the task id: the manifest says
# `tests/eval/hidden/A1`, while the task id is `A1-idempotency-key-length`.
mapfile -t HIDDEN < <(python3 "$LOOKUP" "$TASK" hidden_tests)
[ "${#HIDDEN[@]}" -gt 0 ] || { echo "$TASK declares no hidden_tests" >&2; exit 1; }

PLACED="$(python3 - "$GRADED" "${HIDDEN[@]/#/$ROOT/}" <<'PY'
"""Copy every file of a task's hidden-test directories into the graded tree.

Routing is by the file's own content, not by the task id or a table of fixture names:

  *.java     -> the `package` line it declares, under the module that already owns that
                package. `mn.pay` and `mn.payments` are the same task family at different
                commits, and `mn.acme.common` lives in libs/common while `mn.shop.api` lives
                in api/ — so both the package path and the module are resolved from the tree.
  *.test.ts  -> beside package.json, NOT under src/. tsconfig.json has include: ["src"] and
                the web build is `tsc --noEmit`, so a hidden test inside src/ would be
                type-checked as part of L0 — the grading artefact failing the build it grades.
                vitest still collects it from the package root.
"""
import pathlib
import re
import shutil
import sys

tree = pathlib.Path(sys.argv[1])
SKIP = {".git", "target", "build", "node_modules"}


def one(matches, what):
    if len(matches) != 1:
        sys.exit(f"place_hidden: {what} matches {len(matches)} places: {sorted(map(str, matches))}")
    return matches[0]


def usable(path):
    return not SKIP.intersection(path.relative_to(tree).parts)


def java_destination(source):
    declared = re.search(r"^\s*package\s+([\w.]+)\s*;", source.read_text(), re.M)
    if not declared:
        sys.exit(f"place_hidden: {source} declares no package")
    package = declared.group(1).replace(".", "/")
    owner = one(
        [d for d in tree.glob(f"**/src/main/java/{package}") if d.is_dir() and usable(d)],
        f"package {declared.group(1)}",
    )
    # <module>/src/main/java/<package> -> strip the package and the three source-root parts.
    module = owner.parents[len(package.split("/")) + 2]
    return module / "src/test/java" / package / source.name


def typescript_destination(source):
    manifest = one(
        [p for p in tree.glob("**/package.json") if usable(p)],
        "package.json",
    )
    return manifest.parent / source.name


placed = []
for directory in sys.argv[2:]:
    files = sorted(p for p in pathlib.Path(directory).iterdir() if p.is_file())
    if not files:
        sys.exit(f"place_hidden: no hidden tests in {directory}")
    for source in files:
        if source.suffix == ".java":
            destination = java_destination(source)
        elif source.name.endswith(".test.ts"):
            destination = typescript_destination(source)
        else:
            sys.exit(f"place_hidden: no route for {source}")
        # B1 and B2 both ship a class named mn.shop.api.HiddenTest. Only one task is ever
        # graded at a time so they cannot collide, but a grader that assumed that rather than
        # checking it would silently clobber one of them and grade the survivor twice.
        if destination.exists():
            sys.exit(f"place_hidden: {destination} already exists; {source} would clobber it")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        placed.append(str(destination.relative_to(tree)))

print("\n".join(placed))
PY
)"

# --- the two builds ----------------------------------------------------------------------------

# :Z relabels the mount for SELinux. Without it the container sees an empty /work and Maven
# reports a missing POM from /tmp/hsperfdata_root, which looks nothing like the real problem.
run_build() {  # tree, log -> BUILD_STATUS
  local tree="$1" log="$2"
  BUILD_STATUS=0
  docker run --rm --network=none \
    -v "$tree:/work:Z" -w /work \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    "$IMAGE" \
    bash -lc 'fixture-build .; status=$?; chown -R "$HOST_UID:$HOST_GID" /work 2>/dev/null || true; exit $status' \
    >"$log" 2>&1 </dev/null || BUILD_STATUS=$?
  # 125 is docker failing, 126/127 the container failing to run the command. None of them is a
  # verdict about the agent's diff, and recording one as a failed gate would zero a run for a
  # reason nothing in the results distinguishes from a bad fix.
  case "$BUILD_STATUS" in
    125 | 126 | 127)
      echo "grade.sh: the build container did not run (exit $BUILD_STATUS); see $log" >&2
      exit 1
      ;;
  esac
}

run_build "$BASELINE" "$OUT/grade-baseline.log"; BASELINE_EXIT="$BUILD_STATUS"
run_build "$GRADED" "$OUT/grade-hidden.log";     GRADED_EXIT="$BUILD_STATUS"

# --- the verdict --------------------------------------------------------------------------------

SITES="$(python3 "$LOOKUP" "$TASK" required_sites)"

python3 - "$OUT" "$TASK" "$REPO" "$COMMIT" "$BASELINE_EXIT" "$GRADED_EXIT" \
             "$DIFF_EMPTY" "$DIFF_APPLIED" "$PLACED" "$SITES" <<'PY'
import json
import pathlib
import re
import sys

out, task, repo, commit = sys.argv[1:5]
baseline_exit, graded_exit = int(sys.argv[5]), int(sys.argv[6])
diff_empty, diff_applied = sys.argv[7] == "true", sys.argv[8] == "true"
placed = [line for line in sys.argv[9].splitlines() if line]
sites = [line for line in sys.argv[10].splitlines() if line]
run = pathlib.Path(out)

# A compiler's own failure banner, in the three toolchains this corpus builds with. Used only
# to attribute a failure between L0 and L2, and to tell a hidden test that would not compile
# from one that ran and failed an assertion — never to decide `passed`.
COMPILER_FAILED = re.compile(
    r"COMPILATION ERROR|Compilation failed|compile\w*Java FAILED|error TS\d+", re.I
)

baseline_log = (run / "grade-baseline.log").read_text(errors="replace")
graded_log = (run / "grade-hidden.log").read_text(errors="replace")
adjudicate = []

# L0 — the project the agent left still compiles. L2 — and its own tests still pass. One
# command answers both; the banner says which half went red when it did.
if baseline_exit == 0:
    l0, l2 = True, True
elif COMPILER_FAILED.search(baseline_log):
    l0, l2 = False, False
    adjudicate.append("collateral-unknown-build-failed")
else:
    l0, l2 = True, False

# L1 — the primary gate. The graded tree is the baseline tree plus the hidden tests, so a green
# graded run means the hidden tests passed. A red one means they did not *or* the baseline was
# already red, which is why a red baseline is flagged rather than silently folded in.
l1 = graded_exit == 0
if baseline_exit != 0 and not l1:
    adjudicate.append("l1-not-isolated")

# A hidden test that fails to compile is a different outcome from one that fails an assertion:
# A1 calls `new PaymentValidator()`, so an agent that makes the cap configurable by constructor
# injection writes a legitimate fix that cannot be graded. The baseline tree compiled without
# the hidden tests, so a compile failure with them present is theirs.
if baseline_exit == 0 and graded_exit != 0 and COMPILER_FAILED.search(graded_log):
    adjudicate.append("hidden-test-compile-error")

if not diff_applied:
    adjudicate.append("diff-did-not-apply")

# L3 — reported, never a gate. Which of the sites the task declares does the diff touch? The
# paths are taken from the patch's own file headers rather than by searching the whole patch
# text, so a path that merely appears in a context line is not counted as an edit. A site that
# names a directory (C1's migration directory) matches anything under it.
touched = set()
for line in pathlib.Path(out, "diff.patch").read_text(errors="replace").splitlines():
    for pattern in (r"^--- a/(.+)$", r"^\+\+\+ b/(.+)$", r"^rename (?:from|to) (.+)$"):
        found = re.match(pattern, line)
        if found:
            touched.add(found.group(1))

found_sites = [s for s in sites if any(p == s or p.startswith(s + "/") for p in touched)]

grade = {
    "task": task,
    "repo": repo,
    "commit": commit,
    "L0_build": l0,
    "L1_hidden": l1,
    "L2_collateral": l2,
    "L3_sites_found": found_sites,
    "L3_sites_missed": [s for s in sites if s not in found_sites],
    "passed": l0 and l1 and l2,
    "diff_empty": diff_empty,
    "diff_applied": diff_applied,
    "hidden_tests_placed": placed,
    "baseline_exit": baseline_exit,
    "graded_exit": graded_exit,
    # Empty in the ordinary case. Anything here is a run whose verdict a human should read the
    # logs for before it is counted, rather than a silent scored zero.
    "adjudicate": adjudicate,
}
(run / "grade.json").write_text(json.dumps(grade, indent=2) + "\n")
PY

# The verdict is a file. Nothing about a run goes to stdout but the directory holding it.
echo "$OUT"
