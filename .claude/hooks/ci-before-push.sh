#!/usr/bin/env bash
# PreToolUse(Bash) hook: before any `git push` or `gh pr create`, run `just ci`
# locally. Block the command on CI failure (exit 2) so we don't publish a red
# branch. Any other nonzero exit is non-blocking to Claude Code, so the guard
# fails OPEN on infrastructure errors (e.g. missing jq) — it is a convenience
# gate, not a security boundary; GitHub CI remains the real gate.
#
# Passthrough for every other Bash command: the coarse substring check on the
# raw input runs BEFORE jq, so the vast majority of commands exit here without
# spawning anything.
#
# Bypass for one call: prefix with `SKIP_CI_HOOK=1` (e.g. `SKIP_CI_HOOK=1 git push`).
#
# Known limitations (accepted): matching is substring-based, so exotic
# spellings (`git -C path push`, aliases) bypass the gate, and a command that
# merely quotes "git push" (e.g. `git log --grep 'git push'`) pays a
# redundant-but-safe `just ci` run.

set -eu

input=$(cat)

# Coarse prefilter on the raw JSON — no jq spawn for commands that can't match.
case "$input" in
  *"git push"* | *"gh pr create"*) ;;
  *) exit 0 ;;
esac

cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // ""')

# Escape hatch. Anchored to the documented "prefix it" usage (whole-command
# prefix, or prefixing the push segment of a compound command) so incidental
# mentions of the token — e.g. a commit message documenting the bypass — don't
# silently skip the gate.
case "$cmd" in
  "SKIP_CI_HOOK=1 "* | *"SKIP_CI_HOOK=1 git push"* | *"SKIP_CI_HOOK=1 gh pr create"*) exit 0 ;;
esac

# Only intercept push / PR-creation. Everything else passes straight through.
case "$cmd" in
  *"git push"* | *"gh pr create"*) ;;
  *) exit 0 ;;
esac

# Run `just ci` against the tree the command actually targets, not the hook
# process's own cwd: start from the tool's working directory (the `cwd` field
# of the hook input — where the Bash tool runs the command) and honor a
# leading `cd <path> &&` in the command itself. The agent-worktree pattern
# `cd .claude/worktrees/X && git push` pushes X's branch, so X — not the main
# checkout — is the tree that must be validated.
tool_cwd=$(printf '%s' "$input" | jq -r '.cwd // ""')
[ -d "$tool_cwd" ] || tool_cwd=$(pwd)

run_dir=$tool_cwd
case "$cmd" in
  "cd "*)
    lead_cd=$(printf '%s' "$cmd" | sed -n 's/^cd[[:space:]]\{1,\}\([^;&|]*\).*/\1/p')
    # Trim surrounding whitespace and simple quoting.
    lead_cd=$(printf '%s' "$lead_cd" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e "s/^'\(.*\)'\$/\1/" -e 's/^"\(.*\)"$/\1/')
    if [ -n "$lead_cd" ] && (cd "$tool_cwd" && cd "$lead_cd") >/dev/null 2>&1; then
      run_dir=$(cd "$tool_cwd" && cd "$lead_cd" && pwd)
    fi
    ;;
esac

repo_root=$(git -C "$run_dir" rev-parse --show-toplevel 2>/dev/null || printf '%s' "$run_dir")

if output=$(cd "$repo_root" && just ci </dev/null 2>&1); then
  exit 0
fi

{
  printf '❌ just ci failed in %s; refusing to run: %s\n' "$repo_root" "$cmd"
  printf '\n--- last 25 lines of CI output ---\n'
  printf '%s\n' "$output" | tail -25
  printf '\nTo bypass for one call: prefix with SKIP_CI_HOOK=1\n'
} >&2
exit 2
