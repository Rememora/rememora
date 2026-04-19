#!/usr/bin/env bash
# Rememora SessionEnd hook — closes the active session and runs a final curation pass.
# Runs once per Claude Code session end; pairs with stop-curate.sh's per-turn cooldown
# by guaranteeing the tail of the session is captured even if the last Stop hook was
# inside the cooldown window.

# Kill-switch: set REMEMORA_DISABLE_HOOKS=1 to disable all Rememora hooks.
[ -n "${REMEMORA_DISABLE_HOOKS:-}" ] && exit 0

set -euo pipefail

if ! command -v rememora &>/dev/null; then
  exit 0
fi

INPUT=$(cat)
CWD=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('cwd',''))" 2>/dev/null || echo "")
SESSION_ID=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('session_id',''))" 2>/dev/null || echo "")

rememora session end-active --auto-summary 2>/dev/null || true

if [ -n "$CWD" ] && [ -n "$SESSION_ID" ]; then
  ENCODED_CWD=$(echo "$CWD" | sed 's|/|-|g')
  JSONL_PATH="$HOME/.claude/projects/${ENCODED_CWD}/${SESSION_ID}.jsonl"
  PROJECT=$(basename "$CWD")

  if [ -f "$JSONL_PATH" ]; then
    (
      rememora curate --file "$JSONL_PATH" --project "$PROJECT" 2>/dev/null || true
    ) &
  fi

  LOCK_DIR="${TMPDIR:-/tmp}"
  rm -f "${LOCK_DIR%/}/rememora-curate-${SESSION_ID}.last" 2>/dev/null || true
fi
