use anyhow::Result;
use rusqlite::Connection;

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
    }

    Ok(())
}
