use anyhow::Result;
use rusqlite::Connection;

use rememora::models::context;

pub fn run(conn: &Connection, project: Option<&str>, format: &str) -> Result<()> {
    let contexts = context::list_by_scope(conn, None, None, project, 10000)?;

    match format {
        "json" => {
            let items: Vec<serde_json::Value> = contexts
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or_default())
                .collect();
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        "md" | "markdown" => {
            for c in &contexts {
                println!("{}", rememora::format::context_record_to_markdown(c));
                println!("---\n");
            }
        }
        _ => {
            // Default to JSON
            let items: Vec<serde_json::Value> = contexts
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or_default())
                .collect();
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
    }

    Ok(())
}
