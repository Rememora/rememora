use anyhow::Result;
use rusqlite::Connection;

use crate::format;
use crate::search;

pub struct SearchArgs {
    pub query: String,
    pub project: Option<String>,
    pub category: Option<String>,
    pub limit: usize,
}

pub fn run(conn: &Connection, args: &SearchArgs, json: bool) -> Result<()> {
    let results = search::search(
        conn,
        &args.query,
        args.project.as_deref(),
        args.category.as_deref(),
        args.limit,
    )?;

    if json {
        println!("{}", format::search_results_to_json(&results));
    } else {
        print!("{}", format::search_results_to_markdown(&results));
    }

    Ok(())
}
