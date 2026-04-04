use anyhow::Result;
use rusqlite::Connection;

use rememora::format;
use rememora::hierarchy;
use rememora::models::project;
use rememora::models::watermark;

pub fn run(conn: &Connection, project_name: Option<&str>, auto: bool, cheatsheet: bool) -> Result<()> {
    let proj = if auto {
        let cwd = std::env::current_dir()?;
        project::detect_from_cwd(conn, cwd.to_str().unwrap_or(""))?
    } else {
        project_name.map(String::from)
    };

    if cheatsheet {
        return run_cheatsheet(conn, proj.as_deref());
    }

    let assembly = hierarchy::assemble(conn, proj.as_deref())?;
    print!("{}", format::context_to_markdown(&assembly));

    Ok(())
}

/// Compact cheatsheet: top-5 memories + working state + warnings.
fn run_cheatsheet(conn: &Connection, project: Option<&str>) -> Result<()> {
    let l1 = hierarchy::get_l1_context(conn, project, 5)?;

    if let Some(proj) = project {
        println!("# Rememora Cheatsheet: {proj}");
    } else {
        println!("# Rememora Cheatsheet");
    }
    println!();

    // Top memories
    if l1.is_empty() {
        println!("No memories stored yet.");
    } else {
        println!("## Key Memories\n");
        for sc in &l1 {
            let cat = sc.context.category.as_deref().unwrap_or("?");
            let overview = if sc.context.overview.is_empty() {
                &sc.context.abstract_text
            } else {
                &sc.context.overview
            };
            println!("- [{}] {} (importance: {:.1})", cat, overview, sc.context.importance);
        }
        println!();
    }

    // Working state from last session
    if let Some(session) = hierarchy::get_latest_session(conn, project)? {
        if !session.working_state.is_empty() {
            println!("## Working State\n");
            println!("{}", session.working_state);
            println!();
        }
        if !session.summary.is_empty() {
            println!("## Last Session\n");
            println!("- **Agent**: {}", session.agent);
            println!("- **Status**: {}", session.status);
            if !session.intent.is_empty() {
                println!("- **Intent**: {}", session.intent);
            }
            println!("- **Summary**: {}", session.summary);
            println!();
        }
    }

    // Warnings
    let mut warnings = Vec::new();

    // Check if consolidation is overdue
    let last_consolidation = watermark::latest_consolidation(conn, project)?;
    match &last_consolidation {
        None => {
            let mem_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM contexts WHERE context_type = 'memory' AND superseded_by IS NULL",
                [],
                |r| r.get(0),
            )?;
            if mem_count > 10 {
                warnings.push(format!("Consolidation has never run ({mem_count} memories — consider `rememora consolidate`)"));
            }
        }
        Some(run) => {
            if let Some(completed) = &run.completed_at {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(completed) {
                    let days = (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_days();
                    if days > 7 {
                        warnings.push(format!("Last consolidation was {days} days ago"));
                    }
                }
            }
        }
    }

    if !warnings.is_empty() {
        println!("## Warnings\n");
        for w in &warnings {
            println!("- {w}");
        }
        println!();
    }

    Ok(())
}
