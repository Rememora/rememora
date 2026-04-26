#!/usr/bin/env bash
# Build the rememora-sandbox image.
#
# Default platform: linux/arm64 (Apple Silicon). Pass --amd64 to flip to
# linux/amd64 for x86_64 hosts.
set -euo pipefail

PLATFORM="linux/arm64"
if [[ "${1:-}" == "--amd64" ]]; then
    PLATFORM="linux/amd64"
fi

# Resolve repo root (this script lives at <repo>/docker/scripts/build.sh)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

if ! command -v docker >/dev/null 2>&1; then
    echo "error: docker CLI not found on PATH" >&2
    exit 1
fi

if ! docker info >/dev/null 2>&1; then
    echo "error: docker daemon is not reachable. Start Docker Desktop / Colima / OrbStack and retry." >&2
    exit 1
fi

echo "[build] platform=${PLATFORM}"
echo "[build] context=${REPO_ROOT}"
echo "[build] this can take 5-10 min on first run (cargo release build)"

cd "${REPO_ROOT}"
docker build \
    --platform "${PLATFORM}" \
    -t rememora-sandbox \
    -f docker/Dockerfile \
    .

echo "[build] done. Next: ./docker/scripts/up.sh"
