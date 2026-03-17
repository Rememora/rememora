use anyhow::Result;
use rusqlite::{params, Connection};

use crate::models::context::{self, ContextRecord};

pub struct SearchResult {
    pub context: ContextRecord,
    pub rank: f64,
}

pub fn search(
    conn: &Connection,
    query: &str,
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Build FTS5 query — escape special characters
    let fts_query = query
        .replace('"', "\"\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" OR ");

    if fts_query.is_empty() {
        return Ok(vec![]);
    }

    let mut sql = String::from(
        "SELECT c.id, c.uri, c.parent_uri, c.context_type, c.category, c.name,
                c.abstract, c.overview, c.content, c.tags, c.source_agent,
                c.source_session, c.importance, c.active_count, c.created_at,
                c.updated_at, c.superseded_by, rank
         FROM contexts_fts fts
         JOIN contexts c ON c.rowid = fts.rowid
         WHERE contexts_fts MATCH ?1
         AND c.superseded_by IS NULL",
    );

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(fts_query));
    let mut param_idx = 2;

    if let Some(proj) = project {
        let prefix = format!("rememora://projects/{proj}/");
        sql.push_str(&format!(" AND c.uri LIKE ?{param_idx}"));
        param_values.push(Box::new(format!("{prefix}%")));
        param_idx += 1;

        // Also include global memories
        sql = sql.replace(
            &format!("AND c.uri LIKE ?{}", param_idx - 1),
            &format!("AND (c.uri LIKE ?{} OR c.uri LIKE 'rememora://global/%')", param_idx - 1),
        );
    }

    if let Some(cat) = category {
        sql.push_str(&format!(" AND c.category = ?{param_idx}"));
        param_values.push(Box::new(cat.to_string()));
        param_idx += 1;
    }

    let _ = param_idx;
    sql.push_str(" ORDER BY rank");
    sql.push_str(&format!(" LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            let rank: f64 = row.get(17)?;
            Ok(SearchResult {
                context: ContextRecord {
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
                },
                rank,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Bump active_count for returned results
    for result in &rows {
        context::bump_active_count(conn, &result.context.id)?;
    }

    Ok(rows)
}
