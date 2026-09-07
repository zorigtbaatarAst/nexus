#!/usr/bin/env bash
# All 95 runs: 5 tasks x 3 arms x 5 reps, plus 2 ranking-only tasks x 2 arms x 5 reps. This is
# what spends real money — hundreds of paid `claude -p` invocations across `make bench` — so
# every decision below is aimed at not spending it twice and not silently failing to spend it
# at all.
#
# Resumable: a run with a non-empty grade.json is skipped. A run with .run-started, diff.patch
# or usage.json but no complete grade.json already paid for the agent (see run.sh: .run-started
# is written immediately before the container starts, so it catches a crash during the run, not
# only after); only grading (offline, free) is redone there. Anything else is a full run.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RUN="$ROOT/scripts/eval/run.sh"
GRADE="$ROOT/scripts/eval/grade.sh"
GATE="$ROOT/scripts/eval/test_grade.sh"
CHECK_ARTIFACTS="$ROOT/scripts/eval/check_image_artifacts.sh"

STAMP="${STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
BASE="$ROOT/docs/eval/runs/$STAMP"
# Printed first, and printed on every invocation: an interrupted sweep is resumed by passing
# this stamp back, and an operator who cannot find it re-pays for every completed cell.
echo "sweep.sh: STAMP=$STAMP — resume this sweep with: make bench STAMP=$STAMP"
REPS="${REPS:-5}"
IMAGE="${IMAGE:-nexus-bench:latest}"
NEXUS_BIN="$ROOT/target/release/nexus"

# The seven benchmark tasks, overridable so a dry run can exercise the refusal path (M1, E2) that
# never belongs in a real sweep — that path is proven by constructing it, not by argument.
read -ra TASKS <<<"${TASKS:-A1-idempotency-key-length A2-shared-type-change B1-rename-crosses-the-seam B2-orphaned-field-diagnosis C1-regression-recognised E1-untested-change N1-null-task}"

# The corpus is asymmetric on purpose, and the asymmetry is the whole point of the two added
# tasks — so it is expressed per task, travelling with the task id, rather than as a second task
# list that a `TASKS=` override could silently drop half of.
#
# E1 and N1 exist here only to give the ranking comparison enough non-tied observations: with
# five tasks and two ties the sign test's best achievable p is 0.125 and T7's threshold of 0.10
# is unreachable. A0 contributes nothing to a ranking of two rankings, so running it would cost
# five paid runs a task for no statistical power.
#
# Giving them A0 as well is the expensive mistake, and it is silent: the two pre-registered
# thresholds rest on different task sets by design — T4 (efficiency, A1 vs A0) on five tasks at
# three arms, T7 (ranking, A1 vs A5) on seven at two — and analyse.py derives each comparison's
# task set from the run tree rather than from a list of its own. Run A0 here and both derived
# sets become identical, T4 quietly becomes a seven-task threshold, and nothing in the output
# says so. The error would land in the result rather than in the harness.
arms_for() {  # task id -> the arms it runs, space separated
  case "$1" in
    E1-untested-change | N1-null-task) echo "A1 A5" ;;
    *) echo "A0 A1 A5" ;;
  esac
}

# ARMS restricts which arms each task runs, for a validation sweep that only needs one arm (see
# Task 4 of the tier2-instrument-repair plan: an A0-only run to check the corpus, before any A1
# exists to compare against). It can only ever narrow arms_for()'s answer, never widen it: it is
# intersected per task, so ARMS=A0 still runs zero cells on E1/N1, which arms_for() never gives
# A0 in the first place. Reusing the intersected result as the loop variable inside arms_for()'s
# own case statement would be the same silent-widening bug this file is written to avoid, so the
# per-task result below is named TASK_ARMS, never ARMS — arms_for() itself never sees the override.
#
# Any arm named in ARMS that arms_for() would never return for *any* task is refused up front
# rather than silently intersected away to nothing: a typo here should fail loudly, not produce
# a quietly-empty plan that reads as "ran, found nothing."
if [ -n "${ARMS:-}" ]; then
  ALL_ARMS="$(arms_for '')"  # the "*" branch is arms_for()'s superset of every arm any task gets
  for arm in $ARMS; do
    case " $ALL_ARMS " in
      *" $arm "*) ;;
      *)
        echo "sweep.sh: ARMS names '$arm', which arms_for() never returns for any task" \
          "(known arms: $ALL_ARMS). Refusing rather than silently planning zero cells for it." >&2
        exit 1
        ;;
    esac
  done
fi

# The cell plan, built once and then *executed* below — the plan and the sweep cannot disagree
# because they are the same list. That is what makes DRY_RUN worth having: a typo in either arm
# of the case above silently hands an added task three arms, and this ticket's own trap would
# then be living in the line meant to prevent it. Nothing else in the repo executes this file,
# so test_grade.sh asserts the plan (against the requested TASKS/ARMS/REPS, 95 cells by default,
# no A0 on the added tasks) before it grades anything — free, and ahead of every container.
#
# The exit is here, before the credential warning, the guards and the gate: no docker, no built
# binary, nothing on disk, and no recursion when the gate is the caller.
CELLS=()
for task in "${TASKS[@]}"; do
  read -ra TASK_ARMS <<<"$(arms_for "$task")"
  if [ -n "${ARMS:-}" ]; then
    SELECTED_ARMS=()
    for arm in "${TASK_ARMS[@]}"; do
      case " $ARMS " in
        *" $arm "*) SELECTED_ARMS+=("$arm") ;;
      esac
    done
    TASK_ARMS=("${SELECTED_ARMS[@]}")
  fi
  for arm in "${TASK_ARMS[@]}"; do
    for rep in $(seq 0 $((REPS - 1))); do
      CELLS+=("$task/$arm/$rep")
    done
  done
done

if [ "${DRY_RUN:-0}" = "1" ]; then
  printf '%s\n' "${CELLS[@]}"
  exit 0
fi

# A sweep with no ANTHROPIC_API_KEY runs the host's own ~/.claude credentials through 95 root
# containers (see run.sh for why the mount is read-write and why read-only isn't a fix). A token
# refresh inside any one of those containers rotates the host's copy server-side; every later
# cell then replays a consumed refresh token, which can log the operator out mid-sweep and burn
# hours producing nothing. It does not corrupt results (auth failures land in the infra bucket)
# or leak anything off the host — it just wastes the sweep. Warning only: a stale key would fail
# all 95 cells silently, which is worse, so this never overrides working ~/.claude credentials.
if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  echo "sweep.sh: ANTHROPIC_API_KEY is not set — this sweep will run your host's ~/.claude" >&2
  echo "credentials through 95 root containers. A token refresh inside one of them can rotate" >&2
  echo "the host's copy and log you out mid-sweep. Set ANTHROPIC_API_KEY to avoid this." >&2
fi

# The model is pinned for every reported sweep. run.sh already defaults to claude-opus-5, but a
# stray MODEL left set in the shell from an earlier cheap-model dry run must not ride along into
# a real sweep and silently mix models — that produces numbers that mean nothing.
MODEL="${MODEL:-claude-opus-5}"
if [ "$MODEL" != "claude-opus-5" ] && [ "${ALLOW_MODEL_OVERRIDE:-0}" != "1" ]; then
  echo "sweep.sh: MODEL=$MODEL, not claude-opus-5. A sweep that mixes models produces numbers" >&2
  echo "that mean nothing. Set ALLOW_MODEL_OVERRIDE=1 if this is deliberate." >&2
  exit 1
fi
export MODEL

# --- pre-flight gate: the grader must be trustworthy before anything is spent -------------------

SKIP_GATE="${SKIP_GATE:-0}"
for arg in "$@"; do
  [ "$arg" = "--skip-gate" ] && SKIP_GATE=1
done

if [ "$SKIP_GATE" = "1" ]; then
  echo "sweep.sh: pre-flight gate skipped (SKIP_GATE=1 / --skip-gate)"
else
  echo "sweep.sh: pre-flight — scripts/eval/test_grade.sh must pass before any run is paid for"
  if ! "$GATE"; then
    echo "sweep.sh: test_grade.sh failed. A grader stuck at 'passed: false' reads as a" >&2
    echo "devastating result for every arm rather than as a bug. Refusing to start the sweep." >&2
    echo "Fix the grader, or re-run with SKIP_GATE=1 / --skip-gate once you've verified it" >&2
    echo "yourself (e.g. after a re-run where nothing about grade.sh changed)." >&2
    exit 1
  fi
  echo "sweep.sh: pre-flight gate passed"
fi

# --- stamp what produced the numbers, once, at the top of the run tree --------------------------

[ -x "$NEXUS_BIN" ] || { echo "sweep.sh: $NEXUS_BIN missing; run make release first" >&2; exit 1; }
NEXUS_VERSION="$("$NEXUS_BIN" --version)"
IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$IMAGE" 2>/dev/null)" \
  || { echo "sweep.sh: no image $IMAGE; build it with make bench-image first" >&2; exit 1; }

mkdir -p "$BASE"
META="$BASE/meta.json"
if [ -f "$META" ]; then
  # Resuming: the image, the nexus build and the model that produced the runs already in this
  # tree must still be the ones this invocation is about to use, or the mismatch would silently
  # mix two builds' — or two models' — results into one sweep with nothing in the run tree to
  # say so. Neither comparison is skippable: not by SKIP_GATE, and the model one specifically
  # not by ALLOW_MODEL_OVERRIDE, which is an escape hatch for a deliberate run on a *fresh*
  # stamp, never for resuming an existing one under a different model.
  #
  # Note this does not cover a `make fixtures` regeneration between invocations of the same
  # stamp: the corpus is copied into the image at build time, so the image id is unchanged, but
  # a regenerated corpus would still change what run.sh/grade.sh actually clone at run time.
  # Out of scope for what this task was asked to stamp (image + nexus version) — flagged here so
  # a future reader doesn't assume this guard is complete.
  #
  # The image/tree content check below is deliberately NOT repeated here. Resuming compares the
  # image to the *stamp*, not to the tree: image ids are content addresses, so a matching
  # PREV_IMAGE_ID already proves the binary inside is byte-identical to the one the earlier
  # cells ran — the stamped nexus_image_sha256 is what the sweep is reported against, and it
  # cannot have moved. Re-checking against the tree here would instead refuse every resume in
  # which someone rebuilt target/release/nexus during the hours the sweep was running, and the
  # only way out of that refusal is a rebuilt image, which the id pin then refuses in turn. A
  # guard whose two halves deadlock gets disabled, and then guards nothing.
  {
    read -r PREV_IMAGE_ID PREV_MODEL PREV_REPS
    read -r PREV_TASKS
  } < <(python3 -c "
import json
m = json.load(open('$META'))
print(m['image_id'], m['model'], m['reps'])
print(' '.join(m.get('tasks', [])))
")
  if [ "$PREV_IMAGE_ID" != "$IMAGE_ID" ]; then
    echo "sweep.sh: $BASE was stamped with image $PREV_IMAGE_ID," >&2
    echo "but $IMAGE is now $IMAGE_ID. Refusing to resume with a rebuilt image half-mixed in." >&2
    exit 1
  fi
  if [ "$PREV_MODEL" != "$MODEL" ]; then
    echo "sweep.sh: $BASE was stamped with model $PREV_MODEL, but this invocation is using" >&2
    echo "$MODEL. Refusing to mix models within one sweep, even with ALLOW_MODEL_OVERRIDE set." >&2
    exit 1
  fi
  # `reps` decides how many runs each median is taken over. Resume at a different one and some
  # cells have five observations and some have three, with nothing in the tree saying which —
  # the same class of silent corruption as a mixed model, and it was stamped but never compared.
  if [ "$PREV_REPS" != "$REPS" ]; then
    echo "sweep.sh: $BASE was stamped with reps=$PREV_REPS, but this invocation is using" >&2
    echo "reps=$REPS. Resuming at a different rep count gives one sweep two sample sizes." >&2
    echo "To resume this sweep: re-run with REPS=$PREV_REPS. To take more repetitions, start a" >&2
    echo "fresh stamp (unset STAMP) at the rep count you want; do not extend this one in place." >&2
    exit 1
  fi
  # A subset, deliberately, not equality: `TASKS=` naming one task is the documented way to
  # finish a sweep that died on it, and must keep working. What is refused is a task the stamp
  # never had — that quietly enlarges the corpus, and since analyse.py derives each comparison's
  # task set from the run tree, it moves what T4 and T7 rest on with nothing reporting it.
  # (Empty means a tree stamped before `tasks` was recorded; nothing to compare against.)
  if [ -n "$PREV_TASKS" ]; then
    for task in "${TASKS[@]}"; do
      case " $PREV_TASKS " in
        *" $task "*) ;;
        *)
          echo "sweep.sh: $BASE was stamped for tasks: $PREV_TASKS" >&2
          echo "$task is not one of them. Refusing to add a task to a sweep already in progress." >&2
          echo "To resume this sweep: pass TASKS= naming only stamped tasks, or omit it. To run" >&2
          echo "$task, start a fresh stamp (unset STAMP) — a task added here would enlarge the" >&2
          echo "corpus analyse.py derives T4's and T7's task sets from, with nothing reporting it." >&2
          exit 1
          ;;
      esac
    done
  fi
else
  # A fresh sweep is reported against this tree, so everything the image bakes from the tree
  # must be this tree's copy — the tool under test, the hook that injects context, and the L0
  # build-and-grade script. Compared by content, and stamped by content: `nexus_version` below
  # is the host binary's --version, which is what stamped `nexus 0.3.0` over an image built a
  # day earlier. It stays for readability; the sha256s are the fields that can actually be
  # checked, because they are taken from inside the image and change with every edit whether or
  # not anyone bumped a version. Anyone can rebuild a commit and compare them.
  # See check_image_artifacts.sh, including what it does NOT cover and why.
  IMAGE_ARTIFACTS="$("$CHECK_ARTIFACTS" "$IMAGE" "$ROOT")"

  python3 - "$META" "$STAMP" "$IMAGE" "$IMAGE_ID" "$NEXUS_VERSION" "$IMAGE_ARTIFACTS" "$MODEL" "$REPS" "${TASKS[@]}" <<'PY'
import json
import sys

path, stamp, image, image_id, nexus_version, image_artifacts, model, reps = sys.argv[1:9]
tasks = sys.argv[9:]
artifacts = dict(line.split() for line in image_artifacts.splitlines() if line.strip())
json.dump(
    {
        "stamp": stamp,
        "image": image,
        "image_id": image_id,
        "nexus_version": nexus_version,
        # Every checked artifact, so a report cannot claim clean provenance for a set that was
        # never compared. nexus_image_sha256 is the same value as the map's /usr/local/bin/nexus
        # entry, kept because the docs and the incident write-up name it: one derivation, two
        # names, and the duplicate is written here rather than being able to disagree.
        "nexus_image_sha256": artifacts["/usr/local/bin/nexus"],
        "image_artifact_sha256": artifacts,
        "model": model,
        "reps": int(reps),
        "tasks": sorted(tasks),
    },
    open(path, "w"),
    indent=2,
)
PY
  echo "sweep.sh: stamped $META (image $IMAGE_ID, $NEXUS_VERSION," \
    "$(printf '%s\n' "$IMAGE_ARTIFACTS" | wc -l) artifacts verified against the tree)"
fi

# --- the sweep itself -----------------------------------------------------------------------

for cell in "${CELLS[@]}"; do
  IFS=/ read -r task arm rep <<<"$cell"
  out="$BASE/$cell"

  # -s, not -f: a grade.json that exists but is 0 bytes (a kill mid-write, before grade.sh's
  # atomic rename was in place, or any other truncation) must not read as a completed grade —
  # that silently drops the run from Task 9's medians instead of re-grading it for free.
  if [ -s "$out/grade.json" ]; then
    echo "skip       $task/$arm/$rep (already graded)"
    continue
  fi

  # Any of these three means the agent already ran and the money is already spent: usage.json
  # and diff.patch are both written only after the docker call returns, and .run-started is
  # written immediately before it — before diff.patch and usage.json exist at all, so it also
  # catches a host crash *during* the container run, not just after it. Re-running run.sh here
  # would spend the same cell a second time. If .run-started is the only one of the three
  # present, grade.sh will refuse loudly for lack of a diff.patch — correct: that host crash
  # happened mid-spend, and whether it was billed is not something this script can know from
  # here, so it fails loudly rather than guessing either "yes, redo it" or "no, count it done".
  if [ -f "$out/usage.json" ] || [ -f "$out/diff.patch" ] || [ -f "$out/.run-started" ]; then
    echo "grade-only $task/$arm/$rep (already paid, no completed grade)"
    # grade.sh's own refusal says only "no $OUT/diff.patch to grade", which names neither
    # the marker nor the decision it forces. Said here, before the exit, because the operator
    # who hits this at hour two reads it at hour nine.
    if [ -f "$out/.run-started" ] && [ ! -f "$out/diff.patch" ]; then
      echo "sweep.sh: $out has .run-started but no diff.patch — the host died mid-container." >&2
      echo "Whether that cell was billed cannot be determined from this run tree. Check the" >&2
      echo "account's billing, then delete .run-started to pay for it again, or leave the cell" >&2
      echo "out of the sweep. Guessing double-charges or silently drops a run, so this halts." >&2
    fi
    "$GRADE" "$task" "$out" >/dev/null
    continue
  fi

  # None of the three: never started, or run.sh's refusal guard left an empty directory
  # behind (it does `mkdir -p "$OUT"` before refusing). Either way a full run is owed, and if
  # this is in fact a refused task, run.sh exits 1 here, `set -e` stops the sweep, and the
  # refusal reaches whoever is watching instead of being swallowed as a skip.
  echo "run        $task/$arm/$rep"
  "$RUN" "$task" "$arm" "$rep" "$out" >/dev/null
  "$GRADE" "$task" "$out" >/dev/null
done

echo "sweep complete: $BASE"
