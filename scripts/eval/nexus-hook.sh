#!/usr/bin/env bash
# UserPromptSubmit hook: run `nexus context --task <the prompt>` and print the package, which
# Claude Code adds to the turn.
#
# The prompt arrives as JSON on stdin, not in the environment. `$CLAUDE_USER_PROMPT` — which
# `nexus hooks` installs and which this benchmark's plan copied — is not set by Claude Code
# 2.1.261, so a hook built on it runs, succeeds, and injects an empty package on every turn.
# That failure is invisible from the outside: the run completes, the tokens look plausible,
# and the arm under test is silently identical to the bare agent. Hence this file.
#
# Everything after the first argument is passed through to `nexus context`.
set -uo pipefail

PROMPT="$(node -e '
  let s = "";
  process.stdin.on("data", d => s += d).on("end", () => {
    try { process.stdout.write(JSON.parse(s).prompt || ""); } catch (e) { }
  });
')"

# Fail open, exactly as the hook string in the arm configuration would: a context package that
# cannot be produced must never block the turn.
[ -n "$PROMPT" ] || exit 0

exec nexus context --task "$PROMPT" "$@"
