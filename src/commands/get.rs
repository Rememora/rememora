use anyhow::{bail, Result};
use rusqlite::Connection;

use rememora::format;
use rememora::models::context;

/// Print one memory in full. `reference` is its id or its `rememora://` URI.
pub fn run(conn: &Connection, reference: &str, json: bool) -> Result<()> {
    let ctx = context::resolve_ref(conn, reference.trim())?;
    match ctx {
        Some(c) => {
            context::bump_active_count(conn, &c.id)?;
            if json {
                println!("{}", format::context_record_to_json(&c));
            } else {
                print!("{}", format::context_record_to_markdown(&c));
            }
        }
        None => bail!("Context not found: {reference}"),
    }
    Ok(())
}
