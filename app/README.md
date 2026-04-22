# Rememora desktop viewer (v0)

A read-only Tauri app that renders the contents of your local Rememora
SQLite database (`~/.rememora/rememora.db`).

v0 scope is deliberately tiny — a plumbing PR, not a design PR. See
[`docs/spikes/83-desktop-viewer.md`](../docs/spikes/83-desktop-viewer.md)
for the longer-term design.

## What it does (v0)

- Launches as a native macOS app.
- Opens the encrypted DB using the key from `REMEMORA_KEY` or the OS
  keychain. **Never prompts** — the CLI is expected to have set up the
  keychain entry first (`rememora init`).
- Renders every non-superseded context (URI, name, abstract, category,
  timestamp), newest first, paginated at 200 rows per click.
- Surfaces a clear error message if the DB is missing, unencrypted, or
  the keychain key is not set.

What it **does not** do:

- No editing, supersession, or any other write path.
- No search (BM25 or vector).
- No session timeline, cost chart, or memory detail panel.
- No auto-updater, signing, or notarisation.
- No Linux or Windows targets.

## Prerequisites

- Rust toolchain (stable)
- Node 22+ and [pnpm](https://pnpm.io) 9+
- Xcode Command Line Tools (for linking on macOS)

## Install & run

```bash
# From repo root:
cd app
pnpm install

# Dev loop (hot reload frontend, Rust rebuilds on save):
pnpm tauri dev

# Release build (produces a .app bundle and .dmg):
pnpm tauri build
```

The built bundle is written to:

```
app/src-tauri/target/release/bundle/macos/Rememora.app
app/src-tauri/target/release/bundle/dmg/Rememora_0.1.0_<arch>.dmg
```

The builds are unsigned for v0. macOS Gatekeeper will warn on first launch;
right-click the app and pick "Open" to bypass.

## How it reads the database

The app calls `rememora::db::open_readonly_no_prompt(&default_db_path())`,
which:

1. Requires the DB to exist — otherwise returns `DbMissing`.
2. Requires the DB to be encrypted — otherwise returns `DbUnencrypted`.
3. Resolves the SQLCipher key via `rememora::crypto::resolve_key_no_prompt`
   (env → keychain), returning `KeychainMissing` if neither has it.
4. Applies `PRAGMA key` before any other statement.
5. Configures WAL + busy_timeout so concurrent CLI writes stay safe.
6. **Never runs migrations** — the viewer never mutates the DB schema.

## Troubleshooting

If the app shows "Encryption key not available", run:

```bash
rememora init
```

in a terminal. That CLI command stores the encryption key in the OS
keychain. Reopen the app and it will pick up the key automatically.
