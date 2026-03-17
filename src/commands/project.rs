use anyhow::Result;
use rusqlite::Connection;

use crate::models::project;

pub fn add(conn: &Connection, name: &str, path: Option<&str>, description: &str, stack: &[String], json: bool) -> Result<()> {
    let id = project::add(conn, name, path, description, stack)?;

    if json {
        println!("{}", serde_json::json!({"id": id, "name": name}));
    } else {
        println!("Project added: {name} ({id})");
    }

    Ok(())
}

pub fn list(conn: &Connection, json: bool) -> Result<()> {
    let projects = project::list(conn)?;

    if json {
        let items: Vec<serde_json::Value> = projects
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "uri": p.uri,
                    "description": p.abstract_text,
                    "updated_at": p.updated_at,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        if projects.is_empty() {
            println!("No projects registered.");
            return Ok(());
        }
        for p in &projects {
            println!("  {} - {}", p.name, p.abstract_text);
        }
    }

    Ok(())
}

pub fn show(conn: &Connection, name: &str, json: bool) -> Result<()> {
    let info = project::get_info(conn, name)?;
    match info {
        Some(info) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("# Project: {}", info.name);
                if let Some(ref path) = info.path {
                    println!("  Path: {path}");
                }
                println!("  Description: {}", info.description);
                if !info.tech_stack.is_empty() {
                    println!("  Stack: {}", info.tech_stack.join(", "));
                }
                println!("  Last active: {}", info.last_active);
            }
        }
        None => println!("Project not found: {name}"),
    }
    Ok(())
}
