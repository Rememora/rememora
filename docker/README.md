# Rememora Docker Sandbox

Isolated test environment for the full rememora plugin chain (CLI + curator
hooks + telemetry) with **zero leakage** onto your host filesystem,
keychain, or `~/.claude/` settings.

## Why this exists

Exercising the rememora Claude Code plugin end-to-end requires a real
Claude Code login and a real rememora DB. Running that on your host means
mixing experimental plugin behavior with your everyday config and
keychain. This sandbox solves that with three layers of isolation:

1. **Container filesystem** — the only thing it shares with the host is
   the SSH public key (read-only). No bind mounts of `~`, `~/.claude`,
   `~/.rememora`, or anything else.
2. **Named Docker volumes** — `~/.claude` and `~/.rememora` inside the
   container live in `rememora-sandbox-claude` and `rememora-sandbox-rememora`
   volumes. They persist across container restarts but never appear on
   your host filesystem.
3. **No `libsecret-1-0`** — the package is deliberately omitted, so
   Claude Code falls back to file-based credential storage inside its
   volume. Your host Keychain / Secret Service is never touched.

Auth is pure OAuth done by you inside the container; no
`ANTHROPIC_API_KEY` is ever baked into the image or passed at runtime.

## Prerequisites

- Docker Desktop, Colima, or OrbStack running
- ~5 GB free disk for the image (initial build is ~5–10 min — it compiles
  the rememora release binary)
- A free local TCP port `2222` for SSH

## Workflow

### 1. Build the image

```bash
./docker/scripts/build.sh
```

Default platform is `linux/arm64` (Apple Silicon). For x86_64 hosts:

```bash
./docker/scripts/build.sh --amd64
```

### 2. Start the container

```bash
./docker/scripts/up.sh
```

This will:
- Generate `~/.ssh/rememora-sandbox` (ed25519, no passphrase) if missing
- Start the container with named volumes for `~/.claude` and `~/.rememora`
- Mount your public key read-only at the tester user's `authorized_keys`

### 3. SSH in (and attach to the shared tmux session)

```bash
./docker/scripts/login.sh
```

You land directly inside `tmux` session named `rememora`. Detach with
`Ctrl-b d`; re-attach by running `login.sh` again.

### 4. Log in to Claude Code

Inside the container:

```bash
claude
```

Then in the REPL:

```
/login
```

Follow the printed URL on your **host browser**, authorize, copy the
auth code, paste it back into the SSH terminal. The OAuth token persists
in the `rememora-sandbox-claude` named volume across container restarts.

### 5. Wire up the rememora plugin

```bash
rememora setup --apply
```

This writes only to `/home/tester/.claude/settings.json` inside the
sandbox. Your host's `~/.claude/settings.json` is untouched.

You're now in a real Claude Code + rememora install, fully isolated, ready
to exercise plugin hooks, curator stop-hook, telemetry, OTEL export — the
whole chain.

### 6. Inject commands from your host without SSH-ing in

```bash
./docker/scripts/exec.sh "rememora --version"
./docker/scripts/exec.sh "rememora search 'docker' --project rememora"
```

`exec.sh` send-keys into the **same** tmux session you (or anyone else)
are already attached to via `login.sh`. This is the affordance for an
outside agent to drive the sandbox while a human watches live.

### 7. Tear down

Keep the volumes (Claude Code login + memories survive next `up.sh`):

```bash
./docker/scripts/down.sh
```

Wipe everything (interactive confirm):

```bash
./docker/scripts/down.sh --purge
./docker/scripts/down.sh --purge --yes   # no prompt
```

## Architecture override

```bash
./docker/scripts/build.sh --amd64
```

Use this on x86_64 hosts, or to test the amd64 image on Apple Silicon
(Rosetta will run it, slowly).

## Troubleshooting

**`ssh: connect to host localhost port 2222: Connection refused`**
- Docker daemon not running, or container not up. Run `docker ps` and
  re-run `./docker/scripts/up.sh` if `rememora-sandbox` is missing.
- Port 2222 already in use. Stop the conflicting service or edit the port
  in `up.sh` and `login.sh`.

**`login.sh` lands you in a fresh shell, not tmux**
- The tmux session may not have started in time on first boot. Type
  `tmux attach -t rememora` manually, or just `tmux new -s rememora` to
  create it. The entrypoint will not recreate it on subsequent reattaches.

**Claude Code keeps prompting for login on every restart**
- Check whether `libsecret-1-0` was added to the Dockerfile. It must NOT
  be installed; otherwise Claude Code will try to use the host's Secret
  Service via D-Bus and fail silently each time.
- Verify the volume mounted: `docker inspect rememora-sandbox | jq '.[0].Mounts'`.
  You should see `rememora-sandbox-claude` mounted at `/home/tester/.claude`.

**`exec.sh` sends keys but I see no output**
- You're not attached to the tmux session. Run `login.sh` in another
  terminal and you'll see the keystrokes appear live.

**`build.sh` fails with `cargo` permission errors**
- Probably running on a Docker Desktop with low memory. Bump VM memory to
  ≥ 6 GB; the rusqlite bundled-sqlcipher build is heavy.

## What's inside the image

- Debian bookworm-slim
- `openssh-server`, `tmux`, `git`, `jq`, `nodejs`, `npm`, `curl`,
  `ca-certificates`, `sqlite3`, `sudo`
- `@anthropic-ai/claude-code` (latest from npm at build time)
- `/usr/local/bin/rememora` — the release binary built from this repo
- Non-root `tester` user (uid 1000), pubkey-only SSH

## What's NOT inside the image

- `libsecret-1-0` (deliberately — see "Why this exists" above)
- Any host filesystem bind mounts other than the read-only SSH public key
- Any pre-baked API keys or tokens
