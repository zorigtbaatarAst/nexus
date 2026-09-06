#!/bin/bash
# Measure Nexus against ADR-024's budgets on a real repository.
#
# ADR-024 ships hooks off by default until "measured p95 stays under budget across
# several real projects". The budgets, from its table:
#   SessionStart      800 tok / 400 ms
#   UserPromptSubmit 4000 tok / 150 ms
#   PostToolUse         0 tok / 200 ms
#
# Everything here is free: no model, no network after the clone.
set -uo pipefail

NEXUS="${NEXUS:-$(git rev-parse --show-toplevel)/target/release/nexus}"
REPO="${1:?usage: NAMED_PROMPT=... SYMPTOM_PROMPT=... measure.sh <repo-path> [label]}"
LABEL="${2:-$(basename "$REPO")}"
REPS="${REPS:-10}"

: "${NAMED_PROMPT:?set NAMED_PROMPT to a prompt naming a symbol that exists in this repo}"
: "${SYMPTOM_PROMPT:?set SYMPTOM_PROMPT to a prompt describing a symptom, naming no symbol}"

# A cold scan needs an empty index, so this script deletes $REPO/.nexus. That directory
# holds the memory layer — facts and findings a previous session worked out, which no
# rescan can rebuild. Measure on a throwaway clone. Deleting a real one costs real work,
# so an existing index has to be surrendered by name rather than by running the script.
if [ -e "$REPO/.nexus" ] && [ "${WIPE:-}" != "yes" ]; then
  echo "refusing: $REPO/.nexus exists and would be deleted for the cold scan." >&2
  echo "  measure a throwaway clone, or re-run with WIPE=yes to discard that index." >&2
  exit 1
fi

# p50/p95 of a command's wall time in ms, over REPS runs after one warm-up.
percentiles() {
  local desc="$1"; shift
  "$@" >/dev/null 2>&1                       # warm-up, not counted
  local times=()
  for _ in $(seq "$REPS"); do
    local t0 t1
    t0=$(date +%s%N)
    "$@" >/dev/null 2>&1
    t1=$(date +%s%N)
    times+=( $(( (t1 - t0) / 1000000 )) )
  done
  printf '%s\n' "${times[@]}" | sort -n | python3 -c "
import sys
v=[int(x) for x in sys.stdin.read().split()]
n=len(v)
p50=v[n//2]
p95=v[min(n-1, int(round(0.95*(n-1))))]
print(f'$desc\t{p50}\t{p95}\t{v[0]}\t{v[-1]}')
"
}

echo "=== $LABEL ==="
echo "files: $(find "$REPO" -type f -not -path '*/.git/*' -not -path '*/target/*' -not -path '*/.nexus/*' | wc -l)"

rm -rf "${REPO:?}/.nexus"
t0=$(date +%s%N)
"$NEXUS" --project "$REPO" scan >/dev/null 2>&1
t1=$(date +%s%N)
echo "cold scan: $(( (t1 - t0) / 1000000 )) ms"
echo "index size: $(du -k "$REPO/.nexus/nexus.db" 2>/dev/null | cut -f1) KB"
echo "symbols: $("$NEXUS" --project "$REPO" scan --json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); r=d.get("result",d); print(r.get("symbols","?"))' 2>/dev/null || echo '?')"
echo

printf 'measurement\tp50\tp95\tmin\tmax\n'
percentiles "SessionStart (budget 400ms)" \
  "$NEXUS" --project "$REPO" context --session --budget 800
percentiles "PostToolUse rescan (budget 200ms)" \
  "$NEXUS" --project "$REPO" rescan --quiet
percentiles "UserPrompt, names a symbol (budget 150ms)" \
  "$NEXUS" --project "$REPO" context --task "$NAMED_PROMPT" --budget 4000 --brief
percentiles "UserPrompt, symptom words (budget 150ms)" \
  "$NEXUS" --project "$REPO" context --task "$SYMPTOM_PROMPT" --budget 4000 --brief
percentiles "UserPrompt, forced lexical (budget 150ms)" \
  "$NEXUS" --project "$REPO" context --task "$SYMPTOM_PROMPT" --budget 4000 --brief --rank lexical
echo

echo "--- what the two prompts actually returned"
for p in "$NAMED_PROMPT" "$SYMPTOM_PROMPT"; do
  "$NEXUS" --project "$REPO" context --task "$p" --budget 4000 --json 2>/dev/null | python3 -c "
import json,sys
d=json.load(sys.stdin); r=d.get('result',d)
items=r.get('items',[])
sel=r.get('basis',{}).get('selection','?')
kind='lexical fallback' if 'no symbol anchored' in sel else ('engine' if items else 'EMPTY')
why=items[0].get('why','')[:28] if items else '-'
print(f'  {kind:16} items={len(items):3d} tokens={r.get(\"tokens_estimated\",0):5d}  top={why}')
"
done
