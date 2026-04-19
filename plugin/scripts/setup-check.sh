#!/usr/bin/env bash
# Rememora Setup hook — one-shot check that the rememora CLI is available.
#
# We deliberately DO NOT install anything here. Compare claude-mem's
# smart-install.js, which installs Bun + Python uv + node deps from a Setup
# hook and generates a long tail of install-failure reports (#1662, #1503,
# #1883 in their tracker). Our binary ships via Homebrew; a hook should
# check-and-inform, never install.

# Kill-switch: set REMEMORA_DISABLE_HOOKS=1 to disable all Rememora hooks.
[ -n "${REMEMORA_DISABLE_HOOKS:-}" ] && exit 0

if ! command -v rememora >/dev/null 2>&1; then
  echo "Rememora plugin installed but 'rememora' CLI not found on PATH."
  echo "Install with: brew install Rememora/tap/rememora  (or: cargo install rememora)"
fi

# Always succeed — a missing binary is informational, not an error.
exit 0
