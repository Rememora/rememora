//! Tauri commands backing the desktop viewer.
//!
//! v0 surface is intentionally tiny:
//!   * `get_db_status` — tells the UI whether the DB opened, and if not, why.
//!   * `list_contexts` — paginated read of non-superseded contexts.
//!
//! No write paths. No search. That is deliberate and locked for v0.

use std::sync::Mutex;

use rememora::db::{self, OpenReadonlyError};
use rusqlite::Connection;
use serde::Serialize;

/// What the app knows about the DB after attempting to open it at startup.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "tag", content = "message")]
pub enum DbStatus {
    Ok,
    DbMissing,
    DbUnencrypted,
    KeychainMissing,
    Other(String),
}

impl From<&OpenReadonlyError> for DbStatus {
    fn from(e: &OpenReadonlyError) -> Self {
        match e {
            OpenReadonlyError::DbMissing => DbStatus::DbMissing,
            OpenReadonlyError::DbUnencrypted => DbStatus::DbUnencrypted,
            OpenReadonlyError::KeychainMissing => DbStatus::KeychainMissing,
            OpenReadonlyError::Other(err) => DbStatus::Other(err.to_string()),
        }
    }
}

/// App-wide managed state. The connection lives for the lifetime of the
/// process; a viewer only needs a single reader.
pub struct AppState {
    status: DbStatus,
    conn: Option<Mutex<Connection>>,
}

impl AppState {
    /// Try to open `~/.rememora/rememora.db` read-only and capture the result.
    ///
    /// Called once at startup. Never panics on expected failure modes
    /// (missing DB, missing keychain key, unencrypted DB) — those become
    /// `DbStatus` variants the UI surfaces.
    pub fn initialise() -> Self {
        let path = db::default_db_path();
        match db::open_readonly_no_prompt(&path) {
            Ok(conn) => AppState {
                status: DbStatus::Ok,
                conn: Some(Mutex::new(conn)),
            },
            Err(e) => {
                // Log once so Tauri dev console shows the failure cause.
                eprintln!("rememora-app: DB open failed: {e}");
                AppState {
                    status: DbStatus::from(&e),
                    conn: None,
                }
            }
        }
    }
}

/// Row shape returned by `list_contexts`. Mirrors `ContextRow` in `src/types.ts`.
#[derive(Debug, Serialize)]
pub struct ContextRow {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub abstract_text: String,
    pub category: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ListContextsResponse {
    pub rows: Vec<ContextRow>,
    pub total: i64,
}

#[tauri::command]
pub fn get_db_status(state: tauri::State<'_, AppState>) -> DbStatus {
    state.status.clone()
}

#[tauri::command]
pub fn list_contexts(
    state: tauri::State<'_, AppState>,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<ListContextsResponse, String> {
    let mutex = state
        .conn
        .as_ref()
        .ok_or_else(|| "database is not open".to_string())?;
    let conn = mutex.lock().map_err(|e| e.to_string())?;

    let limit = limit.unwrap_or(200).clamp(1, 1000);
    let offset = offset.unwrap_or(0).max(0);

    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contexts WHERE superseded_by IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT id, uri, name, abstract, category, created_at
             FROM contexts
             WHERE superseded_by IS NULL
             ORDER BY created_at DESC
             LIMIT ? OFFSET ?",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([limit, offset], |row| {
            Ok(ContextRow {
                id: row.get(0)?,
                uri: row.get(1)?,
                name: row.get(2)?,
                abstract_text: row.get(3)?,
                category: row.get(4)?,
                created_at: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(ListContextsResponse { rows, total })
}
