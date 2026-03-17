use anyhow::Result;
use rusqlite::Connection;

use crate::models::relation;

pub fn run(conn: &Connection, source: &str, target: &str, relation_type: &str, reason: &str, json: bool) -> Result<()> {
    let id = relation::create(conn, source, target, relation_type, reason)?;

    if json {
        println!("{}", serde_json::json!({"id": id}));
    } else {
        println!("Relation created: {id}");
    }

    Ok(())
}
