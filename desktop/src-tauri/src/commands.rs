use std::sync::{Mutex, MutexGuard, PoisonError};

use rememora::models::context::{self, ContextRecord};
use rememora::models::project;
use rememora::models::session::{self, SessionRecord};
use rememora::search;
use rusqlite::Connection;
use serde::Serialize;

/// Managed state: a Mutex-wrapped SQLite connection.
pub struct DbState(pub Mutex<Connection>);

fn lock_db(state: &DbState) -> Result<MutexGuard<'_, Connection>, String> {
    state
        .0
        .lock()
        .map_err(|e: PoisonError<MutexGuard<'_, Connection>>| e.to_string())
}

/// Stats for the dashboard overview.
#[derive(Debug, Serialize)]
pub struct DashboardStats {
    pub total_memories: i64,
    pub by_project: Vec<ProjectCount>,
    pub by_category: Vec<CategoryCount>,
    pub active_sessions: i64,
}

#[derive(Debug, Serialize)]
pub struct ProjectCount {
    pub project: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct CategoryCount {
    pub category: String,
    pub count: i64,
}

/// Serializable search result (SearchResult in rememora::search is not Serialize).
#[derive(Debug, Serialize)]
pub struct SearchResultDto {
    pub context: ContextRecord,
    pub rank: f64,
}

#[tauri::command]
pub fn get_projects(state: tauri::State<'_, DbState>) -> Result<Vec<ContextRecord>, String> {
    let conn = lock_db(&state)?;
    project::list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_memories(
    state: tauri::State<'_, DbState>,
    project: Option<String>,
    category: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<ContextRecord>, String> {
    let conn = lock_db(&state)?;
    // context_type = "memory" filters out project records
    context::list_by_scope(
        &conn,
        Some("memory"),
        category.as_deref(),
        project.as_deref(),
        limit.unwrap_or(50),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_memory_detail(
    state: tauri::State<'_, DbState>,
    id: String,
) -> Result<Option<ContextRecord>, String> {
    let conn = lock_db(&state)?;
    context::get_by_id(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn search_memories(
    state: tauri::State<'_, DbState>,
    query: String,
    project: Option<String>,
    category: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<SearchResultDto>, String> {
    let conn = lock_db(&state)?;
    let results = search::search(
        &conn,
        &query,
        project.as_deref(),
        category.as_deref(),
        limit.unwrap_or(20),
    )
    .map_err(|e| e.to_string())?;

    Ok(results
        .into_iter()
        .map(|r| SearchResultDto {
            context: r.context,
            rank: r.rank,
        })
        .collect())
}

#[tauri::command]
pub fn get_dashboard_stats(state: tauri::State<'_, DbState>) -> Result<DashboardStats, String> {
    let conn = lock_db(&state)?;

    let total_memories: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contexts WHERE context_type = 'memory' AND superseded_by IS NULL",
            [],
            |row: &rusqlite::Row| row.get(0),
        )
        .map_err(|e: rusqlite::Error| e.to_string())?;

    // Count by project: extract project name from URI
    let mut by_project_stmt = conn
        .prepare(
            "SELECT
                CASE
                    WHEN uri LIKE 'rememora://projects/%' THEN
                        SUBSTR(uri, LENGTH('rememora://projects/') + 1,
                            INSTR(SUBSTR(uri, LENGTH('rememora://projects/') + 1), '/') - 1)
                    ELSE 'global'
                END AS project_name,
                COUNT(*) as cnt
             FROM contexts
             WHERE context_type = 'memory' AND superseded_by IS NULL
             GROUP BY project_name
             ORDER BY cnt DESC",
        )
        .map_err(|e: rusqlite::Error| e.to_string())?;

    let by_project: Vec<ProjectCount> = by_project_stmt
        .query_map([], |row: &rusqlite::Row| {
            Ok(ProjectCount {
                project: row.get(0)?,
                count: row.get(1)?,
            })
        })
        .map_err(|e: rusqlite::Error| e.to_string())?
        .filter_map(|r: Result<ProjectCount, rusqlite::Error>| r.ok())
        .collect();

    // Count by category
    let mut by_category_stmt = conn
        .prepare(
            "SELECT COALESCE(category, 'uncategorized') as cat, COUNT(*) as cnt
             FROM contexts
             WHERE context_type = 'memory' AND superseded_by IS NULL
             GROUP BY cat
             ORDER BY cnt DESC",
        )
        .map_err(|e: rusqlite::Error| e.to_string())?;

    let by_category: Vec<CategoryCount> = by_category_stmt
        .query_map([], |row: &rusqlite::Row| {
            Ok(CategoryCount {
                category: row.get(0)?,
                count: row.get(1)?,
            })
        })
        .map_err(|e: rusqlite::Error| e.to_string())?
        .filter_map(|r: Result<CategoryCount, rusqlite::Error>| r.ok())
        .collect();

    let active_sessions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE status = 'active'",
            [],
            |row: &rusqlite::Row| row.get(0),
        )
        .map_err(|e: rusqlite::Error| e.to_string())?;

    Ok(DashboardStats {
        total_memories,
        by_project,
        by_category,
        active_sessions,
    })
}

#[tauri::command]
pub fn get_sessions(
    state: tauri::State<'_, DbState>,
    project: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<SessionRecord>, String> {
    let conn = lock_db(&state)?;
    session::list(&conn, project.as_deref(), limit.unwrap_or(10)).map_err(|e| e.to_string())
}
