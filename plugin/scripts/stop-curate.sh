#!/usr/bin/env bash
# Rememora Stop hook — curates memories from the current session transcript.
# Runs in the background after each Claude Code agent turn completes.
# Must never block the agent — all work is forked to a subshell.

set -euo pipefail

# Check if rememora is available
if ! command -v rememora &>/dev/null; then
  exit 0
fi

# Read hook input from stdin (JSON with session_id, cwd, etc.)
INPUT=$(cat)

CWD=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('cwd',''))" 2>/dev/null || echo "")
SESSION_ID=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('session_id',''))" 2>/dev/null || echo "")

if [ -z "$CWD" ] || [ -z "$SESSION_ID" ]; then
  exit 0
fi

# Encode CWD to match Claude Code's project directory naming
# /Users/user/Projects/myproject → -Users-user-Projects-myproject
ENCODED_CWD=$(echo "$CWD" | sed 's|/|-|g')

JSONL_PATH="$HOME/.claude/projects/${ENCODED_CWD}/${SESSION_ID}.jsonl"

if [ ! -f "$JSONL_PATH" ]; then
  exit 0
fi

# Detect project name from CWD
PROJECT=$(basename "$CWD")

# Debounce: skip if this session curated within the cooldown window.
# Claude Code's Stop hook fires per agent turn, not per session — without this gate,
# long sessions stampede `rememora curate` (and its `claude -p` signal-detector child)
# dozens of times concurrently. Per-session lockfile = fresh bucket each new session,
# auto-cleaned from /tmp on reboot. Set REMEMORA_CURATE_COOLDOWN_SECS=0 to disable.
COOLDOWN="${REMEMORA_CURATE_COOLDOWN_SECS:-300}"
LOCK_DIR="${TMPDIR:-/tmp}"
LOCK="${LOCK_DIR%/}/rememora-curate-${SESSION_ID}.last"

if [ -f "$LOCK" ]; then
  # Portable mtime: BSD stat (macOS) first, GNU stat (Linux) fallback.
  LAST=$(stat -f %m "$LOCK" 2>/dev/null || stat -c %Y "$LOCK" 2>/dev/null || echo 0)
  NOW=$(date +%s)
  if [ $((NOW - LAST)) -lt "$COOLDOWN" ]; then
    exit 0
  fi
fi
touch "$LOCK"

# Fork curation to background — must not block the agent
(
  rememora curate --file "$JSONL_PATH" --project "$PROJECT" 2>/dev/null || true
) &

exit 0
