#!/usr/bin/env bash
# Rememora SessionEnd hook — closes the active session.
# Runs automatically when the Claude Code session ends.

set -euo pipefail

if ! command -v rememora &>/dev/null; then
  exit 0
fi

rememora session end-active --auto-summary 2>/dev/null || true
