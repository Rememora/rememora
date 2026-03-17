use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::context::{self, ContextRecord, InsertContext};
use crate::uri;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: Option<String>,
    pub description: String,
    pub tech_stack: Vec<String>,
    pub conventions: String,
    pub last_active: String,
}

pub fn add(conn: &Connection, name: &str, path: Option<&str>, description: &str, stack: &[String]) -> Result<String> {
    let project_uri = uri::build_project_uri(name);
    let stack_json = serde_json::to_string(stack)?;

    // Store path and stack in the content field as structured JSON
    let content = serde_json::json!({
        "path": path,
        "tech_stack": stack,
        "conventions": "",
    })
    .to_string();

    let id = context::insert(
        conn,
        &InsertContext {
            uri: project_uri,
            parent_uri: Some("rememora://projects".to_string()),
            context_type: "project".to_string(),
            category: None,
            name: name.to_string(),
            abstract_text: description.to_string(),
            overview: format!("Project: {name}. Stack: {}", stack.join(", ")),
            content,
            tags: stack_json,
            source_agent: None,
            source_session: None,
            importance: 1.0,
        },
    )?;

    Ok(id)
}

pub fn list(conn: &Connection) -> Result<Vec<ContextRecord>> {
    context::list_by_scope(conn, Some("project"), None, None, 100)
}

pub fn get(conn: &Connection, name: &str) -> Result<Option<ContextRecord>> {
    let uri = uri::build_project_uri(name);
    context::get_by_uri(conn, &uri)
}

pub fn get_info(conn: &Connection, name: &str) -> Result<Option<ProjectInfo>> {
    let record = get(conn, name)?;
    match record {
        None => Ok(None),
        Some(rec) => {
            let content: serde_json::Value = serde_json::from_str(&rec.content).unwrap_or_default();
            let tech_stack: Vec<String> = serde_json::from_value(
                content.get("tech_stack").cloned().unwrap_or_default(),
            )
            .unwrap_or_default();

            Ok(Some(ProjectInfo {
                name: rec.name,
                path: content.get("path").and_then(|v| v.as_str()).map(String::from),
                description: rec.abstract_text,
                tech_stack,
                conventions: content
                    .get("conventions")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                last_active: rec.updated_at,
            }))
        }
    }
}

pub fn detect_from_cwd(conn: &Connection, cwd: &str) -> Result<Option<String>> {
    let projects = list(conn)?;
    for proj in projects {
        let content: serde_json::Value = serde_json::from_str(&proj.content).unwrap_or_default();
        if let Some(path) = content.get("path").and_then(|v| v.as_str()) {
            if cwd.starts_with(path) {
                return Ok(Some(proj.name));
            }
        }
    }
    Ok(None)
}

pub fn update_last_active(conn: &Connection, name: &str) -> Result<()> {
    let uri = uri::build_project_uri(name);
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE contexts SET updated_at = ?1 WHERE uri = ?2",
        params![now, uri],
    )?;
    Ok(())
}
