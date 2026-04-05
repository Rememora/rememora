use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

const MIGRATION_001: &str = include_str!("migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("migrations/002_embeddings.sql");
const MIGRATION_003: &str = include_str!("migrations/003_curator.sql");

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
    #[cfg(feature = "embed-candle")]
    register_vec_extension();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let conn = Connection::open(path)?;
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

pub fn default_db_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("REMEMORA_DB") {
        return std::path::PathBuf::from(p);
    }
    let mut path = dirs::home_dir().expect("Could not determine home directory");
    path.push(".rememora");
    path.push("rememora.db");
    path
}
