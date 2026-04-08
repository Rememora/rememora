use anyhow::Result;
use rusqlite::Connection;

use rememora::format;
use rememora::propagate::PropagationConfig;
use rememora::search;

pub struct SearchArgs {
    pub query: String,
    pub project: Option<String>,
    pub category: Option<String>,
    pub limit: usize,
    pub propagate: bool,
    pub propagate_decay: f64,
    pub propagate_depth: usize,
}

pub fn run(conn: &Connection, args: &SearchArgs, json: bool) -> Result<()> {
    let results = if args.propagate {
        let config = PropagationConfig {
            decay_factor: args.propagate_decay,
            max_depth: args.propagate_depth,
        };
        search::search_with_propagation(
            conn,
            &args.query,
            args.project.as_deref(),
            args.category.as_deref(),
            args.limit,
            &config,
        )?
    } else {
        search::search(
            conn,
            &args.query,
            args.project.as_deref(),
            args.category.as_deref(),
            args.limit,
        )?
    };

    if json {
        println!("{}", format::search_results_to_json(&results));
    } else {
        print!("{}", format::search_results_to_markdown(&results));
    }

    Ok(())
}
