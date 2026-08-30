#!/usr/bin/env bash
# Build a retrieval-eval corpus from a *scratch copy* of a real Rememora DB.
#
# The corpus contains real memory text and is therefore .gitignore'd — it never
# lands in the repo. Only `golden.jsonl` (queries + memory ids) is committed.
#
# Usage:
#   bench/golden/build-corpus.sh [out_dir]
#
# Then:
#   REMEMORA_EVAL_CORPUS=<out_dir>/corpus.jsonl \
#   REMEMORA_EVAL_GOLDEN=bench/golden/golden.jsonl \
#     cargo test --test eval_retrieval -- --nocapture
#
# Why a scratch copy and not ~/.rememora/rememora.db directly:
#   `rememora search` calls context::bump_active_count on every hit
#   (src/search.rs:208). Running an eval against the live DB would mutate the
#   user's real hotness state hundreds of times per run.

set -euo pipefail

OUT="${1:-${TMPDIR:-/tmp}/rememora-eval}"
SRC_DB="${REMEMORA_SOURCE_DB:-$HOME/.rememora/rememora.db}"
SRC_KEY="$(dirname "$SRC_DB")/key"

mkdir -p "$OUT"
cp "$SRC_DB" "$OUT/rememora.db"

# crypto::default_key_file_path() (src/crypto.rs:16) derives <dirname REMEMORA_DB>/key,
# so copying the key file beside the scratch DB gives a zero-keychain, zero-prompt open.
if [ -f "$SRC_KEY" ]; then
  cp "$SRC_KEY" "$OUT/key"
  chmod 600 "$OUT/key"
fi

REMEMORA_DB="$OUT/rememora.db" rememora export --format json \
  | python3 -c '
import json, sys
rows = json.load(sys.stdin)
keep = ["id","uri","parent_uri","category","name","abstract","overview",
        "content","tags","source_agent","importance"]
n = 0
for r in rows:
    if r.get("context_type") != "memory":
        continue
    print(json.dumps({k: r.get(k) for k in keep}))
    n += 1
print(f"corpus rows: {n}", file=sys.stderr)
' > "$OUT/corpus.jsonl"

echo "corpus -> $OUT/corpus.jsonl"
echo "scratch db -> $OUT/rememora.db  (safe to delete)"
