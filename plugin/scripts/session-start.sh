#!/usr/bin/env bash
# Rememora SessionStart hook — loads project context and starts a session.
# Runs on every Claude Code session start / clear / compact / resume.
#
# Claude Code passes a `source` field on stdin JSON indicating which variant
# triggered the hook. We branch on it:
#   startup (default) → inject full context, start a new session row
#   resume            → skip `session start` (existing row continues); still inject context
#   clear             → user asked for a clean slate: skip context injection entirely
#   compact           → inject a compact cheatsheet rather than full L0/L1 to
#                       avoid token pressure right after compaction (falls back
#                       to full context if --cheatsheet is unavailable)
#
# When `source` is missing (older Claude Code versions) we default to the
# startup path — current behavior is preserved.

# Kill-switch: set REMEMORA_DISABLE_HOOKS=1 to disable all Rememora hooks.
[ -n "${REMEMORA_DISABLE_HOOKS:-}" ] && exit 0

set -euo pipefail

# Check if rememora is available
if ! command -v rememora &>/dev/null; then
  exit 0
fi

# Capture stdin (may be empty on older Claude Code versions).
INPUT=$(cat 2>/dev/null || true)
SOURCE=""
if [ -n "$INPUT" ]; then
  SOURCE=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('source',''))" 2>/dev/null || echo "")
fi

PROJECT=$(basename "$PWD")

# Context injection — skip entirely on `clear` (user wants a clean slate).
if [ "$SOURCE" != "clear" ]; then
  if [ "$SOURCE" = "compact" ]; then
    CONTEXT=$(rememora context --auto --cheatsheet 2>/dev/null || rememora context --auto 2>/dev/null || true)
  else
    CONTEXT=$(rememora context --auto 2>/dev/null || true)
  fi

  if [ -n "${CONTEXT:-}" ]; then
    echo "$CONTEXT"
    echo ""
    echo "---"
    echo "The above is your project memory from rememora. Use it to inform your work."
    echo "Remember to save new decisions, bug fixes, and patterns as you work."
  fi
fi

# Session row — only create a new one on genuine startup. Resuming continues
# the existing row; clear/compact stay within the current session.
if [ -z "$SOURCE" ] || [ "$SOURCE" = "startup" ]; then
  rememora session start --agent claude-code --project "$PROJECT" --intent "Interactive session" 2>/dev/null || true
fi

# Consolidation gate runs on every source — cheap (exit-code only) and worth
# catching on resume/compact too.
rememora consolidate --check-only --project "$PROJECT" 2>/dev/null
if [ $? -eq 42 ]; then
  (rememora consolidate --project "$PROJECT" 2>/dev/null || true) &
fi
