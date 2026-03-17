use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

const MIGRATION_001: &str = include_str!("migrations/001_initial.sql");

pub fn open(path: &Path) -> Result<Connection> {
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

    let applied: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = '001_initial')",
        [],
        |row| row.get(0),
    )?;

    if !applied {
        conn.execute_batch(MIGRATION_001)?;
        conn.execute(
            "INSERT INTO _migrations (name, applied_at) VALUES ('001_initial', datetime('now'))",
            [],
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
