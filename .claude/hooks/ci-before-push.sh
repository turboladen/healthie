#!/usr/bin/env bash
# PreToolUse(Bash) hook: before any `git push` or `gh pr create`, run `just ci`
# locally. Block the command on CI failure so we don't publish a red branch.
#
# Passthrough for every other Bash command (the case-esac at the top exits 0
# immediately, no CI spawn).
#
# Bypass for one call: prefix with `SKIP_CI_HOOK=1` (e.g. `SKIP_CI_HOOK=1 git push`).

set -eu

input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // ""')

# Escape hatch.
case "$cmd" in
  *"SKIP_CI_HOOK=1"*) exit 0 ;;
esac

# Only intercept push / PR-creation. Everything else passes straight through.
case "$cmd" in
  *"git push"* | *"gh pr create"*) ;;
  *) exit 0 ;;
esac

repo_root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)

if output=$(cd "$repo_root" && just ci </dev/null 2>&1); then
  exit 0
fi

{
  printf '❌ just ci failed; refusing to run: %s\n' "$cmd"
  printf '\n--- last 25 lines of CI output ---\n'
  printf '%s\n' "$output" | tail -25
  printf '\nTo bypass for one call: prefix with SKIP_CI_HOOK=1\n'
} >&2
exit 2
