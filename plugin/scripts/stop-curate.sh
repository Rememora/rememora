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

# Fork curation to background — must not block the agent
(
  rememora curate --file "$JSONL_PATH" --project "$PROJECT" 2>/dev/null || true
) &

exit 0
