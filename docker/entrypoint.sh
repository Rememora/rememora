#!/usr/bin/env bash
# Entrypoint: ensure a detached tmux session named "rememora" exists for the
# tester user, then run sshd in the foreground.
#
# The shared tmux session is the affordance the human + outside `exec.sh`
# both attach to: the user SSHes in and `tmux attach`, while host-side
# scripts use `docker exec ... tmux send-keys` to inject commands into the
# same session.

set -euo pipefail

# Generate host keys if missing (first boot)
ssh-keygen -A >/dev/null 2>&1 || true

# The host-mounted public key lives at /home/tester/.ssh/authorized_keys
# (read-only, root-owned). sshd's StrictModes would reject it. Copy it
# into the sandbox-managed AuthorizedKeysFile path with safe perms.
SRC_KEY="/home/tester/.ssh/authorized_keys"
DST_KEY="/etc/ssh/authorized_keys/tester"
if [[ -f "${SRC_KEY}" ]]; then
    install -m 0644 -o root -g root "${SRC_KEY}" "${DST_KEY}"
fi

# Issue #110: silence "Claude configuration file not found at: ~/.claude.json"
# noise that `claude -p` prints 3+ times before responding inside the sandbox.
# Cosmetic but loud. Seed an empty JSON object owned by the tester user. If
# the user later runs `/login` and Claude Code populates the real file, this
# placeholder is overwritten on first real write — idempotent.
CLAUDE_CONFIG="/home/tester/.claude.json"
if [[ ! -f "${CLAUDE_CONFIG}" ]]; then
    echo '{}' > "${CLAUDE_CONFIG}"
    chown tester:tester "${CLAUDE_CONFIG}"
    chmod 0644 "${CLAUDE_CONFIG}"
fi

# Start a detached tmux session as the tester user. Idempotent: re-running
# is a no-op if the session already exists.
sudo -u tester -H bash -c '
    cd /home/tester
    if ! tmux has-session -t rememora 2>/dev/null; then
        tmux new-session -d -s rememora -x 220 -y 50
        tmux send-keys -t rememora "echo \"[rememora-sandbox] tmux session ready. rememora $(rememora --version 2>/dev/null || echo \"not found\")\"" Enter
    fi
'

# sshd in foreground (PID 1 for clean container shutdown)
exec /usr/sbin/sshd -D -e
