#!/usr/bin/env bash
# Rememora curate monitor — long-lived streaming curator.
#
# Claude Code runs this as a `monitors` entry: a background process that
# streams a session's JSONL through `rememora curate --stream`. The stream
# subcommand gates + curates transcript deltas incrementally, writing one
# rate-limited line per notification to stdout. Claude Code surfaces those
# lines as per-session notifications.
#
# Stage 1 is **opt-in**: this script exits immediately unless
# `REMEMORA_USE_MONITOR=1`. The Stop-hook curator remains the default.

# --- Gates ------------------------------------------------------------------

# Opt-in only during Stage 1 rollout.
[ "${REMEMORA_USE_MONITOR:-0}" = "1" ] || exit 0

# Never run inside a curator-spawned `claude -p` child — the curator tags
# those with REMEMORA_CURATE_CHILD=1 specifically so background processes
# don't recurse on their own transcripts.
[ -n "${REMEMORA_CURATE_CHILD:-}" ] && exit 0

if ! command -v rememora >/dev/null 2>&1; then
  exit 0
fi
if ! command -v tail >/dev/null 2>&1; then
  exit 0
fi

# --- Session resolution -----------------------------------------------------

# Claude Code's monitor env is still shaking out. Prefer the explicit vars
# when set, fall back to $PWD + newest-JSONL probe.
CWD="${CLAUDE_CWD:-$PWD}"
SESSION_ID="${CLAUDE_SESSION_ID:-}"
ENCODED_CWD=$(printf '%s' "$CWD" | sed 's|/|-|g')
PROJECT_DIR="$HOME/.claude/projects/${ENCODED_CWD}"

if [ -n "$SESSION_ID" ] && [ -f "$PROJECT_DIR/${SESSION_ID}.jsonl" ]; then
  JSONL_PATH="$PROJECT_DIR/${SESSION_ID}.jsonl"
else
  # Fallback: newest JSONL in the encoded-cwd dir by mtime.
  JSONL_PATH=$(ls -t "$PROJECT_DIR"/*.jsonl 2>/dev/null | head -n 1)
fi

if [ -z "${JSONL_PATH:-}" ] || [ ! -f "$JSONL_PATH" ]; then
  echo "rememora monitor: no session JSONL at $PROJECT_DIR — exiting"
  exit 0
fi

PROJECT=$(basename "$CWD")
SESSION_ARG=()
if [ -n "$SESSION_ID" ]; then
  SESSION_ARG=(--session "$SESSION_ID")
fi

# Stage-2 metrics: if REMEMORA_NOTIFY_LOG is set, pass it through so the
# stream appends per-flush outcomes. Empty-string disables.
NOTIFY_LOG_ARG=()
if [ -n "${REMEMORA_NOTIFY_LOG:-}" ]; then
  NOTIFY_LOG_ARG=(--notify-log "$REMEMORA_NOTIFY_LOG")
fi

# --- Supervise --------------------------------------------------------------

# Bounded restart loop: if the pipeline crashes 5 times within 60 s, back off
# for 60 s before retrying. Prevents a broken env from spamming restarts.
restart_count=0
window_start=$(date +%s)

while true; do
  # `tail -F -n 0` starts at EOF and follows appends across rotations.
  # SIGPIPE from Claude Code closing the monitor propagates to tail.
  tail -F -n 0 "$JSONL_PATH" \
    | rememora curate --stream --project "$PROJECT" \
        "${SESSION_ARG[@]}" "${NOTIFY_LOG_ARG[@]}"

  status=$?
  # Claude Code closing stdin (SIGPIPE / 141) is a clean exit for us.
  if [ "$status" -eq 141 ] || [ "$status" -eq 0 ]; then
    exit 0
  fi

  now=$(date +%s)
  if [ $((now - window_start)) -gt 60 ]; then
    window_start=$now
    restart_count=0
  fi
  restart_count=$((restart_count + 1))
  if [ "$restart_count" -ge 5 ]; then
    echo "rememora monitor: 5 restarts in 60 s — backing off 60 s"
    sleep 60
    restart_count=0
    window_start=$(date +%s)
  else
    sleep 5
  fi
done
