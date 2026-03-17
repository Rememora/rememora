use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationRecord {
    pub id: String,
    pub source_uri: String,
    pub target_uri: String,
    pub relation_type: String,
    pub reason: String,
    pub created_at: String,
}

pub fn create(conn: &Connection, source_uri: &str, target_uri: &str, relation_type: &str, reason: &str) -> Result<String> {
    let id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO relations (id, source_uri, target_uri, relation_type, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, source_uri, target_uri, relation_type, reason, now],
    )?;

    Ok(id)
}

pub fn list_for_uri(conn: &Connection, uri: &str) -> Result<Vec<RelationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, source_uri, target_uri, relation_type, reason, created_at
         FROM relations WHERE source_uri = ?1 OR target_uri = ?1
         ORDER BY created_at DESC",
    )?;

    let rows = stmt
        .query_map(params![uri], |row| {
            Ok(RelationRecord {
                id: row.get(0)?,
                source_uri: row.get(1)?,
                target_uri: row.get(2)?,
                relation_type: row.get(3)?,
                reason: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}
