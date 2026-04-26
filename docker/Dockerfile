# syntax=docker/dockerfile:1.6
#
# Rememora isolated test sandbox.
#
# Two stages:
#   1. builder — compiles `rememora` (release) from the repo source.
#   2. runtime — debian:bookworm-slim with sshd + tmux + Claude Code CLI.
#
# Notable choices:
#   - libsecret-1-0 is DELIBERATELY NOT installed. This forces Claude Code
#     to fall back to file-based credential storage inside the named volume
#     (~/.claude), so OAuth tokens never touch any host keychain.
#   - SSH is pubkey-only. The host mounts its public key read-only at
#     /home/tester/.ssh/authorized_keys via `up.sh`.
#   - Default platform is linux/arm64; pass `--platform linux/amd64` (or use
#     `build.sh --amd64`) for x86_64 hosts.

# ---------- Stage 1: builder ----------
FROM rust:1.89-bookworm AS builder

WORKDIR /build

# System deps for the bundled-sqlcipher build
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        build-essential \
    && rm -rf /var/lib/apt/lists/*

# Copy the entire workspace. .dockerignore excludes target/, .git/, node_modules,
# etc. so the build context stays small.
COPY . .

# Build the release binary
RUN cargo build --release --bin rememora

# ---------- Stage 2: runtime ----------
FROM debian:bookworm-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive

# Runtime tooling. NOTE: libsecret-1-0 is intentionally OMITTED so Claude Code
# uses file-based credential storage in the user's $HOME (which is a named
# volume, not a host bind).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        jq \
        nodejs \
        npm \
        openssh-server \
        sqlite3 \
        sudo \
        tmux \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code globally
RUN npm install -g @anthropic-ai/claude-code \
    && npm cache clean --force

# Copy the rememora binary from the builder stage
COPY --from=builder /build/target/release/rememora /usr/local/bin/rememora
RUN chmod +x /usr/local/bin/rememora

# Create non-root tester user. Set the shadow password to "*" (locked but
# not -, which would prevent pubkey login on Debian's sshd defaults).
RUN useradd --create-home --shell /bin/bash --uid 1000 tester \
    && usermod -p '*' tester \
    && mkdir -p /home/tester/.ssh /home/tester/.claude /home/tester/.rememora \
    && chown -R tester:tester /home/tester \
    && chmod 700 /home/tester/.ssh

# SSH server config: pubkey only, no passwords, no root login.
# AuthorizedKeysFile points at a sandbox-managed path that the entrypoint
# populates from the host-mounted public key (so we sidestep StrictModes
# checks against a root-owned bind-mounted authorized_keys).
RUN mkdir -p /var/run/sshd /etc/ssh/authorized_keys \
    && sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config \
    && sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin no/' /etc/ssh/sshd_config \
    && sed -i 's/^#\?PubkeyAuthentication.*/PubkeyAuthentication yes/' /etc/ssh/sshd_config \
    && sed -i 's/^#\?ChallengeResponseAuthentication.*/ChallengeResponseAuthentication no/' /etc/ssh/sshd_config \
    && sed -i 's/^#\?UsePAM.*/UsePAM no/' /etc/ssh/sshd_config \
    && sed -i 's|^#\?AuthorizedKeysFile.*|AuthorizedKeysFile /etc/ssh/authorized_keys/%u|' /etc/ssh/sshd_config \
    && echo 'PermitUserEnvironment yes' >> /etc/ssh/sshd_config

# Entrypoint that starts a detached tmux session, then sshd in the foreground
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

EXPOSE 22

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
