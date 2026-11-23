use anyhow::Result;
use rusqlite::Connection;

use rememora::format;
use rememora::hierarchy;
use rememora::models::project;
use rememora::models::session;
use rememora::models::watermark;

pub struct ContextArgs<'a> {
    pub project: Option<&'a str>,
    pub auto: bool,
    pub cheatsheet: bool,
    /// `None`: `format::DEFAULT_CONTEXT_MAX_TOKENS`. `Some(0)`: unlimited.
    pub max_tokens: Option<usize>,
    pub query: Option<&'a str>,
    pub json: bool,
}

pub fn run(conn: &Connection, args: &ContextArgs<'_>) -> Result<()> {
    let project_name = args.project;
    let auto = args.auto;
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    // `--auto` is what the SessionStart hook uses to load context, so it has to
    // see through a worktree the same way `save` and `search` do — a bare
    // `detect_from_cwd` returns None from any worktree and the session opens
    // with global memories only. An explicit `--project` goes through the same
    // ladder so a fabricated name gets rewritten rather than filtering to
    // nothing.
    let proj = if auto {
        project::resolve_for_cwd(conn, &cwd)?
    } else {
        project::resolve_write_target(conn, project_name, &cwd).project
    };

    if args.cheatsheet {
        return run_cheatsheet(conn, proj.as_deref());
    }

    let budget = match args.max_tokens {
        None => Some(format::DEFAULT_CONTEXT_MAX_TOKENS),
        Some(0) => None,
        Some(n) => Some(n),
    };
    let opts = hierarchy::ContextOptions {
        query: args.query.map(String::from),
    };
    let assembly = hierarchy::assemble_with(conn, proj.as_deref(), &opts)?;
    if args.json {
        println!("{}", format::context_to_json(&assembly, budget));
    } else {
        print!("{}", format::render_context(&assembly, budget).markdown);
    }

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
            println!(
                "- [{}] {} (importance: {:.1}) `{}`",
                cat, overview, sc.context.importance, sc.context.id
            );
        }
        println!();
    }

    // Handoff from the last agent, then who is working right now.
    if let Some(proj) = project {
        // Same block as the main context output: relative age, stale note.
        if let Some(h) = hierarchy::get_latest_session(conn, Some(proj))? {
            print!("{}", format::handoff_block(&h));
        }
        let active = session::list_active_for_project(conn, proj, hierarchy::ACTIVE_SESSION_LIMIT)?;
        if !active.is_empty() {
            println!("## Active Sessions\n");
            for a in &active {
                println!("{}", format::active_session_line(a));
            }
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

    if !l1.is_empty() {
        println!("_{}_", format::CONTEXT_GET_HINT);
    }

    Ok(())
}
