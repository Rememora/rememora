#!/usr/bin/env bash
# Start the rememora-sandbox container.
#
# - Generates ~/.ssh/rememora-sandbox keypair (ed25519, no passphrase) if
#   missing.
# - Mounts named volumes for ~/.claude (Claude Code OAuth credentials) and
#   ~/.rememora (the encrypted SQLite DB) so they persist across container
#   restarts.
# - Mounts the host public key read-only at authorized_keys; this is the ONLY
#   host bind mount, deliberately, to keep host isolation.
set -euo pipefail

KEY_PATH="${HOME}/.ssh/rememora-sandbox"
PUB_PATH="${KEY_PATH}.pub"
CONTAINER_NAME="rememora-sandbox"
PORT="2222"

if ! command -v docker >/dev/null 2>&1; then
    echo "error: docker CLI not found on PATH" >&2
    exit 1
fi

if ! docker info >/dev/null 2>&1; then
    echo "error: docker daemon is not reachable" >&2
    exit 1
fi

# Generate key if missing
mkdir -p "${HOME}/.ssh"
chmod 700 "${HOME}/.ssh"
if [[ ! -f "${KEY_PATH}" ]]; then
    echo "[up] generating ${KEY_PATH} (ed25519, no passphrase)"
    ssh-keygen -t ed25519 -N "" -C "rememora-sandbox" -f "${KEY_PATH}"
fi
if [[ ! -f "${PUB_PATH}" ]]; then
    echo "error: public key ${PUB_PATH} missing after keygen" >&2
    exit 1
fi

# Stop and remove any existing container so this script is idempotent
if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "[up] removing existing ${CONTAINER_NAME} container"
    docker rm -f "${CONTAINER_NAME}" >/dev/null
fi

# Issue #106: the sandbox container regenerates its SSH host key whenever it
# is rebuilt or its named volume is wiped. A stale entry in
# ~/.ssh/known_hosts.rememora-sandbox would make the next `login.sh` abort
# with HOST KEY VERIFICATION FAILED. Always drop the file before booting so
# the first `ssh-keyscan` after startup wins fresh.
KNOWN_HOSTS="${HOME}/.ssh/known_hosts.rememora-sandbox"
if [[ -f "${KNOWN_HOSTS}" ]]; then
    rm -f "${KNOWN_HOSTS}"
    echo "[up] removed stale ${KNOWN_HOSTS}"
fi

echo "[up] starting ${CONTAINER_NAME} on port ${PORT}"
docker run -d \
    --name "${CONTAINER_NAME}" \
    -p "${PORT}:22" \
    -v rememora-sandbox-claude:/home/tester/.claude \
    -v rememora-sandbox-rememora:/home/tester/.rememora \
    -v "${PUB_PATH}:/home/tester/.ssh/authorized_keys:ro" \
    rememora-sandbox >/dev/null

# Fix ownership inside the container (named volumes mount as root by default)
docker exec "${CONTAINER_NAME}" chown -R tester:tester /home/tester/.claude /home/tester/.rememora

cat <<EOF
[up] container is running.

Next steps:
  1. SSH in and attach to the shared tmux session:
       ./docker/scripts/login.sh

  2. Inside the container, log in to Claude Code:
       claude
       /login
     Open the printed URL on your host browser, authorize, paste the auth
     code back into the SSH terminal. Token persists in the named volume.

  3. Wire the rememora plugin (sandbox-only, will not touch your host):
       rememora setup --apply

  4. Inject a command from the host without SSH-ing in:
       ./docker/scripts/exec.sh "rememora --version"

Tear down:
  ./docker/scripts/down.sh           # keep volumes (login + memories)
  ./docker/scripts/down.sh --purge   # also wipe volumes
EOF
