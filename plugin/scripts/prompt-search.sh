#!/usr/bin/env bash
# Rememora UserPromptSubmit hook — inject top-N FTS5 search hits into the
# prompt's additional-context channel.
#
# Design principles:
#   - Local-only: runs `rememora search --format context` against ~/.rememora/rememora.db
#   - Bounded: `--limit 3` and `--format context` enforce a ~2KB cap in the CLI
#   - Best-effort: any failure (rememora missing, DB locked, empty result) is silent

# Kill-switch — disables all Rememora hooks.
[ -n "${REMEMORA_DISABLE_HOOKS:-}" ] && exit 0

if ! command -v rememora >/dev/null 2>&1; then
  exit 0
fi

INPUT=$(cat 2>/dev/null || true)
[ -z "$INPUT" ] && exit 0

# Extract the prompt text and cwd from the hook payload. Claude Code passes
# top-level {session_id, transcript_path, cwd, prompt} on UserPromptSubmit.
PROMPT=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('prompt',''))" 2>/dev/null || echo "")
CWD=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('cwd',''))" 2>/dev/null || echo "")

# Strip FTS5-reserved punctuation. The query is passed unescaped to SQLite's
# FTS5 MATCH and chars like `?`, `:`, `(`, `)`, `"`, `*`, `-` carry query
# semantics that break on natural-language prompts. Collapse whitespace too.
PROMPT=$(printf '%s' "$PROMPT" | tr -d '?:"()*-' | tr -s '[:space:]' ' ' | sed 's/^ //; s/ $//')

# Skip on empty / overly-short prompts — FTS5 on 1–2 chars returns noise.
if [ "${#PROMPT}" -lt 6 ]; then
  exit 0
fi

PROJECT=""
if [ -n "$CWD" ]; then
  PROJECT=$(basename "$CWD")
fi

# Run with a short timeout — if the DB is locked or slow, we'd rather skip
# injection than delay the user's prompt.
if command -v timeout >/dev/null 2>&1; then
  RUN=(timeout 2 rememora)
else
  RUN=(rememora)
fi

if [ -n "$PROJECT" ]; then
  OUT=$("${RUN[@]}" search --project "$PROJECT" --limit 3 --format context "$PROMPT" 2>/dev/null || true)
else
  OUT=$("${RUN[@]}" search --limit 3 --format context "$PROMPT" 2>/dev/null || true)
fi

if [ -n "$OUT" ]; then
  echo "$OUT"
fi

exit 0
