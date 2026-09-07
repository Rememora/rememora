use anyhow::Result;
use rusqlite::Connection;

use rememora::models::project;

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

/// Re-home memories filed under project namespaces that no project claims.
///
/// Dry run unless `apply` is set, matching `evolve` — this rewrites URIs, so
/// the default has to be the one that cannot lose anything. The dry run prints
/// how each namespace resolved *and by what route*, because "session cwd" and
/// "encoded path" carry different confidence and the operator should be able to
/// tell them apart before arming it.
pub fn reconcile(conn: &Connection, apply: bool, json: bool) -> Result<()> {
    let plan = project::find_stranded(conn)?;

    if plan.is_empty() {
        if json {
            println!("{}", serde_json::json!({"stranded": [], "applied": false}));
        } else {
            println!("No stranded project namespaces. Every memory is filed under a registered project.");
        }
        return Ok(());
    }

    if !apply {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "stranded": plan,
                    "applied": false,
                }))?
            );
        } else {
            println!("# Stranded project namespaces (dry run)\n");
            for ns in &plan {
                match &ns.target {
                    // Already the right name — the project was simply never
                    // registered. Nothing moves; say so rather than implying a
                    // rewrite is pending.
                    Some(t) if t == &ns.name => println!(
                        "  {} ({} memories) → already coherent, just unregistered \
                         — run `rememora project add {}`",
                        ns.name, ns.memories, ns.name
                    ),
                    Some(t) => println!(
                        "  {} ({} memories) → {}  [{}]",
                        ns.name, ns.memories, t, ns.via
                    ),
                    None => println!(
                        "  {} ({} memories) → UNRESOLVED — register the project or move these by hand",
                        ns.name, ns.memories
                    ),
                }
                if let Some(w) = &ns.worktree {
                    println!("      worktree: {w}");
                }
            }
            let resolvable: usize = plan
                .iter()
                .filter(|n| n.target.as_deref().is_some_and(|t| t != n.name))
                .map(|n| n.memories)
                .sum();
            println!("\n{resolvable} memories would be re-homed. Re-run with --apply to commit.");
        }
        return Ok(());
    }

    let outcome = project::reconcile(conn, &plan)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        println!(
            "Re-homed {} memories and {} sessions across {} namespaces.",
            outcome.memories_moved, outcome.sessions_moved, outcome.namespaces_rewritten
        );
        if outcome.conflicts > 0 {
            println!(
                "{} memories left in place — the destination URI already exists (the same \
                 memory was saved under both names). Review and supersede one of each pair.",
                outcome.conflicts
            );
        }
        if !outcome.needs_registration.is_empty() {
            println!(
                "Already coherent but unregistered: {}",
                outcome.needs_registration.join(", ")
            );
            println!("Register these with `rememora project add <name> --path <path>`.");
        }
        if !outcome.unresolved.is_empty() {
            println!("Unresolved: {}", outcome.unresolved.join(", "));
            println!("Register these with `rememora project add <name> --path <path>` and re-run.");
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
