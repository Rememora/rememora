use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

const MIGRATION_001: &str = include_str!("migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("migrations/002_embeddings.sql");
const MIGRATION_003: &str = include_str!("migrations/003_curator.sql");
const MIGRATION_004: &str = include_str!("migrations/004_agent_invocations.sql");

/// Register sqlite-vec extension before opening connections.
/// Must be called before any Connection::open calls.
///
/// The transmute follows the sqlite-vec crate's own test pattern — there is no
/// safe wrapper. See: https://docs.rs/sqlite-vec/latest/sqlite_vec/
#[cfg(feature = "embed-candle")]
fn register_vec_extension() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        // Clippy wants a type annotation but the target type is an opaque
        // sqlite3 callback. This is the canonical pattern from the sqlite-vec
        // crate's own tests — suppress the lint.
        #[allow(clippy::missing_transmute_annotations)]
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

pub fn open(path: &Path) -> Result<Connection> {
    open_with_options(path, false)
}

pub fn open_with_options(path: &Path, no_encryption: bool) -> Result<Connection> {
    #[cfg(feature = "embed-candle")]
    register_vec_extension();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let conn = Connection::open(path)?;

    // PRAGMA key must be the very first statement on a SQLCipher connection
    if !no_encryption {
        apply_encryption_key(&conn, path)?;
    }

    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_memory() -> Result<Connection> {
    #[cfg(feature = "embed-candle")]
    register_vec_extension();

    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA cache_size = -64000;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )?;

    let applied_001: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = '001_initial')",
        [],
        |row| row.get(0),
    )?;

    if !applied_001 {
        conn.execute_batch(MIGRATION_001)?;
        conn.execute(
            "INSERT INTO _migrations (name, applied_at) VALUES ('001_initial', datetime('now'))",
            [],
        )?;
    }

    let applied_002: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = '002_embeddings')",
        [],
        |row| row.get(0),
    )?;

    if !applied_002 {
        conn.execute_batch(MIGRATION_002)?;
        conn.execute(
            "INSERT INTO _migrations (name, applied_at) VALUES ('002_embeddings', datetime('now'))",
            [],
        )?;
    }

    let applied_003: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = '003_curator')",
        [],
        |row| row.get(0),
    )?;

    if !applied_003 {
        conn.execute_batch(MIGRATION_003)?;
        conn.execute(
            "INSERT INTO _migrations (name, applied_at) VALUES ('003_curator', datetime('now'))",
            [],
        )?;
    }

    let applied_004: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = '004_agent_invocations')",
        [],
        |row| row.get(0),
    )?;

    if !applied_004 {
        conn.execute_batch(MIGRATION_004)?;
        conn.execute(
            "INSERT INTO _migrations (name, applied_at) VALUES ('004_agent_invocations', datetime('now'))",
            [],
        )?;
    }

    // Create sqlite-vec virtual table when feature is enabled
    #[cfg(feature = "embed-candle")]
    {
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_contexts USING vec0(
                context_id TEXT PRIMARY KEY,
                embedding float[384] distance_metric=cosine
            );",
        )?;
    }

    Ok(())
}

/// Apply encryption key to a SQLCipher connection.
/// - Encrypted DB: key is required (env/keychain/prompt)
/// - Existing unencrypted DB: left as-is (user must run `rememora encrypt`)
/// - New DB with key available: encrypted from the start
fn apply_encryption_key(conn: &Connection, path: &Path) -> Result<()> {
    use crate::crypto;

    let encrypted = crypto::is_db_encrypted(path);
    let exists = path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false);

    if encrypted {
        // DB is encrypted — key is required
        let key = crypto::resolve_key(true)?
            .with_context(|| "Encryption key required but not available")?;
        conn.pragma_update(None, "key", &key)?;
    } else if exists {
        // DB exists and is unencrypted — don't apply a key.
        // User must run `rememora encrypt` to migrate.
    } else {
        // New DB — encrypt if a key is available (env/keychain), don't prompt
        if let Some(key) = crypto::resolve_key(false)? {
            conn.pragma_update(None, "key", &key)?;
        }
    }

    Ok(())
}

/// Error cases for `open_readonly_no_prompt`, designed for GUI callers that
/// need to render a precise, actionable message rather than a generic failure.
#[derive(Debug)]
pub enum OpenReadonlyError {
    /// The DB file does not exist at `path`.
    DbMissing,
    /// The DB file exists but is plain SQLite (no cipher applied). Viewer v0
    /// expects the CLI to have set up an encrypted DB.
    DbUnencrypted,
    /// The DB is encrypted but no key is available in env or keychain.
    /// The user needs to run the CLI first (`rememora init` flow) to populate
    /// the keychain entry.
    KeychainMissing,
    /// Any other failure (SQLite open failure, bad cipher, IO, etc.).
    Other(anyhow::Error),
}

impl std::fmt::Display for OpenReadonlyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenReadonlyError::DbMissing => write!(
                f,
                "Rememora database not found at the expected path. \
                 Run `rememora init` in a terminal first."
            ),
            OpenReadonlyError::DbUnencrypted => write!(
                f,
                "Rememora database is present but unencrypted. \
                 Run `rememora encrypt` in a terminal to migrate it."
            ),
            OpenReadonlyError::KeychainMissing => write!(
                f,
                "Rememora database is encrypted but no key was found in \
                 REMEMORA_KEY or the OS keychain. Run `rememora init` in a \
                 terminal first so the desktop app can decrypt the database."
            ),
            OpenReadonlyError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OpenReadonlyError {}

/// Open the DB for **read-only, non-interactive** access.
///
/// Contract vs `open`:
/// - Never prompts on stdin — safe to call from a GUI/non-terminal process.
/// - Requires the DB to already exist; returns `DbMissing` otherwise.
/// - Requires the DB to already be encrypted; `DbUnencrypted` otherwise.
///   (A viewer should not silently migrate the user's DB.)
/// - Requires the key to be available via env or keychain; `KeychainMissing`
///   otherwise.
/// - Does **not** run migrations — a viewer must not mutate the DB schema,
///   even transiently.
/// - Applies the same runtime PRAGMAs (`journal_mode=WAL`, `foreign_keys`,
///   `busy_timeout`, `cache_size`, `synchronous=NORMAL`) so concurrent
///   reads against the CLI's WAL are well-behaved.
pub fn open_readonly_no_prompt(path: &Path) -> Result<Connection, OpenReadonlyError> {
    use crate::crypto;

    #[cfg(feature = "embed-candle")]
    register_vec_extension();

    if !path.exists() {
        return Err(OpenReadonlyError::DbMissing);
    }

    let encrypted = crypto::is_db_encrypted(path);
    if !encrypted {
        return Err(OpenReadonlyError::DbUnencrypted);
    }

    let key = crypto::resolve_key_no_prompt()
        .map_err(OpenReadonlyError::Other)?
        .ok_or(OpenReadonlyError::KeychainMissing)?;

    let conn = Connection::open(path).map_err(|e| OpenReadonlyError::Other(e.into()))?;
    // `pragma_update` for "key" must be the very first statement.
    conn.pragma_update(None, "key", &key)
        .map_err(|e| OpenReadonlyError::Other(e.into()))?;

    configure(&conn).map_err(OpenReadonlyError::Other)?;
    // Intentionally skip migrate(): viewer is read-only.
    Ok(conn)
}

pub fn default_db_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("REMEMORA_DB") {
        return std::path::PathBuf::from(p);
    }
    let mut path = dirs::home_dir().expect("Could not determine home directory");
    path.push(".rememora");
    path.push("rememora.db");
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_readonly_returns_db_missing_for_nonexistent_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.db");
        let err = open_readonly_no_prompt(&path).unwrap_err();
        assert!(
            matches!(err, OpenReadonlyError::DbMissing),
            "expected DbMissing, got {err:?}"
        );
    }

    #[test]
    fn open_readonly_returns_db_unencrypted_for_plain_db() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plain.db");
        // Create a plain (unencrypted) SQLite DB using `open_with_options`.
        let conn = open_with_options(&path, true).expect("create plain db");
        drop(conn);

        let err = open_readonly_no_prompt(&path).unwrap_err();
        assert!(
            matches!(err, OpenReadonlyError::DbUnencrypted),
            "expected DbUnencrypted, got {err:?}"
        );
    }
}
