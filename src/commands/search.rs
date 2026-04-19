use anyhow::{bail, Result};
use rusqlite::Connection;

use rememora::format;
use rememora::propagate::PropagationConfig;
use rememora::search;

/// Output mode for `rememora search`.
///
/// - `Full`: current markdown format (numbered list, multi-line per hit).
/// - `Compact`: progressive-disclosure single-line-per-hit (~75 tokens).
/// - `Context`: tiny, length-capped form safe for inline prompt injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFormat {
    Full,
    Compact,
    Context,
}

impl SearchFormat {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "full" | "markdown" | "md" => Ok(Self::Full),
            "compact" => Ok(Self::Compact),
            "context" => Ok(Self::Context),
            other => bail!("unknown --format value: {other} (expected full|compact|context)"),
        }
    }
}

pub struct SearchArgs {
    pub query: String,
    pub project: Option<String>,
    pub category: Option<String>,
    pub limit: usize,
    pub propagate: bool,
    pub propagate_decay: f64,
    pub propagate_depth: usize,
    pub format: SearchFormat,
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
        match args.format {
            SearchFormat::Full => print!("{}", format::search_results_to_markdown(&results)),
            SearchFormat::Compact => print!("{}", format::search_results_to_compact(&results)),
            SearchFormat::Context => print!("{}", format::search_results_to_context(&results)),
        }
    }

    Ok(())
}
