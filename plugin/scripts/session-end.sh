#!/usr/bin/env bash
# Rememora SessionEnd hook — closes the active session and runs a final curation pass.
# Runs once per Claude Code session end; pairs with stop-curate.sh's per-turn cooldown
# by guaranteeing the tail of the session is captured even if the last Stop hook was
# inside the cooldown window.

# Kill-switch: set REMEMORA_DISABLE_HOOKS=1 to disable all Rememora hooks.
[ -n "${REMEMORA_DISABLE_HOOKS:-}" ] && exit 0

# Curator-child gate (issue #117). See session-start.sh for the reasoning;
# we must not call `session end-active` from inside a curator child or it
# will close the user's most-recent active session in the middle of their
# real claude run.
[ -n "${REMEMORA_CURATE_CHILD:-}" ] && exit 0

set -euo pipefail

if ! command -v rememora &>/dev/null; then
  exit 0
fi

INPUT=$(cat)
CWD=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('cwd',''))" 2>/dev/null || echo "")
SESSION_ID=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('session_id',''))" 2>/dev/null || echo "")

# Pass the same project name session-start.sh used (basename of cwd). Without
# this, `end-active` falls back to `detect_from_cwd` which only resolves
# *registered* projects — running `claude -p` from `/tmp/<scratch>` (CI,
# agent-loop, ad-hoc) would silently no-op and leak active session rows
# forever (issue #114). The CLI also gained a basename-fallback so older
# hooks keep working, but this explicit form is the canonical one.
PROJECT_FOR_END=""
if [ -n "$CWD" ]; then
  PROJECT_FOR_END=$(basename "$CWD")
fi

if [ -n "$PROJECT_FOR_END" ]; then
  rememora session end-active --project "$PROJECT_FOR_END" --auto-summary 2>/dev/null || true
else
  rememora session end-active --auto-summary 2>/dev/null || true
fi

if [ -n "$CWD" ] && [ -n "$SESSION_ID" ]; then
  ENCODED_CWD=$(echo "$CWD" | sed 's|/|-|g')
  JSONL_PATH="$HOME/.claude/projects/${ENCODED_CWD}/${SESSION_ID}.jsonl"
  PROJECT=$(basename "$CWD")

  if [ -f "$JSONL_PATH" ]; then
    # Fully detach the final-pass curate. Plain `&` backgrounds scheduling
    # but does NOT close inherited file descriptors — a curate subprocess
    # that lingers past SessionEnd could keep the hook's pipe write-end
    # open and cause the same FD-leak blocking we fixed in stop-curate.sh.
    # Apply the same setsid (Linux) / nohup + disown (macOS) detachment and
    # redirect all three std streams on the outer launch.
    if command -v setsid >/dev/null 2>&1; then
      setsid bash -c '
        rememora curate --file "$1" --project "$2" >/dev/null 2>&1 || true
      ' _ "$JSONL_PATH" "$PROJECT" </dev/null >/dev/null 2>&1 &
    else
      nohup bash -c '
        rememora curate --file "$1" --project "$2" >/dev/null 2>&1 || true
      ' _ "$JSONL_PATH" "$PROJECT" </dev/null >/dev/null 2>&1 &
      disown
    fi
  fi

  LOCK_DIR="${TMPDIR:-/tmp}"
  rm -f "${LOCK_DIR%/}/rememora-curate-${SESSION_ID}.last" 2>/dev/null || true
fi
