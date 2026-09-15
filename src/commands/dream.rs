use anyhow::Result;
use rusqlite::Connection;

use super::{curate, evolve};

/// `rememora dream` — the one command that runs memory upkeep end-to-end,
/// on demand. Rememora fires nothing automatically from Claude Code /
/// Gemini CLI hooks; this is how you tell it to catch up: curate any
/// session transcripts that piled up, then evolve (dedup/prune/merge) the
/// resulting memories — applying the decisions unless `--dry-run` is set.
/// Run it by hand, or wire it to your own cron / launchd job — rememora
/// itself installs no schedule.
pub struct DreamArgs {
    /// Project scope. `None` curates every auto-discovered session file
    /// under its own detected project and evolves across all projects.
    pub project: Option<String>,
    /// Show what would happen without writing anything.
    pub dry_run: bool,
}

pub fn run(conn: &Connection, args: &DreamArgs, json_output: bool) -> Result<()> {
    if !json_output {
        println!("== Curating pending sessions ==");
    }
    let curate_args = curate::CurateArgs {
        file: None,
        from_stdin: false,
        auto: true,
        dry_run: args.dry_run,
        reset_watermark: false,
        project: args.project.clone(),
    };
    curate::run(conn, &curate_args, json_output)?;

    if !json_output {
        println!("\n== Evolving memories ==");
    }
    let evolve_args = evolve::EvolveArgs {
        project: args.project.as_deref(),
        dry_run: args.dry_run,
        apply: !args.dry_run,
        undo_log: false,
        min_similarity: 0.3,
        max_batch: 50,
    };
    evolve::run(conn, &evolve_args, json_output)?;

    Ok(())
}
