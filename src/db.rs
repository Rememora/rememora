use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

const MIGRATION_001: &str = include_str!("migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("migrations/002_embeddings.sql");

/// Register sqlite-vec extension before opening connections.
/// Must be called before any Connection::open calls.
#[cfg(feature = "embed-candle")]
fn register_vec_extension() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        unsafe { sqlite_vec::sqlite3_vec_init() };
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
    let mut path = dirs::home_dir().expect("Could not determine home directory");
    path.push(".rememora");
    path.push("rememora.db");
    path
}
