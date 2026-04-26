#!/usr/bin/env bash
# SSH into the sandbox and attach to the shared tmux session named
# "rememora". If the session is missing for any reason, create a new one.
set -euo pipefail

KEY_PATH="${HOME}/.ssh/rememora-sandbox"
PORT="2222"

if [[ ! -f "${KEY_PATH}" ]]; then
    echo "error: ${KEY_PATH} not found. Run ./docker/scripts/up.sh first." >&2
    exit 1
fi

exec ssh \
    -i "${KEY_PATH}" \
    -p "${PORT}" \
    -o StrictHostKeyChecking=accept-new \
    -o UserKnownHostsFile="${HOME}/.ssh/known_hosts.rememora-sandbox" \
    tester@localhost \
    -t 'tmux attach -t rememora || tmux new -s rememora'
