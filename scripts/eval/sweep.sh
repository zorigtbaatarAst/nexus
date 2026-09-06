#!/usr/bin/env bash
# All 75 runs: 5 tasks x 3 arms x 5 repetitions. This is what spends real money — hundreds of
# paid `claude -p` invocations across `make bench` — so every decision below is aimed at not
# spending it twice and not silently failing to spend it at all.
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

STAMP="${STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
BASE="$ROOT/docs/eval/runs/$STAMP"
# Printed first, and printed on every invocation: an interrupted sweep is resumed by passing
# this stamp back, and an operator who cannot find it re-pays for every completed cell.
echo "sweep.sh: STAMP=$STAMP — resume this sweep with: make bench STAMP=$STAMP"
REPS="${REPS:-5}"
IMAGE="${IMAGE:-nexus-bench:latest}"
NEXUS_BIN="$ROOT/target/release/nexus"

# The five benchmark tasks, overridable so a dry run can exercise the refusal path (M1, E2) that
# never belongs in a real sweep — that path is proven by constructing it, not by argument.
read -ra TASKS <<<"${TASKS:-A1-idempotency-key-length A2-shared-type-change B1-rename-crosses-the-seam B2-orphaned-field-diagnosis C1-regression-recognised}"
ARMS=(A0 A1 A5)

# A sweep with no ANTHROPIC_API_KEY runs the host's own ~/.claude credentials through 75 root
# containers (see run.sh for why the mount is read-write and why read-only isn't a fix). A token
# refresh inside any one of those containers rotates the host's copy server-side; every later
# cell then replays a consumed refresh token, which can log the operator out mid-sweep and burn
# hours producing nothing. It does not corrupt results (auth failures land in the infra bucket)
# or leak anything off the host — it just wastes the sweep. Warning only: a stale key would fail
# all 75 cells silently, which is worse, so this never overrides working ~/.claude credentials.
if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  echo "sweep.sh: ANTHROPIC_API_KEY is not set — this sweep will run your host's ~/.claude" >&2
  echo "credentials through 75 root containers. A token refresh inside one of them can rotate" >&2
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
  read -r PREV_IMAGE_ID PREV_MODEL < <(python3 -c "
import json
m = json.load(open('$META'))
print(m['image_id'], m['model'])
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
else
  python3 - "$META" "$STAMP" "$IMAGE" "$IMAGE_ID" "$NEXUS_VERSION" "$MODEL" "$REPS" <<'PY'
import json
import sys

path, stamp, image, image_id, nexus_version, model, reps = sys.argv[1:8]
json.dump(
    {
        "stamp": stamp,
        "image": image,
        "image_id": image_id,
        "nexus_version": nexus_version,
        "model": model,
        "reps": int(reps),
    },
    open(path, "w"),
    indent=2,
)
PY
  echo "sweep.sh: stamped $META (image $IMAGE_ID, $NEXUS_VERSION)"
fi

# --- the sweep itself -----------------------------------------------------------------------

for task in "${TASKS[@]}"; do
  for arm in "${ARMS[@]}"; do
    for rep in $(seq 0 $((REPS - 1))); do
      out="$BASE/$task/$arm/$rep"

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
  done
done

echo "sweep complete: $BASE"
