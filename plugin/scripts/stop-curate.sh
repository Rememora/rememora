#!/usr/bin/env bash
# Do not curate our own children. `rememora curate` spawns `claude -p`
# subprocesses for signal detection / AUDN curation; each child gets its own
# session_id and fires its own Stop hook. Without this gate, curate recursively
# curates itself.
[ -n "${REMEMORA_CURATE_CHILD:-}" ] && exit 0

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

# Issue #65 stage-2 dogfood bridge: if a monitors-based streaming curator is
# already processing this session's JSONL, don't double-curate. Removed in
# stage 4 when the Stop-hook curator is retired.
if pgrep -f "rememora curate --stream.*${SESSION_ID}" >/dev/null 2>&1; then
  exit 0
fi

# Two gates guard the per-turn Stop-hook stampede of `rememora curate` (and its
# `claude -p` signal-detector child).
#
# 1) Concurrency: at most one curate in-flight per session. Checked against the
#    kernel via pgrep — the jsonl path carries SESSION_ID, so the match is
#    unambiguous. This is the primary gate: it stays correct even when curate
#    runtime exceeds the frequency cooldown.
# 2) Frequency: at least COOLDOWN seconds between consecutive *finishes*. The
#    stamp is touched after curate returns, so the window means what its name
#    says (not start-to-start).
#
# Set REMEMORA_CURATE_COOLDOWN_SECS=0 to disable the frequency gate.
if pgrep -f "rememora curate --file .*${SESSION_ID}" >/dev/null 2>&1; then
  exit 0
fi

COOLDOWN="${REMEMORA_CURATE_COOLDOWN_SECS:-300}"
LOCK_DIR="${TMPDIR:-/tmp}"
STAMP="${LOCK_DIR%/}/rememora-curate-${SESSION_ID}.last"

if [ -f "$STAMP" ]; then
  # Portable mtime: BSD stat (macOS) first, GNU stat (Linux) fallback.
  LAST=$(stat -f %m "$STAMP" 2>/dev/null || stat -c %Y "$STAMP" 2>/dev/null || echo 0)
  NOW=$(date +%s)
  if [ $((NOW - LAST)) -lt "$COOLDOWN" ]; then
    exit 0
  fi
fi

# Fork curation to a fully detached background process.
#
# The Stop hook's stdout is a pipe Claude Code reads until EOF. Plain `&`
# backgrounds scheduling but does NOT close inherited file descriptors — the
# curate subprocess keeps the pipe's write-end open, so Claude Code waits for
# curate (and its `claude -p` children) to exit, blocking the next user turn
# for as long as curation runs. Observed: 10–97 min hangs in live sessions.
#
# Fix: redirect all three std streams on the outer launch AND place curate in
# its own session via setsid/nohup so no descendant holds the hook's pipe.
if command -v setsid >/dev/null 2>&1; then
  setsid bash -c '
    rememora curate --file "$1" --project "$2" >/dev/null 2>&1 || true
    touch "$3"
  ' _ "$JSONL_PATH" "$PROJECT" "$STAMP" </dev/null >/dev/null 2>&1 &
else
  # Stock macOS has no setsid; nohup + disown achieves equivalent detachment.
  nohup bash -c '
    rememora curate --file "$1" --project "$2" >/dev/null 2>&1 || true
    touch "$3"
  ' _ "$JSONL_PATH" "$PROJECT" "$STAMP" </dev/null >/dev/null 2>&1 &
  disown
fi

exit 0
