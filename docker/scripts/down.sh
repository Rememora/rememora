#!/usr/bin/env bash
# Stop and remove the rememora-sandbox container.
#
# Default: keeps named volumes (rememora-sandbox-claude, rememora-sandbox-rememora)
# so the next `up.sh` retains the Claude Code login and rememora memories.
#
# --purge   also remove the named volumes (interactive confirm unless --yes)
# --yes     skip the confirmation prompt for --purge
set -euo pipefail

CONTAINER_NAME="rememora-sandbox"
VOLUMES=("rememora-sandbox-claude" "rememora-sandbox-rememora")

PURGE=0
ASSUME_YES=0
for arg in "$@"; do
    case "${arg}" in
        --purge) PURGE=1 ;;
        --yes|-y) ASSUME_YES=1 ;;
        *) echo "unknown flag: ${arg}" >&2; exit 2 ;;
    esac
done

if ! command -v docker >/dev/null 2>&1; then
    echo "error: docker CLI not found on PATH" >&2
    exit 1
fi

if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "[down] removing container ${CONTAINER_NAME}"
    docker rm -f "${CONTAINER_NAME}" >/dev/null
else
    echo "[down] container ${CONTAINER_NAME} not present"
fi

if [[ "${PURGE}" -eq 1 ]]; then
    if [[ "${ASSUME_YES}" -ne 1 ]]; then
        echo "[down] --purge will delete the following volumes:"
        for v in "${VOLUMES[@]}"; do echo "  - ${v}"; done
        echo "[down] this wipes Claude Code login and all rememora memories in the sandbox."
        read -r -p "Continue? [y/N] " reply
        if [[ ! "${reply}" =~ ^[Yy]$ ]]; then
            echo "[down] purge cancelled"
            exit 0
        fi
    fi
    for v in "${VOLUMES[@]}"; do
        if docker volume ls --format '{{.Name}}' | grep -q "^${v}$"; then
            docker volume rm "${v}" >/dev/null
            echo "[down] removed volume ${v}"
        else
            echo "[down] volume ${v} not present"
        fi
    done

    # Issue #106: the sandbox container regenerates its SSH host key on
    # every fresh boot. Stale entries in ~/.ssh/known_hosts.rememora-sandbox
    # cause `ssh` to abort with HOST KEY VERIFICATION FAILED on the next
    # `up.sh + login.sh` cycle. Drop the file so the next login starts clean.
    KNOWN_HOSTS="${HOME}/.ssh/known_hosts.rememora-sandbox"
    if [[ -f "${KNOWN_HOSTS}" ]]; then
        rm -f "${KNOWN_HOSTS}"
        echo "[down] removed stale ${KNOWN_HOSTS}"
    fi
fi

echo "[down] done."
