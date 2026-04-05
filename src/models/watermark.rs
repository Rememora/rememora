use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

// ── Watermark ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatermarkRecord {
    pub file_path: String,
    pub byte_offset: u64,
    pub line_count: u64,
    pub updated_at: String,
}

/// Get the watermark for a session file. Returns None if not yet tracked.
pub fn get(conn: &Connection, file_path: &str) -> Result<Option<WatermarkRecord>> {
    let mut stmt = conn.prepare(
        "SELECT file_path, byte_offset, line_count, updated_at
         FROM watermarks WHERE file_path = ?1",
    )?;

    let result = stmt
        .query_row(params![file_path], |row| {
            Ok(WatermarkRecord {
                file_path: row.get(0)?,
                byte_offset: row.get::<_, i64>(1)? as u64,
                line_count: row.get::<_, i64>(2)? as u64,
                updated_at: row.get(3)?,
            })
        })
        .optional()?;

    Ok(result)
}

/// Set (upsert) the watermark for a session file.
pub fn set(conn: &Connection, file_path: &str, byte_offset: u64, line_count: u64) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO watermarks (file_path, byte_offset, line_count, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(file_path) DO UPDATE SET
             byte_offset = excluded.byte_offset,
             line_count = excluded.line_count,
             updated_at = excluded.updated_at",
        params![file_path, byte_offset as i64, line_count as i64, now],
    )?;

    Ok(())
}

/// Reset watermark to zero (re-curate from beginning).
pub fn reset(conn: &Connection, file_path: &str) -> Result<()> {
    set(conn, file_path, 0, 0)
}

/// List all tracked watermarks.
pub fn list(conn: &Connection) -> Result<Vec<WatermarkRecord>> {
    let mut stmt = conn.prepare(
        "SELECT file_path, byte_offset, line_count, updated_at
         FROM watermarks ORDER BY updated_at DESC",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok(WatermarkRecord {
                file_path: row.get(0)?,
                byte_offset: row.get::<_, i64>(1)? as u64,
                line_count: row.get::<_, i64>(2)? as u64,
                updated_at: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

// ── Curator Log ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratorLogEntry {
    pub id: String,
    pub file_path: String,
    pub action: String,
    pub context_id: Option<String>,
    pub reason: String,
    pub model: String,
    pub created_at: String,
}

/// Log a curation action.
pub fn log_action(
    conn: &Connection,
    file_path: &str,
    action: &str,
    context_id: Option<&str>,
    reason: &str,
    model: &str,
) -> Result<String> {
    let id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO curator_log (id, file_path, action, context_id, reason, model, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, file_path, action, context_id, reason, model, now],
    )?;

    Ok(id)
}

/// Get recent curator log entries for a file.
pub fn get_log(conn: &Connection, file_path: &str, limit: usize) -> Result<Vec<CuratorLogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, action, context_id, reason, model, created_at
         FROM curator_log WHERE file_path = ?1
         ORDER BY created_at DESC LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(params![file_path, limit as i64], |row| {
            Ok(CuratorLogEntry {
                id: row.get(0)?,
                file_path: row.get(1)?,
                action: row.get(2)?,
                context_id: row.get(3)?,
                reason: row.get(4)?,
                model: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

// ── Consolidation Runs ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationRun {
    pub id: String,
    pub project: Option<String>,
    pub memories_before: i64,
    pub memories_after: i64,
    pub clusters_found: i64,
    pub actions_taken: String,
    pub model: String,
    pub triggered_by: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// Start a consolidation run. Returns the run ID.
pub fn start_consolidation(
    conn: &Connection,
    project: Option<&str>,
    memories_before: i64,
    triggered_by: &str,
) -> Result<String> {
    let id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO consolidation_runs (id, project, memories_before, triggered_by, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, project, memories_before, triggered_by, now],
    )?;

    Ok(id)
}

/// Complete a consolidation run with results.
pub fn complete_consolidation(
    conn: &Connection,
    id: &str,
    memories_after: i64,
    clusters_found: i64,
    actions_taken: &str,
    model: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE consolidation_runs
         SET memories_after = ?1, clusters_found = ?2, actions_taken = ?3,
             model = ?4, completed_at = ?5
         WHERE id = ?6",
        params![memories_after, clusters_found, actions_taken, model, now, id],
    )?;

    Ok(())
}

/// Get the latest consolidation run for a project.
pub fn latest_consolidation(
    conn: &Connection,
    project: Option<&str>,
) -> Result<Option<ConsolidationRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, project, memories_before, memories_after, clusters_found,
                actions_taken, model, triggered_by, started_at, completed_at
         FROM consolidation_runs
         WHERE (?1 IS NULL AND project IS NULL) OR project = ?1
         ORDER BY started_at DESC LIMIT 1",
    )?;

    let result = stmt
        .query_row(params![project], |row| {
            Ok(ConsolidationRun {
                id: row.get(0)?,
                project: row.get(1)?,
                memories_before: row.get(2)?,
                memories_after: row.get(3)?,
                clusters_found: row.get(4)?,
                actions_taken: row.get(5)?,
                model: row.get(6)?,
                triggered_by: row.get(7)?,
                started_at: row.get(8)?,
                completed_at: row.get(9)?,
            })
        })
        .optional()?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn test_watermark_crud() {
        let conn = db::open_memory().unwrap();

        // Initially no watermark
        let wm = get(&conn, "/path/to/session.jsonl").unwrap();
        assert!(wm.is_none());

        // Set watermark
        set(&conn, "/path/to/session.jsonl", 1024, 50).unwrap();
        let wm = get(&conn, "/path/to/session.jsonl").unwrap().unwrap();
        assert_eq!(wm.byte_offset, 1024);
        assert_eq!(wm.line_count, 50);

        // Update watermark
        set(&conn, "/path/to/session.jsonl", 2048, 100).unwrap();
        let wm = get(&conn, "/path/to/session.jsonl").unwrap().unwrap();
        assert_eq!(wm.byte_offset, 2048);
        assert_eq!(wm.line_count, 100);
    }

    #[test]
    fn test_watermark_reset() {
        let conn = db::open_memory().unwrap();

        set(&conn, "/path/to/session.jsonl", 5000, 200).unwrap();
        reset(&conn, "/path/to/session.jsonl").unwrap();

        let wm = get(&conn, "/path/to/session.jsonl").unwrap().unwrap();
        assert_eq!(wm.byte_offset, 0);
        assert_eq!(wm.line_count, 0);
    }

    #[test]
    fn test_watermark_list() {
        let conn = db::open_memory().unwrap();

        set(&conn, "/path/a.jsonl", 100, 10).unwrap();
        set(&conn, "/path/b.jsonl", 200, 20).unwrap();

        let wms = list(&conn).unwrap();
        assert_eq!(wms.len(), 2);
    }

    #[test]
    fn test_curator_log() {
        let conn = db::open_memory().unwrap();

        // Create a context so the FK constraint is satisfied
        let ctx_id = "ctx-test-123";
        conn.execute(
            "INSERT INTO contexts (id, uri, context_type, category, name, abstract, created_at, updated_at)
             VALUES (?1, 'rememora://test/ctx', 'memory', 'decision', 'test', 'test', datetime('now'), datetime('now'))",
            rusqlite::params![ctx_id],
        ).unwrap();

        let id = log_action(
            &conn,
            "/path/to/session.jsonl",
            "add",
            Some(ctx_id),
            "New decision about auth approach",
            "sonnet",
        )
        .unwrap();
        assert!(!id.is_empty());

        log_action(
            &conn,
            "/path/to/session.jsonl",
            "noop",
            None,
            "No actionable signal",
            "haiku",
        )
        .unwrap();

        let entries = get_log(&conn, "/path/to/session.jsonl", 10).unwrap();
        assert_eq!(entries.len(), 2);
        // Most recent first
        assert_eq!(entries[0].action, "noop");
        assert_eq!(entries[1].action, "add");
        assert_eq!(entries[1].context_id.as_deref(), Some(ctx_id));
    }

    #[test]
    fn test_consolidation_lifecycle() {
        let conn = db::open_memory().unwrap();

        let run_id =
            start_consolidation(&conn, Some("rememora"), 42, "manual").unwrap();

        let run = latest_consolidation(&conn, Some("rememora"))
            .unwrap()
            .unwrap();
        assert_eq!(run.memories_before, 42);
        assert!(run.completed_at.is_none());

        complete_consolidation(&conn, &run_id, 35, 5, "[\"merge\",\"prune\"]", "sonnet").unwrap();

        let run = latest_consolidation(&conn, Some("rememora"))
            .unwrap()
            .unwrap();
        assert_eq!(run.memories_after, 35);
        assert_eq!(run.clusters_found, 5);
        assert!(run.completed_at.is_some());
    }
}
