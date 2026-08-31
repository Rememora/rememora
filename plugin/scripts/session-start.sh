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

# Curator-child gate (issue #117). When `rememora curate` spawns `claude -p`
# for signal detection / AUDN curation, every hook in this directory runs
# against a transcript the curator already owns. Without an early-exit, each
# child claude would: create a spurious session row, prepend full project
# context to its closed-loop signal-detector prompt, and re-run FTS5 search
# on a transcript that's pure rememora plumbing. The Stop hook had this
# guard; the other three didn't, producing ~30 leaked sessions per real
# user turn. The whole hook chain is a no-op inside curator children.
[ -n "${REMEMORA_CURATE_CHILD:-}" ] && exit 0

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

# Consolidation gate. `--check-only` signals "gate met" with exit 42, which
# under the `set -e` above terminated this hook on the spot — so the `if` that
# followed was unreachable, the hook exited 42 on every session where the gate
# was met, and consolidation never ran once (`rememora status` still reports
# "Consolidation runs: 0"). Capture the status explicitly instead of letting
# `set -e` see a non-zero return.
CONSOLIDATE_GATE=0
rememora consolidate --check-only --project "$PROJECT" >/dev/null 2>&1 || CONSOLIDATE_GATE=$?

# The trigger stays disabled pending the clustering fix. `find_clusters`
# currently inverts BM25 (src/evolve.rs — a strong match scores LOWER than a
# stopword match), so every memory collapses into one cluster per category, and
# the merge path supersedes every member of a cluster on a single LLM decision
# with no way to undo it. Firing this automatically would risk the user's
# memory store. Re-enable by setting REMEMORA_AUTO_CONSOLIDATE=1 once clustering
# is fixed and the supersede path is bounded and reversible.
if [ "$CONSOLIDATE_GATE" -eq 42 ] && [ "${REMEMORA_AUTO_CONSOLIDATE:-0}" = "1" ]; then
  (rememora consolidate --project "$PROJECT" >/dev/null 2>&1 || true) &
fi

# Hooks must fail soft — never block or disrupt a coding session.
exit 0
