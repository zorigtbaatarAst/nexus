#!/usr/bin/env bash
# One benchmark run: one task, one arm, one repetition, one container.
#
# Per-run containers, not per-arm: a leftover target/ or .nexus/ from an earlier run reaching
# a later one is contamination inside an arm, which is the worst place to allow it.
set -euo pipefail

TASK="${1:?task id}"
ARM="${2:?arm: A0|A1|A5}"
REP="${3:?repetition}"
OUT="${4:?output directory}"
MODEL="${MODEL:-claude-opus-5}"
TIMEOUT_S="${TIMEOUT_S:-900}"
IMAGE="${IMAGE:-nexus-bench:latest}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

mkdir -p "$OUT"

# A task that declares a start state starts from a dirty working tree, and nothing here applies
# the patch. Running it anyway would run a different task than the one the spec describes and
# say nothing about it — the one failure mode worse than not running it at all.
if python3 "$ROOT/scripts/eval/task_lookup.py" "$TASK" start_state >/dev/null 2>&1; then
  echo "$TASK declares start_state; run.sh does not apply the working-tree patch yet" >&2
  exit 1
fi

# Which repository and commit this task starts from, read from the fixture manifests.
read -r REPO COMMIT PROMPT < <(python3 "$ROOT/scripts/eval/task_lookup.py" "$TASK")

FIXTURE="$ROOT/target/fixtures/$REPO"
[ -d "$FIXTURE" ] || { echo "run make fixtures first" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
git clone -q "$FIXTURE" "$WORK/repo"
git -C "$WORK/repo" checkout -q "$COMMIT"

mkdir -p "$WORK/repo/.claude"
cp "$ROOT/scripts/eval/arms/$ARM.json" "$WORK/repo/.claude/settings.json"

# The harness's own footprint must not reach the grader, which reads diff.patch. The arm
# configuration and the index are ours, not the run's work, so git never sees them.
printf '%s\n' '/.claude/' '/.nexus/' >> "$WORK/repo/.git/info/exclude"

# The prompt and the result live outside the repository for the same reason.
mkdir -p "$WORK/bench"
printf '%s' "$PROMPT" > "$WORK/bench/prompt"

# Credentials are copied into a throwaway per-run directory rather than mounting the real
# ~/.claude: a bind mount of it would have to be SELinux-relabelled, and relabelling the
# user's own Claude Code state to run a benchmark is not a trade worth making.
mkdir -p "$WORK/home"
if [ -f "$HOME/.claude/.credentials.json" ]; then
  install -m 600 "$HOME/.claude/.credentials.json" "$WORK/home/.credentials.json"
elif [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  echo "no ~/.claude/.credentials.json and no ANTHROPIC_API_KEY: the agent cannot authenticate" >&2
  exit 1
fi

# A1 and A5 need an index before the hooks can answer anything. A0 must not have one.
#
# The index is logged rather than discarded, and its exit code recorded. A scan that fails
# leaves the hooks with nothing to say, which looks from the outside exactly like an arm that
# helped no one — so the sweep has to be able to tell the two apart afterwards.
SETUP=""
if [ "$ARM" != "A0" ]; then
  # The group sits in an `if`, where `set -e` is suspended. Written as a bare group, a failing
  # `nexus scan` — the last command of the AND-OR list — aborts the container before claude runs
  # and before the log says why, and the trap then deletes the log unread.
  SETUP="if { nexus init && nexus scan; } >/bench/setup.log 2>&1; then echo 'setup exit=0'; else echo \"setup exit=\$?\"; fi >>/bench/setup.log;"
fi

# Written immediately before the container starts — not after, and not derived from anything
# the container produces. The money is spent inside the call below; a host crash any time from
# here through the diff/usage.json writes near the bottom of this script (OOM, a killed sweep,
# a reboot) must leave evidence that the spend already happened, or a resumed sweep re-runs this
# exact cell and pays for it twice. Every earlier guard in this script (start_state, the fixture
# checkout, the credentials check) has already passed by this point, so this file is never
# written for a cell that didn't reach the point of committing to spend.
touch "$OUT/.run-started"

# IS_SANDBOX=1 because the container runs as root and Claude Code otherwise refuses
# bypassPermissions there; the container is the sandbox.
# :Z relabels the per-run mounts for SELinux and is a no-op where SELinux is not enforcing.
docker run --rm \
  -v "$WORK/repo:/work:Z" \
  -v "$WORK/bench:/bench:Z" \
  -v "$WORK/home:/root/.claude:Z" \
  -e ANTHROPIC_API_KEY \
  -e IS_SANDBOX=1 \
  -e HOST_UID="$(id -u)" \
  -e HOST_GID="$(id -g)" \
  "$IMAGE" \
  bash -lc "
    set -e
    # The clone belongs to the host user, so git in the container calls it dubious and refuses
    # every command against it. Left unset, nexus reports vcs 'none' and the whole history
    # signal — the thing the C-family tasks measure — silently disappears.
    git config --global --add safe.directory /work
    cd /work
    $SETUP
    # The exit code is kept, not swallowed: a run killed at the timeout (124) and a run that
    # legitimately did nothing both write an all-zero usage.json, and Task 8 would take medians
    # over the difference.
    STATUS=0
    timeout ${TIMEOUT_S}s claude -p \"\$(cat /bench/prompt)\" \
      --model $MODEL \
      --output-format json \
      --permission-mode bypassPermissions \
      > /bench/result.json 2>/bench/stderr || STATUS=\$?
    echo \"\$STATUS\" > /bench/status
    # Everything the container wrote is owned by root; hand it back so the host can read the
    # diff and delete the workspace.
    chown -R \"\$HOST_UID:\$HOST_GID\" /work /bench /root/.claude 2>/dev/null || true
  " < /dev/null

# Everything the run produced, extracted before the workspace is destroyed. The diff is taken
# against the starting commit, not against HEAD: an agent that commits its own work would
# otherwise hand the grader an empty patch and be scored as having done nothing.
git -C "$WORK/repo" add -A >/dev/null 2>&1 || true
git -C "$WORK/repo" diff --cached "$COMMIT" > "$OUT/diff.patch"
cp "$WORK/bench/result.json" "$OUT/transcript.json" 2>/dev/null || echo '{}' > "$OUT/transcript.json"
cp "$WORK/bench/stderr" "$OUT/stderr.log" 2>/dev/null || true
cp "$WORK/bench/setup.log" "$OUT/setup.log" 2>/dev/null || true
# What the hooks actually put into the turn — absent for A0, which has none.
cp "$WORK/bench/injected.log" "$OUT/injected.log" 2>/dev/null || true

# -1 means the container never got as far as recording one.
CLAUDE_EXIT="$(cat "$WORK/bench/status" 2>/dev/null || echo -1)"

python3 - "$OUT" "$TASK" "$ARM" "$REP" "$MODEL" "$CLAUDE_EXIT" <<'PY'
import json, sys, pathlib
out, task, arm, rep, model, claude_exit = sys.argv[1:7]
d = pathlib.Path(out)
try:
    r = json.loads((d / "transcript.json").read_text())
except Exception:
    r = {}
u = r.get("usage", {}) or {}
# One accounting source for every arm. A0 has no hooks and A1 does, so counting these from
# anywhere but the same field would make the headline number an artefact of the harness.
(d / "usage.json").write_text(json.dumps({
    "task": task, "arm": arm, "repetition": int(rep), "model": model,
    "input_tokens": u.get("input_tokens", 0),
    "output_tokens": u.get("output_tokens", 0),
    "cache_read_tokens": u.get("cache_read_input_tokens", 0),
    "cache_creation_tokens": u.get("cache_creation_input_tokens", 0),
    "total_cost_usd": r.get("total_cost_usd", 0.0),
    "num_turns": r.get("num_turns", 0),
    "duration_api_ms": r.get("duration_api_ms", 0),
    # 0 is a run that finished, 124 one the timeout killed, anything else a crash. Without it
    # every one of those is the same all-zero row.
    "claude_exit": int(claude_exit),
    "claimed_done": "done" in (r.get("result") or "").lower(),
}, indent=2) + "\n")
PY

echo "$OUT"
