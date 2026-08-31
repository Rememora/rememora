#!/usr/bin/env bash
# Rememora UserPromptSubmit hook — inject top-N FTS5 search hits into the
# prompt's additional-context channel.
#
# Design principles:
#   - Local-only: runs `rememora search --format context` against ~/.rememora/rememora.db
#   - Bounded: `--limit 3` and `--format context` enforce a ~2KB cap in the CLI
#   - Best-effort: any failure (rememora missing, DB locked, empty result) is silent

# Kill-switch — disables all Rememora hooks.
[ -n "${REMEMORA_DISABLE_HOOKS:-}" ] && exit 0

# Curator-child gate (issue #117). The signal-detector / AUDN curator prompts
# are closed-loop rememora plumbing. Prepending FTS5 hits to them just bloats
# the prompt with content the curator never asked for and cannot use.
[ -n "${REMEMORA_CURATE_CHILD:-}" ] && exit 0

if ! command -v rememora >/dev/null 2>&1; then
  exit 0
fi

INPUT=$(cat 2>/dev/null || true)
[ -z "$INPUT" ] && exit 0

# Extract the prompt text and cwd from the hook payload. Claude Code passes
# top-level {session_id, transcript_path, cwd, prompt} on UserPromptSubmit.
PROMPT=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('prompt',''))" 2>/dev/null || echo "")
CWD=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('cwd',''))" 2>/dev/null || echo "")

# Strip FTS5-reserved punctuation. The query is passed unescaped to SQLite's
# FTS5 MATCH and chars like `?`, `:`, `(`, `)`, `"`, `*`, `-` carry query
# semantics that break on natural-language prompts. Collapse whitespace too.
PROMPT=$(printf '%s' "$PROMPT" | tr -d '?:"()*-' | tr -s '[:space:]' ' ' | sed 's/^ //; s/ $//')

# Skip on empty / overly-short prompts — FTS5 on 1–2 chars returns noise.
if [ "${#PROMPT}" -lt 6 ]; then
  exit 0
fi

# Project resolution is the CLI's job — pass the raw cwd and let
# `project::resolve_for_cwd` map it (including git worktrees, which is where
# agent work happens). This script used to send `basename "$CWD"`, which in a
# worktree produced a name matching no registered project. Because the project
# filter is a hard URI prefix match, that silently excluded every project
# memory and injected only global noise.
ARGS=(search --limit 3 --format context)
if [ -n "$CWD" ]; then
  ARGS+=(--cwd "$CWD")
fi

# Bound the call — we'd rather skip injection than delay the user's prompt.
# `timeout` is GNU coreutils and is NOT present on stock macOS, the primary
# platform, so fall back to a background process we reap ourselves. Without
# this the hook is unbounded on the prompt-submit critical path.
run_bounded() {
  if command -v timeout >/dev/null 2>&1; then
    timeout 2 rememora "$@"
    return $?
  fi

  local out_file rc pid waited
  out_file=$(mktemp -t rememora-recall) || return 1
  rememora "$@" >"$out_file" 2>/dev/null &
  pid=$!

  waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge 20 ]; then
      kill -TERM "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      rm -f "$out_file"
      return 124
    fi
    sleep 0.1
    waited=$((waited + 1))
  done

  wait "$pid" 2>/dev/null
  rc=$?
  cat "$out_file"
  rm -f "$out_file"
  return $rc
}

OUT=$(run_bounded "${ARGS[@]}" "$PROMPT" 2>/dev/null || true)

if [ -n "$OUT" ]; then
  echo "$OUT"
fi

exit 0
