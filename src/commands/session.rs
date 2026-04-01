use anyhow::Result;
use rusqlite::Connection;

use rememora::format;
use rememora::models::project;
use rememora::models::session;

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

pub fn end_active(
    conn: &Connection,
    project: Option<&str>,
    summary: Option<&str>,
    working_state: Option<&str>,
    auto_summary: bool,
    json: bool,
) -> Result<()> {
    // Resolve project: explicit flag or auto-detect from CWD
    let resolved_project = if let Some(p) = project {
        Some(p.to_string())
    } else {
        let cwd = std::env::current_dir()?;
        project::detect_from_cwd(conn, cwd.to_str().unwrap_or(""))?
    };

    let project_name = match resolved_project {
        Some(p) => p,
        None => {
            // No project detected — no-op for hook safety
            if json {
                println!(
                    "{}",
                    serde_json::json!({"status": "no-op", "reason": "no project detected"})
                );
            }
            return Ok(());
        }
    };

    // Find the most recent active session for this project
    let active = session::get_active_for_project(conn, &project_name)?;

    let sess = match active {
        Some(s) => s,
        None => {
            // No active session — no-op for hook safety
            if json {
                println!(
                    "{}",
                    serde_json::json!({"status": "no-op", "reason": "no active session"})
                );
            }
            return Ok(());
        }
    };

    // Build summary text
    let final_summary = if let Some(s) = summary {
        s.to_string()
    } else if auto_summary {
        // Generate from session metadata
        let duration = compute_duration(&sess.started_at);
        if sess.intent.is_empty() {
            format!("Session ended automatically. Duration: {duration}")
        } else {
            format!(
                "Session ended automatically. Intent: {}. Duration: {duration}",
                sess.intent
            )
        }
    } else {
        String::new()
    };

    session::end(conn, &sess.id, &final_summary, working_state, None)?;

    if json {
        println!(
            "{}",
            serde_json::json!({"status": "ok", "id": sess.id, "project": project_name})
        );
    } else {
        println!("Session ended: {}", sess.id);
    }

    Ok(())
}

/// Compute a human-readable duration string from a start timestamp to now.
fn compute_duration(started_at: &str) -> String {
    let start = chrono::DateTime::parse_from_rfc3339(started_at);
    match start {
        Ok(start_time) => {
            let elapsed = chrono::Utc::now().signed_duration_since(start_time);
            let total_secs = elapsed.num_seconds();
            if total_secs < 60 {
                format!("{total_secs}s")
            } else if total_secs < 3600 {
                format!("{}m", total_secs / 60)
            } else {
                let hours = total_secs / 3600;
                let mins = (total_secs % 3600) / 60;
                format!("{hours}h {mins}m")
            }
        }
        Err(_) => "unknown".to_string(),
    }
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
