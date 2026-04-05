use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

pub fn run(conn: &Connection, json: bool) -> Result<()> {
    let total_contexts: i64 = conn.query_row("SELECT COUNT(*) FROM contexts", [], |r| r.get(0))?;
    let total_memories: i64 = conn.query_row("SELECT COUNT(*) FROM contexts WHERE context_type = 'memory'", [], |r| r.get(0))?;
    let total_projects: i64 = conn.query_row("SELECT COUNT(*) FROM contexts WHERE context_type = 'project'", [], |r| r.get(0))?;
    let total_sessions: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
    let active_sessions: i64 = conn.query_row("SELECT COUNT(*) FROM sessions WHERE status = 'active'", [], |r| r.get(0))?;
    let total_relations: i64 = conn.query_row("SELECT COUNT(*) FROM relations", [], |r| r.get(0))?;

    // Count by category
    let mut stmt = conn.prepare("SELECT category, COUNT(*) FROM contexts WHERE context_type = 'memory' AND category IS NOT NULL GROUP BY category ORDER BY COUNT(*) DESC")?;
    let categories: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Curator stats
    let watermark_count: i64 = conn.query_row("SELECT COUNT(*) FROM watermarks", [], |r| r.get(0))?;
    let curator_actions: i64 = conn.query_row("SELECT COUNT(*) FROM curator_log", [], |r| r.get(0))?;
    let curator_adds: i64 = conn.query_row("SELECT COUNT(*) FROM curator_log WHERE action = 'add'", [], |r| r.get(0))?;
    let consolidation_runs: i64 = conn.query_row("SELECT COUNT(*) FROM consolidation_runs", [], |r| r.get(0))?;
    let last_consolidation: Option<String> = conn
        .query_row(
            "SELECT completed_at FROM consolidation_runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    if json {
        let cat_map: serde_json::Map<String, serde_json::Value> = categories
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::from(*v)))
            .collect();

        println!(
            "{}",
            serde_json::json!({
                "contexts": total_contexts,
                "memories": total_memories,
                "projects": total_projects,
                "sessions": total_sessions,
                "active_sessions": active_sessions,
                "relations": total_relations,
                "categories": cat_map,
                "curator": {
                    "tracked_files": watermark_count,
                    "total_actions": curator_actions,
                    "memories_added": curator_adds,
                    "consolidation_runs": consolidation_runs,
                    "last_consolidation": last_consolidation,
                },
            })
        );
    } else {
        println!("Rememora Status");
        println!("===============");
        println!("  Contexts: {total_contexts}");
        println!("  Memories: {total_memories}");
        println!("  Projects: {total_projects}");
        println!("  Sessions: {total_sessions} ({active_sessions} active)");
        println!("  Relations: {total_relations}");
        if !categories.is_empty() {
            println!("\n  Memory categories:");
            for (cat, count) in &categories {
                println!("    {cat}: {count}");
            }
        }
        println!("\n  Curator:");
        println!("    Tracked files:       {watermark_count}");
        println!("    Curation actions:    {curator_actions} ({curator_adds} adds)");
        println!("    Consolidation runs:  {consolidation_runs}");
        if let Some(last) = &last_consolidation {
            println!("    Last consolidation:  {last}");
        }
    }

    Ok(())
}
