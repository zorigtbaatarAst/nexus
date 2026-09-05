#!/usr/bin/env bash
# Claude Code hook helper for the benchmark arms. Two jobs.
#
# 1. Get the prompt. It arrives as JSON on stdin, not in the environment. `$CLAUDE_USER_PROMPT`
#    — which `nexus hooks` installs and which this benchmark's plan copied — is not set by
#    Claude Code 2.1.261, so a hook built on it runs, succeeds, and injects an empty package on
#    every turn. From the outside that is indistinguishable from a healthy run.
#
# 2. Record what was injected, every turn, a zero-byte package loudest of all. Without this,
#    "A1 was no better than A0" has two explanations — the ranker selected nothing, or the hook
#    was dead — and nothing in the run directory tells them apart afterwards.
#
#   nexus-hook --session <args…>   -> nexus context --session <args…>
#   nexus-hook <args…>             -> nexus context --task <the prompt> <args…>
#
# The mode is a flag rather than the stdin `hook_event_name`, so a SessionStart package does not
# depend on an assumption about a payload this script has never seen.
set -uo pipefail

LOG=/bench/injected.log

# record <label> <package>: log it if the harness is listening, then inject it either way.
record() {
  if [ -d "$(dirname "$LOG")" ]; then
    {
      printf '=== %s injected=%s bytes\n' "$1" "$(printf '%s' "$2" | wc -c)"
      printf '%s\n' "$2"
    } >> "$LOG" 2>/dev/null || true
  fi
  printf '%s' "$2"
}

if [ "${1:-}" = "--session" ]; then
  shift
  record SessionStart "$(nexus context --session "$@" 2>/dev/null)"
  exit 0
fi

PROMPT="$(node -e '
  let s = "";
  process.stdin.on("data", d => s += d).on("end", () => {
    try { process.stdout.write(JSON.parse(s).prompt || ""); } catch (e) { }
  });
')"

# The prompt length is logged next to the package size because the two failures look the same
# from a byte count alone: prompt=0 means the payload changed shape, prompt=93 injected=0 means
# the ranker selected nothing.
record "UserPromptSubmit prompt=${#PROMPT}" "$(nexus context --task "$PROMPT" "$@" 2>/dev/null)"
