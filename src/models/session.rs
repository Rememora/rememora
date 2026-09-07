use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub agent: String,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: String,
    pub intent: String,
    pub working_state: String,
    pub message_count: i64,
    pub token_estimate: i64,
    pub parent_session: Option<String>,
    pub status: String,
    /// Linked worktree the session ran in; `None` means the main checkout.
    /// Migration 007 — `cwd` records the raw directory, this records what it
    /// *means* once the project has been folded onto the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Branch checked out when the session started.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Provenance for a session, mirroring the `worktree`/`branch` columns.
///
/// A struct rather than two more positional parameters: `start` already takes
/// six, and two adjacent `Option<&str>` in a row is exactly the shape that gets
/// silently transposed at a call site.
#[derive(Debug, Clone, Default)]
pub struct Provenance<'a> {
    pub worktree: Option<&'a str>,
    pub branch: Option<&'a str>,
}

pub fn start(conn: &Connection, agent: &str, project: Option<&str>, cwd: Option<&str>, intent: &str, parent_session: Option<&str>, prov: &Provenance<'_>) -> Result<String> {
    let id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO sessions (id, agent, project, cwd, started_at, intent, parent_session, status, worktree, branch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, ?9)",
        params![id, agent, project, cwd, now, intent, parent_session, prov.worktree, prov.branch],
    )?;

    Ok(id)
}

pub fn end(conn: &Connection, id: &str, summary: &str, working_state: Option<&str>, status: Option<&str>) -> Result<()> {
    let existing = get_by_id(conn, id)?;
    if existing.is_none() {
        bail!("Session not found: {id}");
    }

    let now = chrono::Utc::now().to_rfc3339();
    let final_status = status.unwrap_or("ended");

    conn.execute(
        "UPDATE sessions SET ended_at = ?1, summary = ?2, working_state = COALESCE(?3, working_state), status = ?4 WHERE id = ?5",
        params![now, summary, working_state, final_status, id],
    )?;

    Ok(())
}

pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<SessionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent, project, cwd, started_at, ended_at, summary, intent, working_state, message_count, token_estimate, parent_session, status, worktree, branch
         FROM sessions WHERE id = ?1",
    )?;

    let result = stmt
        .query_row(params![id], row_to_session)
        .optional()?;

    Ok(result)
}

pub fn get_latest_for_project(conn: &Connection, project: &str) -> Result<Option<SessionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent, project, cwd, started_at, ended_at, summary, intent, working_state, message_count, token_estimate, parent_session, status, worktree, branch
         FROM sessions WHERE project = ?1
         ORDER BY started_at DESC LIMIT 1",
    )?;

    let result = stmt
        .query_row(params![project], row_to_session)
        .optional()?;

    Ok(result)
}

pub fn get_active_for_project(conn: &Connection, project: &str) -> Result<Option<SessionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent, project, cwd, started_at, ended_at, summary, intent, working_state, message_count, token_estimate, parent_session, status, worktree, branch
         FROM sessions WHERE project = ?1 AND status = 'active'
         ORDER BY started_at DESC LIMIT 1",
    )?;

    let result = stmt
        .query_row(params![project], row_to_session)
        .optional()?;

    Ok(result)
}

pub fn list(conn: &Connection, project: Option<&str>, limit: usize) -> Result<Vec<SessionRecord>> {
    let (sql, param_values): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(proj) = project {
        (
            "SELECT id, agent, project, cwd, started_at, ended_at, summary, intent, working_state, message_count, token_estimate, parent_session, status, worktree, branch
             FROM sessions WHERE project = ?1 ORDER BY started_at DESC LIMIT ?2".to_string(),
            vec![Box::new(proj.to_string()) as Box<dyn rusqlite::types::ToSql>, Box::new(limit as i64)],
        )
    } else {
        (
            "SELECT id, agent, project, cwd, started_at, ended_at, summary, intent, working_state, message_count, token_estimate, parent_session, status, worktree, branch
             FROM sessions ORDER BY started_at DESC LIMIT ?1".to_string(),
            vec![Box::new(limit as i64) as Box<dyn rusqlite::types::ToSql>],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(params_ref.as_slice(), row_to_session)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        agent: row.get(1)?,
        project: row.get(2)?,
        cwd: row.get(3)?,
        started_at: row.get(4)?,
        ended_at: row.get(5)?,
        summary: row.get(6)?,
        intent: row.get(7)?,
        working_state: row.get(8)?,
        message_count: row.get(9)?,
        token_estimate: row.get(10)?,
        parent_session: row.get(11)?,
        status: row.get(12)?,
        worktree: row.get(13)?,
        branch: row.get(14)?,
    })
}
