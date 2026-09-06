#!/usr/bin/env bash
# Do A0 and A1 account for tokens the same way?  (design risk R-b)
#
# A0 has no hooks; A1 injects a context package on every prompt. If those two paths are
# measured from different fields, the headline cost-per-success number is an artefact of the
# harness and nothing in the sweep's own results would reveal it — every arm would still look
# internally consistent. So: run the same trivial prompt in both arms, through the same
# runner the sweep uses, and check that the two runs are described by the same record, that
# every accounting field is really populated in both, and that the difference between them is
# the injected package rather than a difference in how the arms are counted.
#
# The prompt is trivial on purpose. Two things follow from it, and both matter:
#   * the run is cheap and finishes inside a short timeout, so the check fits a two-run budget;
#   * neither arm uses tools, so the input-side difference between them is the package and
#     almost nothing else. On a real task the agent's own choices dominate the token counts and
#     the magnitude comparison says nothing.
# It still names real fixture symbols, because a prompt with no identifiers makes the ranker
# select nothing and A1 injects zero bytes — a healthy hook with nothing to say, which is
# exactly the state this check must not mistake for a dead one.
#
# Cost: two paid `claude -p` runs. Cheap model by default; re-running the assertions against an
# existing output directory is free (see REUSE below).
#
#   ./scripts/eval/parity.sh              # two paid runs, then assert
#   REUSE=1 ./scripts/eval/parity.sh      # assert against an existing $OUT, spend nothing
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${OUT:-/tmp/bench-parity}"
MODEL="${MODEL:-claude-haiku-4-5}"
TIMEOUT_S="${TIMEOUT_S:-180}"
TASK="${TASK:-A1-idempotency-key-length}"

# This check needs a working accounting path, not a good model. Paying opus prices for it is
# almost certainly a stray MODEL from another shell rather than an intention.
if [ "$MODEL" = "claude-opus-5" ] && [ "${ALLOW_EXPENSIVE:-0}" != "1" ]; then
  echo "parity.sh: MODEL=$MODEL. The parity check exercises the same accounting path on a" >&2
  echo "cheap model. Set ALLOW_EXPENSIVE=1 if you really mean to pay for it." >&2
  exit 1
fi

# Trivial for the agent, non-empty for the ranker. The identifiers are real symbols at this
# task's start commit; keep them if you change the wording, or A1 injects nothing and the
# check reports an inconclusive result rather than a parity verdict.
BENCH_PROMPT="${BENCH_PROMPT:-Harness parity check, not a task: do not use any tools and do \
not modify any files. Reply with exactly the word ok. Context terms so the ranker has \
something to select: idempotency key length, Payment entity, PaymentService, PaymentValidator, \
payment migration schema.}"
export BENCH_PROMPT

if [ -s "$OUT/A0/usage.json" ] && [ -s "$OUT/A1/usage.json" ] && [ "${FORCE:-0}" != "1" ]; then
  echo "parity.sh: $OUT already holds both arms — asserting against them, spending nothing."
  echo "parity.sh: FORCE=1 to pay for two fresh runs."
else
  if [ "${REUSE:-0}" = "1" ]; then
    echo "parity.sh: REUSE=1 but $OUT does not hold both arms' usage.json" >&2
    exit 1
  fi
  echo "parity.sh: two paid runs on $MODEL — $TASK, arms A0 and A1, override prompt."
  rm -rf "$OUT"
  mkdir -p "$OUT"
  for arm in A0 A1; do
    MODEL="$MODEL" TIMEOUT_S="$TIMEOUT_S" \
      "$ROOT/scripts/eval/run.sh" "$TASK" "$arm" 0 "$OUT/$arm" >/dev/null
  done
fi

python3 - "$OUT" <<'PY'
import json
import pathlib
import re
import sys

out = pathlib.Path(sys.argv[1])
failures = []


def check(condition, message):
    if not condition:
        failures.append(message)
    return condition


def usage(arm):
    path = out / arm / "usage.json"
    if not path.exists():
        sys.exit(f"parity.sh: {path} missing — the run did not complete; nothing to compare")
    return json.loads(path.read_text())


a0, a1 = usage("A0"), usage("A1")

# --- 1. the injecting arm really injected ------------------------------------------------------
#
# The whole check is worthless if A1 was a second A0 wearing a hook config. This is not
# hypothetical: the product's own installed hook interpolates $CLAUDE_USER_PROMPT, which Claude
# Code does not set, so it injects an empty package on every turn while exiting 0. An A1 in that
# state is byte-identical to A0 and would sail through every field comparison below.

INJECTED = re.compile(r"^=== (\S+)(?: prompt=(\d+))? injected=(\d+) bytes$")
log = out / "A1/injected.log"
injected_bytes = 0
if check(log.exists(), "A1 wrote no injected.log — its hooks never ran"):
    records = [m.groups() for m in (INJECTED.match(l) for l in log.read_text().splitlines()) if m]
    check(records, "A1's injected.log records no injection at all")
    prompts = [r for r in records if r[0] == "UserPromptSubmit"]
    check(prompts, "A1 logged no UserPromptSubmit injection — the per-prompt hook never fired")
    for label, prompt_len, size in prompts:
        check(
            prompt_len is not None and int(prompt_len) > 0,
            "A1's hook saw a zero-length prompt: the hook fired but read nothing "
            "(the $CLAUDE_USER_PROMPT failure mode). Nothing about the arm was exercised.",
        )
        check(
            int(size) > 0,
            "A1 injected 0 bytes on a prompt: the hook is alive but the ranker selected "
            "nothing, so A1 was materially A0. INCONCLUSIVE, not a parity failure — give the "
            "prompt identifiers that exist at this task's start commit and re-run.",
        )
    injected_bytes = sum(int(r[2]) for r in records)

# A0 must be the bare arm it claims to be. If it injected too, the two arms are not the
# comparison this benchmark reports.
check(
    not (out / "A0/injected.log").exists(),
    "A0 wrote an injected.log — the bare arm is injecting, so the arms do not differ as claimed",
)

# --- 2. the two arms are described by the same record ------------------------------------------

check(
    set(a0) == set(a1),
    f"the arms' usage.json carry different fields: only in A0 {sorted(set(a0) - set(a1))}, "
    f"only in A1 {sorted(set(a1) - set(a0))}",
)

FIELDS = [
    "input_tokens", "output_tokens", "cache_read_tokens", "cache_creation_tokens",
    "total_cost_usd", "num_turns", "duration_api_ms",
]
for f in FIELDS:
    check(f in a0 and f in a1, f"{f} missing from one arm")
    check(
        isinstance(a0.get(f), (int, float)) and isinstance(a1.get(f), (int, float)),
        f"{f} is not numeric in both arms: A0={a0.get(f)!r} A1={a1.get(f)!r}",
    )

# --- 3. both records are of a run that actually happened ---------------------------------------
#
# A killed or crashed run writes an all-zero usage.json. Two all-zero records agree perfectly,
# which is the way this check would most easily pass for the wrong reason.

for arm, u in (("A0", a0), ("A1", a1)):
    check(u.get("claude_exit") == 0, f"{arm} exited {u.get('claude_exit')} — a killed run's "
                                     f"counters are all zero and compare equal to anything")
    for f in ("input_tokens", "output_tokens", "num_turns"):
        check(u.get(f, 0) > 0, f"{arm} reports {f}={u.get(f)}: that arm is not being measured")

# The dollar figure is a separate accounting source from the token counters (Task 9 reports a
# price-weighted CPS from it). Under subscription auth the API reports no per-call price and
# both arms read 0.00; one arm priced and the other not is the asymmetry that would matter.
check(
    (a0.get("total_cost_usd", 0) > 0) == (a1.get("total_cost_usd", 0) > 0),
    f"total_cost_usd is populated for one arm only: A0={a0.get('total_cost_usd')} "
    f"A1={a1.get('total_cost_usd')}",
)


def input_side(u):
    """Everything charged on the way in, however the API split it between cache and not."""
    return sum(u.get(f, 0) for f in
               ("input_tokens", "cache_creation_tokens", "cache_read_tokens"))


# --- 4. the difference is the package, not the accounting --------------------------------------
#
# Only meaningful while neither arm used tools: on a task where the agent works, its own
# choices move these counters by far more than the package does, and a difference in the right
# direction would prove nothing. Rather than gate on a comparison it cannot support, the check
# says so.

worked = [a for a, u in (("A0", a0), ("A1", a1)) if u.get("num_turns", 0) > 2]
if worked:
    failures.append(
        f"{', '.join(worked)} took more than two turns — the agent did work, so the "
        f"magnitude comparison below is contaminated by its choices rather than by the "
        f"injected package. INCONCLUSIVE on magnitude; the field parity above still holds."
    )
else:
    # A floor, not an estimate: English runs about 4 characters per token and the package is
    # denser than English (paths, camelCase identifiers), which tokenises to *more* tokens per
    # character, so 8 leaves roughly a factor of two of headroom below what is expected.
    floor = injected_bytes / 8
    delta = input_side(a1) - input_side(a0)
    check(
        delta >= floor,
        f"A1 injected {injected_bytes:,} bytes but its input-side total exceeds A0's by only "
        f"{delta:,} tokens (floor {floor:,.0f}). The injected package is not being charged to "
        f"A1 the way A0's prompt is charged to A0 — which is R-b.",
    )

# --- report ------------------------------------------------------------------------------------

print(f"{'':<22} {'A0':>12} {'A1':>12}")
for f in ["input_tokens", "cache_creation_tokens", "cache_read_tokens", "output_tokens",
          "num_turns", "total_cost_usd"]:
    print(f"{f:<22} {str(a0.get(f)):>12} {str(a1.get(f)):>12}")
print(f"{'input-side total':<22} {input_side(a0):>12,} {input_side(a1):>12,}")
print(f"{'injected bytes':<22} {0:>12,} {injected_bytes:>12,}")
print(f"{'input-side delta':<22} {'':>12} {input_side(a1) - input_side(a0):>12,}")
print()

if failures:
    print("PARITY CHECK FAILED")
    for f in failures:
        print(f"  - {f}")
    sys.exit(1)

print("parity ok — A1 injected a real package, A0 injected nothing, both arms populate every")
print("accounting field from the same record, and A1's extra input-side tokens are the package.")
PY
