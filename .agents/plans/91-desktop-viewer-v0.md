# Desktop Viewer v0 (Issue #91)

**Ticket**: https://github.com/Rememora/rememora/issues/91
**Branch**: `feat/91-desktop-viewer-v0`

## Summary

Ship a minimal Tauri macOS desktop app (`app/` folder) that launches, reads the
encrypted `~/.rememora/rememora.db` via the CLI's keychain-stored SQLCipher key,
and renders a read-only list of contexts (URI / title / abstract / category /
timestamp). No search, no write path, no multi-platform. Plumbing PR.

## Locked scoping decisions (from user, 2026-04-21)

1. Stack: **Tauri** (hard no on Electron).
2. SQLCipher key: reuse `rememora::crypto` keychain lookup, **no prompt**,
   clear error if keychain entry missing.
3. Repo layout: monorepo `app/` folder.
4. Platform: **macOS only** (`aarch64-apple-darwin`, `x86_64-apple-darwin`).
5. Deliverable: buildable macOS `.app` via `pnpm tauri build`. No CI gate in v0.

## Pre-existing work context

`desktop/` already exists in the repo (PRs #53 and #57) with a Tauri v2
scaffold, richer Rust command surface, and a vanilla-JS frontend wired to mock
data. It uses `rememora::db::open()` which calls `crypto::resolve_key(true)`
and will interactively prompt from stdin in a GUI — violating locked decision
#2. The locked decision says `app/`, so v0 lives in a fresh `app/` folder;
removing `desktop/` is a separate cleanup ticket. PR description will flag it.

## Implementation steps

### 1. Expose a keychain-only key accessor in `src/crypto.rs`

Factor the existing env+keychain logic from `resolve_key` into a shared helper
so both the prompt and non-prompt callers share code. Add:

```rust
pub fn resolve_key_no_prompt() -> Result<Option<String>>
```

Returns `Ok(Some(..))` if `REMEMORA_KEY` env var or keychain has a key,
`Ok(None)` otherwise. Never prompts.

### 2. Add a non-prompting read-only DB opener in `src/db.rs`

`db::open` calls `crypto::resolve_key(true)` which blocks on stdin — fatal in a
GUI. Add:

```rust
pub fn open_readonly_no_prompt(path: &Path) -> Result<Connection>
```

- Requires the DB to already exist (returns a typed error if missing).
- If encrypted, resolves the key via `crypto::resolve_key_no_prompt`; if no
  key is available, returns a clear error.
- Applies `PRAGMA key` before any other statement.
- Applies the same `configure()` PRAGMAs (WAL/foreign_keys/busy_timeout/cache/synchronous).
- **Skips** `migrate()` — read-only viewer must never mutate the DB.

### 3. Scaffold `app/` (Tauri v2, React+Vite+TS, pnpm)

Layout:

```
app/
  package.json
  pnpm-lock.yaml
  index.html
  vite.config.ts
  tsconfig.json
  .gitignore
  README.md
  src/
    main.tsx
    App.tsx
    types.ts
    styles.css
  src-tauri/
    Cargo.toml
    build.rs
    tauri.conf.json
    capabilities/default.json
    icons/icon.png
    src/
      main.rs
      commands.rs
```

Frontend: React 18 + Vite 6 + TypeScript + `@tauri-apps/api` v2. No UI library.

`tauri.conf.json`:
- `productName: "Rememora"`
- `identifier: "ai.rememora.app"`
- `bundle.targets: ["app", "dmg"]`
- `bundle.macOS.minimumSystemVersion: "10.15"`
- Window 960x640, title "Rememora".
- Build targets limited to `aarch64-apple-darwin` and `x86_64-apple-darwin`.

### 4. `app/src-tauri/Cargo.toml`

- `tauri = { version = "2", features = [] }`
- `tauri-build = { version = "2", features = [] }` (build-deps)
- `serde` / `serde_json` / `anyhow`
- `rusqlite = { version = "0.34", features = ["bundled-sqlcipher"] }`  (same as root)
- `rememora = { path = "../.." }`

### 5. Tauri Rust backend

`main.rs`:
- Open DB via `rememora::db::open_readonly_no_prompt(&default_db_path())`.
- On `DB missing`, `DB present but unencrypted`, or `keychain key missing`,
  launch the app with the connection in an `Err` state so the UI can render a
  targeted error screen.
- `.manage(DbState)` and register commands.

`commands.rs` v0 surface (minimal):

```rust
#[tauri::command]
pub fn list_contexts(
    state: tauri::State<'_, DbState>,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<ListContextsResponse, String>

#[tauri::command]
pub fn get_db_status(state: tauri::State<'_, DbState>) -> DbStatus
```

```rust
pub struct ListContextsResponse { rows: Vec<ContextRow>, total: i64 }
pub struct ContextRow {
    id: String, uri: String, name: String,
    abstract_text: String, category: Option<String>, created_at: String,
}
pub enum DbStatus { Ok, DbMissing, DbUnencrypted, KeychainMissing, Other(String) }
```

SQL: `SELECT id, uri, name, abstract, category, created_at FROM contexts
WHERE superseded_by IS NULL ORDER BY created_at DESC LIMIT ? OFFSET ?;`
plus a separate `SELECT COUNT(*)` for the total.

Defaults: offset=0, limit=200.

### 6. Frontend (`app/src/`)

- `App.tsx` — on mount, call `get_db_status`. If `Ok`, call `list_contexts`
  and render a table: `Category | URI | Name | Abstract (truncated) | Created`.
  Pagination: "Load more" button incrementing offset by 200.
- Error states render a targeted message (e.g. "Rememora keychain entry not
  found. Run `rememora init` in a terminal first.").
- Minimal `styles.css` with `prefers-color-scheme` dark mode.

### 7. `app/README.md`

Document:
- Prereqs (Rust, Node 22+, pnpm 9+, Xcode CLT).
- `cd app && pnpm install`
- `pnpm tauri dev` / `pnpm tauri build`
- macOS-only v0, unsigned local builds expected.
- Requires keychain entry set by the CLI (`rememora init`).

### 8. `.gitignore`

Add `app/` excludes to root `.gitignore` (or an `app/.gitignore`):
- `app/node_modules`
- `app/dist`
- `app/src-tauri/target`
- `app/src-tauri/gen/schemas`

## Testing strategy

- `cargo test` at repo root must pass (touches `src/crypto.rs`, `src/db.rs`).
- `cargo clippy --all-targets` must pass at repo root and inside `app/src-tauri`.
- New unit tests:
  - `src/crypto.rs` — `resolve_key_no_prompt` returns `None` when env+keychain
    empty; returns `Some(..)` for a `REMEMORA_KEY` env override.
  - `src/db.rs` — `open_readonly_no_prompt` opens an existing unencrypted
    temp DB without error and does not run migrations.
- Manual verification captured in PR description: `pnpm tauri dev` launches
  the app against the real `~/.rememora/rememora.db`. Screenshot + terminal
  output included.

## Out of scope (explicit)

- Search UI (BM25 or vector).
- Write/edit/supersede.
- Session timeline, cost chart, memory detail panel.
- Auth/passphrase UI or onboarding.
- Multi-profile / multi-DB.
- Auto-updater, signing, notarisation.
- Linux/Windows build targets.
- Retiring or migrating the existing `desktop/` scaffold.

## Risks

- **SQLCipher version interop**: pin `rusqlite 0.34` + `bundled-sqlcipher` in
  both root and `app/src-tauri`.
- **`keyring` from a GUI**: macOS Security framework works for GUI apps — will
  verify manually.
