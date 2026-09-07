#!/usr/bin/env bash
# Materialise the tokio corpus: target/fixtures/tokio + target/fixtures/tokio.manifest.json.
#
# The generated corpus is built by `nexus fixture generate` from a list of blobs. This one
# cannot be — it is ~800 files of somebody else's repository — so it is *replayed* instead:
# clone tokio, and for each task write one commit on top of the fix commit's parent that adds
# the commit's own test file and nothing else. That commit is the task's start state: a red
# test the agent can run, and no fix.
#
# Deterministic by construction. The author, the committer and both dates are pinned, so the
# five start-state shas are a function of (upstream tokio, the hidden test files) and nothing
# else — two machines running this produce the same manifest, which is the same property
# `make fixtures-verify` gates on for the generated corpus.
#
#   TOKIO_SOURCE=<url or path>   where to clone from (default: upstream)
#
# A local clone is accepted and is what a rebuild should use: `git clone` from a path on the
# same filesystem hardlinks its objects, so this costs a second rather than a download.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SPEC="$ROOT/tests/fixtures/corpora/tokio/fixture.toml"
OUT="$ROOT/target/fixtures/tokio"
MANIFEST="$ROOT/target/fixtures/tokio.manifest.json"
SOURCE="${TOKIO_SOURCE:-$(python3 -c "
import tomllib
print(tomllib.load(open('$SPEC','rb'))['source']['url'])")}"

# Pinned, because the start-state shas are reported and resumed against. Any of these moving
# moves every sha, which a manifest comparison would catch — but only after the corpus had
# already been rebuilt, so they are pinned here rather than defaulted from the environment.
export GIT_AUTHOR_NAME="Nexus Fixtures" GIT_AUTHOR_EMAIL="fixtures@nexus.invalid"
export GIT_COMMITTER_NAME="Nexus Fixtures" GIT_COMMITTER_EMAIL="fixtures@nexus.invalid"
export GIT_AUTHOR_DATE="@1756000000 +0000" GIT_COMMITTER_DATE="@1756000000 +0000"

if [ -d "$OUT/.git" ]; then
  echo "tokio_fixture.sh: reusing $OUT (delete it to re-clone)"
  git -C "$OUT" fetch -q --all --tags || true
else
  rm -rf "$OUT"
  echo "tokio_fixture.sh: cloning $SOURCE -> $OUT"
  git clone -q "$SOURCE" "$OUT"
fi

# One row per task, read from the spec rather than repeated here: a second copy of the parent
# shas is a second thing to keep in step with the corpus, and the corpus is the spec.
TASKS="$(python3 - "$SPEC" <<'PY'
import sys, tomllib
doc = tomllib.load(open(sys.argv[1], "rb"))
for task in doc["task"]:
    print(task["commit"], task["id"], task["parent"], task["real_fix"])
PY
)"

# The image's pre-warm, written here rather than in the Dockerfile. It needs each task's start
# sha and its three commands, which live in a TOML file the image has no parser for — and
# baking a parser (or a second copy of the commands) into the image to read data this script
# already holds is two problems where there was one.
#
# It lands under target/fixtures, which the Dockerfile COPYs to /warm and deletes in the same
# layer, so it leaves no artifact in the final image for check_image_artifacts.sh to compare —
# the same reasoning that keeps the corpus itself out of that table.
PREWARM="$ROOT/target/fixtures/tokio.prewarm.sh"

COMMITS=""
while read -r id task parent fix; do
  [ -n "$id" ] || continue

  # The fix commit must be reachable, not just the parent: the grader's own test reconstructs
  # the historical fix from `git diff <parent> <fix>`, and a shallow or stale clone that lacks
  # it would fail there rather than here, after the image had been built.
  git -C "$OUT" cat-file -e "$fix^{commit}" 2>/dev/null || {
    echo "tokio_fixture.sh: $SOURCE has no commit $fix ($task)" >&2; exit 1; }

  git -C "$OUT" checkout -q --detach "$parent"
  git -C "$OUT" clean -qfdx
  # The start state's tests ARE the hidden tests — one copy, used twice. Anything else lets
  # the test the agent sees and the test the grader runs drift apart, and the grader would win
  # silently.
  HIDDEN="$ROOT/tests/eval/hidden/$task"
  [ -d "$HIDDEN" ] || { echo "tokio_fixture.sh: no hidden tests at $HIDDEN" >&2; exit 1; }
  ( cd "$HIDDEN" && find . -type f -print0 | tar --null -cf - -T - ) | tar -xf - -C "$OUT"
  git -C "$OUT" add -A
  git -C "$OUT" commit -q -m "bench($id): $task — the test from $fix, without its fix"
  SHA="$(git -C "$OUT" rev-parse HEAD)"
  git -C "$OUT" branch -q -f "bench/$id" HEAD
  COMMITS="$COMMITS$id $task $SHA
"
  echo "tokio_fixture.sh: $id $task -> $SHA"
done <<<"$TASKS"

# Leave the repository on its own default branch: run.sh clones and then checks out the task's
# sha, but a fixture left detached on the last task's start state is a confusing thing to open.
git -C "$OUT" checkout -q master 2>/dev/null || git -C "$OUT" checkout -q main

python3 - "$SPEC" "$PREWARM" "$COMMITS" <<'PY'
"""Emit the image's pre-warm script: one block per task, driven by the spec's own commands."""
import sys, tomllib

spec, out, commits = sys.argv[1:4]
shas = {line.split()[1]: line.split()[2] for line in commits.splitlines() if line.strip()}
doc = tomllib.load(open(spec, "rb"))

blocks = ["""#!/usr/bin/env bash
# GENERATED by scripts/eval/tokio_fixture.sh. Runs inside the benchmark image, once, at build
# time. Two jobs, and the second is the more important one:
#
#   1. Warm /cargo-target/<task> so a graded cell pays seconds instead of a cold minute.
#   2. Prove, in the container the sweep will actually use, that every start state COMPILES,
#      that its collateral tests are GREEN, and that its own test is RED. A start state whose
#      test already passes grades nothing — that is the exact defect that made the generated
#      corpus worthless — and here it fails the image build instead of a $37 sweep.
set -eux
cd /work
"""]
for task in doc["task"]:
    tid = task["id"]
    blocks.append(f"""
# --- {tid} -------------------------------------------------------------
git checkout -q --detach {shas[tid]}
git clean -qfdx
bench-mtime /work
export CARGO_TARGET_DIR=/cargo-target/{tid}
{task["build_cmd"]}
{task["collateral_cmd"]}
if {task["test_cmd"]}; then
  echo "prewarm: {tid} PASSES at its start state. It grades nothing. Refusing to build." >&2
  exit 1
fi
echo "prewarm: {tid} start state — builds, collateral green, own test red"
""")
open(out, "w").write("".join(blocks))
PY
echo "tokio_fixture.sh: wrote $PREWARM"

python3 - "$MANIFEST" "$SOURCE" "$COMMITS" <<'PY'
import json, sys

path, source, commits = sys.argv[1:4]
rows = [line.split() for line in commits.splitlines() if line.strip()]
json.dump(
    {
        "name": "tokio",
        "description": "tokio at five points in its own history: five real bugs, five real tests.",
        "role": "real-repository",
        "stack": ["rust", "cargo"],
        "source": source,
        "default_branch": "master",
        "commits": [
            {"id": i, "sha": sha, "branch": f"bench/{i}", "task": t} for i, t, sha in rows
        ],
    },
    open(path, "w"),
    indent=2,
)
PY
echo "tokio_fixture.sh: wrote $MANIFEST"
