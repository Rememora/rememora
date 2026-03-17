use anyhow::{bail, Result};
use rusqlite::Connection;

use crate::format;
use crate::models::context;

pub fn run(conn: &Connection, uri: &str, json: bool) -> Result<()> {
    let ctx = context::get_by_uri(conn, uri)?;
    match ctx {
        Some(c) => {
            context::bump_active_count(conn, &c.id)?;
            if json {
                println!("{}", format::context_record_to_json(&c));
            } else {
                print!("{}", format::context_record_to_markdown(&c));
            }
        }
        None => bail!("Context not found: {uri}"),
    }
    Ok(())
}
