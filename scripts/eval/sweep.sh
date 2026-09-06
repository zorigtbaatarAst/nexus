#!/usr/bin/env bash
# All 75 runs: 5 tasks x 3 arms x 5 repetitions. This is what spends real money — hundreds of
# paid `claude -p` invocations across `make bench` — so every decision below is aimed at not
# spending it twice and not silently failing to spend it at all.
#
# Resumable: a run whose grade.json already exists is skipped. A run whose usage.json exists
# but grade.json does not already paid for the agent; only grading (offline, free) is redone —
# re-running the agent there would spend the same run twice. Anything else — no usage.json at
# all, including a directory run.sh's refusal guard left empty — is a full run.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RUN="$ROOT/scripts/eval/run.sh"
GRADE="$ROOT/scripts/eval/grade.sh"
GATE="$ROOT/scripts/eval/test_grade.sh"

STAMP="${STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
BASE="$ROOT/docs/eval/runs/$STAMP"
REPS="${REPS:-5}"
IMAGE="${IMAGE:-nexus-bench:latest}"
NEXUS_BIN="$ROOT/target/release/nexus"

# The five benchmark tasks, overridable so a dry run can exercise the refusal path (M1, E2) that
# never belongs in a real sweep — that path is proven by constructing it, not by argument.
read -ra TASKS <<<"${TASKS:-A1-idempotency-key-length A2-shared-type-change B1-rename-crosses-the-seam B2-orphaned-field-diagnosis C1-regression-recognised}"
ARMS=(A0 A1 A5)

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
  # Resuming: the image and nexus build that produced the runs already in this tree must still
  # be the ones on disk, or a rebuild between invocations would silently mix two builds' results
  # into one sweep with nothing in the run tree to say so.
  PREV_IMAGE_ID="$(python3 -c "import json; print(json.load(open('$META'))['image_id'])")"
  if [ "$PREV_IMAGE_ID" != "$IMAGE_ID" ]; then
    echo "sweep.sh: $BASE was stamped with image $PREV_IMAGE_ID," >&2
    echo "but $IMAGE is now $IMAGE_ID. Refusing to resume with a rebuilt image half-mixed in." >&2
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

      if [ -f "$out/grade.json" ]; then
        echo "skip       $task/$arm/$rep (already graded)"
        continue
      fi

      if [ -f "$out/usage.json" ]; then
        # The agent already ran and cost money; only grading (offline, free) is owed here.
        # Re-running run.sh would spend that run a second time for a diff it already produced.
        echo "grade-only $task/$arm/$rep (usage.json present, no grade.json)"
        "$GRADE" "$task" "$out" >/dev/null
        continue
      fi

      # No usage.json: never started, or run.sh's refusal guard left an empty directory behind
      # (it does `mkdir -p "$OUT"` before refusing). Either way a full run is owed, and if this
      # is in fact a refused task, run.sh exits 1 here, `set -e` stops the sweep, and the refusal
      # reaches whoever is watching instead of being swallowed as a skip.
      echo "run        $task/$arm/$rep"
      "$RUN" "$task" "$arm" "$rep" "$out" >/dev/null
      "$GRADE" "$task" "$out" >/dev/null
    done
  done
done

echo "sweep complete: $BASE"
