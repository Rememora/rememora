#!/usr/bin/env bash
# Inject a command into the sandbox's shared tmux session via send-keys.
#
# This is the affordance for driving the sandbox from the host without
# SSH-ing in: the user (or another agent) types
#
#     ./docker/scripts/exec.sh "rememora --version"
#
# and the keystrokes land in the same tmux session the user is attached to,
# so they see live output in their terminal.
set -euo pipefail

CONTAINER_NAME="rememora-sandbox"

if [[ $# -lt 1 ]]; then
    echo "usage: $0 <command...>" >&2
    echo "example: $0 rememora --version" >&2
    exit 2
fi

if ! docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "error: container ${CONTAINER_NAME} is not running. Run ./docker/scripts/up.sh first." >&2
    exit 1
fi

# Send-keys to the shared "rememora" tmux session, then press Enter.
docker exec "${CONTAINER_NAME}" \
    sudo -u tester -H tmux send-keys -t rememora "$*" Enter
