use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRecord {
    pub id: String,
    pub uri: String,
    pub parent_uri: Option<String>,
    pub context_type: String,
    pub category: Option<String>,
    pub name: String,
    #[serde(rename = "abstract")]
    pub abstract_text: String,
    pub overview: String,
    pub content: String,
    pub tags: String,
    pub source_agent: Option<String>,
    pub source_session: Option<String>,
    pub importance: f64,
    pub active_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub superseded_by: Option<String>,
}

#[derive(Debug)]
pub struct InsertContext {
    pub uri: String,
    pub parent_uri: Option<String>,
    pub context_type: String,
    pub category: Option<String>,
    pub name: String,
    pub abstract_text: String,
    pub overview: String,
    pub content: String,
    pub tags: String,
    pub source_agent: Option<String>,
    pub source_session: Option<String>,
    pub importance: f64,
}

pub fn insert(conn: &Connection, ctx: &InsertContext) -> Result<String> {
    let id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO contexts (id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, ?14, ?14)",
        params![
            id,
            ctx.uri,
            ctx.parent_uri,
            ctx.context_type,
            ctx.category,
            ctx.name,
            ctx.abstract_text,
            ctx.overview,
            ctx.content,
            ctx.tags,
            ctx.source_agent,
            ctx.source_session,
            ctx.importance,
            now,
        ],
    )?;

    Ok(id)
}

pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<ContextRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by
         FROM contexts WHERE id = ?1",
    )?;

    let result = stmt
        .query_row(params![id], row_to_context)
        .optional()?;

    Ok(result)
}

pub fn get_by_uri(conn: &Connection, uri: &str) -> Result<Option<ContextRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by
         FROM contexts WHERE uri = ?1",
    )?;

    let result = stmt
        .query_row(params![uri], row_to_context)
        .optional()?;

    Ok(result)
}

pub fn list_by_parent(conn: &Connection, parent_uri: &str) -> Result<Vec<ContextRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by
         FROM contexts WHERE parent_uri = ?1 AND superseded_by IS NULL
         ORDER BY importance DESC, created_at DESC",
    )?;

    let rows = stmt
        .query_map(params![parent_uri], row_to_context)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn update(conn: &Connection, id: &str, abstract_text: Option<&str>, overview: Option<&str>, content: Option<&str>, importance: Option<f64>, tags: Option<&str>) -> Result<()> {
    let existing = get_by_id(conn, id)?;
    if existing.is_none() {
        bail!("Context not found: {id}");
    }
    let existing = existing.unwrap();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE contexts SET abstract = ?1, overview = ?2, content = ?3, importance = ?4, tags = ?5, updated_at = ?6 WHERE id = ?7",
        params![
            abstract_text.unwrap_or(&existing.abstract_text),
            overview.unwrap_or(&existing.overview),
            content.unwrap_or(&existing.content),
            importance.unwrap_or(existing.importance),
            tags.unwrap_or(&existing.tags),
            now,
            id,
        ],
    )?;

    Ok(())
}

pub fn supersede(conn: &Connection, old_id: &str, new_id: &str) -> Result<()> {
    let old = get_by_id(conn, old_id)?;
    if old.is_none() {
        bail!("Old context not found: {old_id}");
    }
    let new = get_by_id(conn, new_id)?;
    if new.is_none() {
        bail!("New context not found: {new_id}");
    }

    conn.execute(
        "UPDATE contexts SET superseded_by = ?1, updated_at = ?2 WHERE id = ?3",
        params![new_id, chrono::Utc::now().to_rfc3339(), old_id],
    )?;

    Ok(())
}

pub fn bump_active_count(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE contexts SET active_count = active_count + 1, updated_at = ?1 WHERE id = ?2",
        params![chrono::Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

pub fn list_by_scope(conn: &Connection, context_type: Option<&str>, category: Option<&str>, project: Option<&str>, limit: usize) -> Result<Vec<ContextRecord>> {
    let mut sql = String::from(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by
         FROM contexts WHERE superseded_by IS NULL",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if let Some(ct) = context_type {
        sql.push_str(&format!(" AND context_type = ?{param_idx}"));
        param_values.push(Box::new(ct.to_string()));
        param_idx += 1;
    }

    if let Some(cat) = category {
        sql.push_str(&format!(" AND category = ?{param_idx}"));
        param_values.push(Box::new(cat.to_string()));
        param_idx += 1;
    }

    if let Some(proj) = project {
        let prefix = format!("rememora://projects/{proj}/");
        sql.push_str(&format!(" AND uri LIKE ?{param_idx}"));
        param_values.push(Box::new(format!("{prefix}%")));
        param_idx += 1;
    }

    let _ = param_idx;
    sql.push_str(" ORDER BY importance DESC, created_at DESC");
    sql.push_str(&format!(" LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(params_ref.as_slice(), row_to_context)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

fn row_to_context(row: &rusqlite::Row) -> rusqlite::Result<ContextRecord> {
    Ok(ContextRecord {
        id: row.get(0)?,
        uri: row.get(1)?,
        parent_uri: row.get(2)?,
        context_type: row.get(3)?,
        category: row.get(4)?,
        name: row.get(5)?,
        abstract_text: row.get(6)?,
        overview: row.get(7)?,
        content: row.get(8)?,
        tags: row.get(9)?,
        source_agent: row.get(10)?,
        source_session: row.get(11)?,
        importance: row.get(12)?,
        active_count: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        superseded_by: row.get(16)?,
    })
}

use rusqlite::OptionalExtension;
