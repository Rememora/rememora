use anyhow::Result;
use rusqlite::Connection;

use crate::format;
use crate::models::session;

pub fn start(conn: &Connection, agent: &str, project: Option<&str>, intent: &str, parent: Option<&str>, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(String::from));

    let id = session::start(conn, agent, project, cwd.as_deref(), intent, parent)?;

    if json {
        println!("{}", serde_json::json!({"id": id}));
    } else {
        println!("{id}");
    }

    Ok(())
}

pub fn end(conn: &Connection, id: &str, summary: &str, working_state: Option<&str>, status: Option<&str>, json: bool) -> Result<()> {
    session::end(conn, id, summary, working_state, status)?;

    if json {
        println!("{}", serde_json::json!({"status": "ok", "id": id}));
    } else {
        println!("Session ended: {id}");
    }

    Ok(())
}

pub fn resume(conn: &Connection, project: &str) -> Result<()> {
    let latest = session::get_latest_for_project(conn, project)?;
    match latest {
        Some(s) => print!("{}", format::session_to_markdown(&s)),
        None => println!("No sessions found for project: {project}"),
    }
    Ok(())
}

pub fn list(conn: &Connection, project: Option<&str>, limit: usize, json: bool) -> Result<()> {
    let sessions = session::list(conn, project, limit)?;

    if json {
        let items: Vec<serde_json::Value> = sessions
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "agent": s.agent,
                    "project": s.project,
                    "status": s.status,
                    "intent": s.intent,
                    "summary": s.summary,
                    "started_at": s.started_at,
                    "ended_at": s.ended_at,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        if sessions.is_empty() {
            println!("No sessions found.");
            return Ok(());
        }
        for s in &sessions {
            let status_marker = match s.status.as_str() {
                "active" => "[active]",
                "transferred" => "[transferred]",
                _ => "[ended]",
            };
            println!(
                "{} {} {} - {} ({})",
                s.id,
                status_marker,
                s.agent,
                if s.intent.is_empty() { &s.summary } else { &s.intent },
                s.started_at,
            );
        }
    }

    Ok(())
}
