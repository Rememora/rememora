use anyhow::Result;
use rusqlite::Connection;

use crate::models::context;

pub fn run(conn: &Connection, old_id: &str, new_id: &str, json: bool) -> Result<()> {
    context::supersede(conn, old_id, new_id)?;

    if json {
        println!("{}", serde_json::json!({"status": "ok", "old_id": old_id, "new_id": new_id}));
    } else {
        println!("Superseded: {old_id} → {new_id}");
    }

    Ok(())
}
