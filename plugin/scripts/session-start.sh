#!/usr/bin/env bash
# Rememora SessionStart hook — loads project context and starts a session.
# Runs automatically on every Claude Code session start/resume.

set -euo pipefail

# Check if rememora is available
if ! command -v rememora &>/dev/null; then
  exit 0
fi

# Load project context (auto-detects from CWD)
CONTEXT=$(rememora context --auto 2>/dev/null || true)

if [ -n "$CONTEXT" ]; then
  # Output goes to Claude as additionalContext
  echo "$CONTEXT"
  echo ""
  echo "---"
  echo "The above is your project memory from rememora. Use it to inform your work."
  echo "Remember to save new decisions, bug fixes, and patterns as you work."
fi

# Start a session (captures session ID for later)
rememora session start --agent claude-code --project "$(basename "$PWD")" --intent "Interactive session" 2>/dev/null || true
