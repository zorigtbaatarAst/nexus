#!/usr/bin/env bash
# Does parity.sh's verdict actually depend on anything?  Free — no containers, no API calls.
#
# parity.sh is the check that retires R-b, so a parity.sh that returns `parity ok` regardless
# would retire the design's most dangerous risk on no evidence at all. Every case below is a
# hand-built pair of run directories that MUST be rejected, plus the healthy pair that must be
# accepted. Two of them (asymmetric-cache, a1-inflated) are the reason the magnitude assertion
# is bounded on both sides rather than only from below — they passed a lower bound alone.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
FAILURES=0

# A healthy pair, close to the real measured run: A1's input side exceeds A0's by 706 tokens
# against 2,073 injected bytes.
seed() {
  rm -rf "$WORK/A0" "$WORK/A1"
  mkdir -p "$WORK/A0" "$WORK/A1"
  python3 - "$WORK" <<'PY'
import json, sys, pathlib
w = pathlib.Path(sys.argv[1])
# The measured 2026-09-06 run, verbatim: input-side 20,652 against 21,358, delta 706.
for arm, creation, read in (("A0", 7027, 13615), ("A1", 3619, 17729)):
    (w / arm / "usage.json").write_text(json.dumps({
        "task": "T", "arm": arm, "repetition": 0, "model": "claude-haiku-4-5",
        "input_tokens": 10, "output_tokens": 98,
        "cache_read_tokens": read, "cache_creation_tokens": creation,
        "total_cost_usd": 0.01, "num_turns": 1, "duration_api_ms": 1000,
        "claude_exit": 0, "claimed_done": False,
    }))
(w / "A1/injected.log").write_text(
    "=== SessionStart injected=258 bytes\n"
    "=== UserPromptSubmit prompt=268 injected=1815 bytes\n"
)
PY
}

# edit <arm> <python-fragment mutating the dict `d`>
edit() {
  python3 - "$WORK/$1/usage.json" "$2" <<'PY'
import json, sys
path, frag = sys.argv[1], sys.argv[2]
d = json.load(open(path))
exec(frag)
json.dump(d, open(path, "w"))
PY
}

# expect <exit-code> <substring of the expected message> <case name>
expect() {
  local want="$1" needle="$2" name="$3" out rc
  out="$(OUT="$WORK" REUSE=1 bash "$ROOT/scripts/eval/parity.sh" 2>&1)"; rc=$?
  if [ "$rc" != "$want" ]; then
    echo "FAIL $name: exit $rc, expected $want"; echo "$out" | sed 's/^/    /'
    FAILURES=$((FAILURES + 1)); return
  fi
  if ! printf '%s' "$out" | grep -qF "$needle"; then
    echo "FAIL $name: exit $rc as expected, but no message matching '$needle'"
    echo "$out" | sed 's/^/    /'
    FAILURES=$((FAILURES + 1)); return
  fi
  echo "ok   $name"
}

seed; expect 0 "parity ok" "healthy pair is accepted"

# 1-2: the arm under test never really injected — the failure that would make every other
# assertion below compare two bare arms and call it parity.
seed
printf '=== SessionStart injected=0 bytes\n=== UserPromptSubmit prompt=0 injected=0 bytes\n' \
  > "$WORK/A1/injected.log"
expect 1 "zero-length prompt" "A1's hook fired but read nothing (the \$CLAUDE_USER_PROMPT mode)"

seed; rm "$WORK/A1/injected.log"
expect 1 "no injected.log" "A1's hooks never ran"

# 3: both arms injecting is not the comparison the benchmark reports.
seed; cp "$WORK/A1/injected.log" "$WORK/A0/injected.log"
expect 1 "A0 wrote an injected.log" "the bare arm is injecting too"

# 4-6: the magnitude assertion, in all three directions it can be wrong.
seed; edit A1 'd["cache_creation_tokens"] = 7027; d["cache_read_tokens"] = 13615'
expect 1 "not the injected package" "A1's package is not charged at all"

seed; edit A0 'd["cache_read_tokens"] = 0; d["cache_creation_tokens"] = 0'
expect 1 "not the injected package" "A0's cache counters are not recorded — asymmetric, is R-b"

seed; edit A1 'd["input_tokens"] += 20000'
expect 1 "not the injected package" "A1 charged 20k tokens that are not the package"

# 7: two all-zero records agree perfectly. This is how the check most easily passes wrongly.
seed
edit A0 'd.update(input_tokens=0, output_tokens=0, cache_read_tokens=0, cache_creation_tokens=0, num_turns=0, total_cost_usd=0.0, claude_exit=124)'
edit A1 'd.update(input_tokens=0, output_tokens=0, cache_read_tokens=0, cache_creation_tokens=0, num_turns=0, total_cost_usd=0.0, claude_exit=124)'
expect 1 "a killed run" "both arms killed at the timeout, all counters zero"

# 8: the arms counted from different fields — R-b in its most literal form.
seed; edit A1 'd["cache_read_input_tokens"] = d.pop("cache_read_tokens")'
expect 1 "different fields" "A1's usage.json names a different field"

# 9: one arm priced and the other not.
seed; edit A0 'd["total_cost_usd"] = 0.0'
expect 1 "populated for one arm only" "only A1 has a dollar figure"

# 10: the agent worked, so the magnitude comparison is about its choices, not the package.
seed; edit A1 'd["num_turns"] = 5'
expect 1 "INCONCLUSIVE" "A1 took more than two turns"

if [ "$FAILURES" -ne 0 ]; then
  echo "parity_selftest: $FAILURES case(s) failed"
  exit 1
fi
echo "parity_selftest: all cases behaved as specified"
