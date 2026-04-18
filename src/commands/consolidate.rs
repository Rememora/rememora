use anyhow::{Context, Result};
use rusqlite::Connection;

use rememora::curator;
use rememora::evolve;
use rememora::models::context;
use rememora::models::watermark;

const CONSOLIDATE_PROMPT: &str = include_str!("../../prompts/consolidate.md");

/// Dual-gate thresholds: consolidation only runs when BOTH are met.
const MIN_HOURS_SINCE_LAST: f64 = 24.0;
const MIN_NEW_MEMORIES: i64 = 5;

/// Exit code indicating the gate is met and consolidation should run.
pub const GATE_MET_EXIT: i32 = 42;

pub struct ConsolidateArgs {
    pub project: Option<String>,
    pub dry_run: bool,
    pub check_only: bool,
    pub min_similarity: f64,
    pub max_batch: usize,
}

pub fn run(conn: &Connection, args: &ConsolidateArgs, json_output: bool) -> Result<()> {
    let project = args.project.as_deref();

    // Dual-gate check: 24h since last consolidation + 5 new memories
    if args.check_only {
        return check_gate(conn, project, json_output);
    }

    let gate_met = is_gate_met(conn, project)?;
    if !gate_met && !args.dry_run {
        if json_output {
            println!("{{\"status\":\"gate_not_met\",\"message\":\"Dual gate not met (need 24h + 5 new memories)\"}}");
        } else {
            println!("Consolidation gate not met (need {}h since last run + {} new memories).",
                MIN_HOURS_SINCE_LAST, MIN_NEW_MEMORIES);
        }
        return Ok(());
    }

    // Load active memories
    let memories = context::list_by_scope(conn, Some("memory"), None, project, 10_000)?;
    if memories.is_empty() {
        if json_output {
            println!("{{\"status\":\"no_memories\"}}");
        } else {
            println!("No active memories to consolidate.");
        }
        return Ok(());
    }

    let total = memories.len();

    // Start a consolidation run
    let run_id = watermark::start_consolidation(
        conn,
        project,
        total as i64,
        "manual",
    )?;

    // Find clusters
    let clusters = evolve::find_clusters(conn, memories, args.min_similarity)?;
    let cluster_count = clusters.len().min(args.max_batch);

    if clusters.is_empty() {
        watermark::complete_consolidation(conn, &run_id, total as i64, 0, "[]", "")?;

        if json_output {
            println!("{{\"status\":\"no_clusters\",\"memories_scanned\":{total}}}");
        } else {
            println!("Scanned {total} memories — no clusters found.");
        }
        return Ok(());
    }

    if !json_output {
        eprintln!(
            "Found {} cluster(s) from {} memories (processing up to {}).",
            clusters.len(),
            total,
            args.max_batch
        );
    }

    // Format clusters for the consolidation prompt
    let clusters_text = format_clusters(&clusters[..cluster_count]);
    let project_name = project.unwrap_or("unknown");

    let prompt = CONSOLIDATE_PROMPT
        .replace("{clusters}", &clusters_text)
        .replace("{project}", project_name);

    let full_prompt = if args.dry_run {
        format!(
            "DRY RUN MODE: Do NOT execute any rememora commands. \
             Instead, show what commands you WOULD run and why.\n\n{prompt}"
        )
    } else {
        prompt
    };

    let subagent_output = curator::call_subagent(&full_prompt, "sonnet")?;
    let output = subagent_output.text;

    // Record the consolidate subagent call so it appears in `rememora usage`.
    rememora::models::agent_invocation::try_insert(
        conn,
        &rememora::models::agent_invocation::record_from_subagent(
            rememora::models::agent_invocation::Caller::Consolidate,
            project.map(str::to_string),
            None,
            &subagent_output.telemetry,
        ),
    );

    // Complete the consolidation run
    let actions_json = serde_json::json!({"output": &output[..output.len().min(1000)]}).to_string();
    watermark::complete_consolidation(
        conn,
        &run_id,
        total as i64, // approximate — subagent handles the actual changes
        cluster_count as i64,
        &actions_json,
        "sonnet",
    )?;

    if json_output {
        let result = serde_json::json!({
            "status": "completed",
            "run_id": run_id,
            "memories_scanned": total,
            "clusters_processed": cluster_count,
            "dry_run": args.dry_run,
            "output": output,
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{output}");
        println!(
            "\nConsolidation complete: {cluster_count} clusters processed from {total} memories."
        );
        if args.dry_run {
            println!("(dry run — no changes were made)");
        }
    }

    Ok(())
}

/// Check if the dual gate is met. Used by --check-only and by cron/session-start.
fn check_gate(conn: &Connection, project: Option<&str>, json_output: bool) -> Result<()> {
    let met = is_gate_met(conn, project)?;

    if json_output {
        println!("{{\"gate_met\":{met}}}");
    } else if met {
        println!("Consolidation gate met — ready to run.");
    } else {
        println!("Consolidation gate not met.");
    }

    if met {
        std::process::exit(GATE_MET_EXIT);
    }

    Ok(())
}

/// Check the dual gate: >= 24h since last consolidation AND >= 5 new memories since then.
fn is_gate_met(conn: &Connection, project: Option<&str>) -> Result<bool> {
    let last_run = watermark::latest_consolidation(conn, project)?;

    match last_run {
        None => {
            // Never consolidated — check if we have enough memories
            let count = context::list_by_scope(conn, Some("memory"), None, project, 1)?.len();
            Ok(count > 0)
        }
        Some(run) => {
            let completed_at = run
                .completed_at
                .as_deref()
                .unwrap_or(&run.started_at);

            let last_time = chrono::DateTime::parse_from_rfc3339(completed_at)
                .context("Failed to parse consolidation timestamp")?;

            let hours_since = (chrono::Utc::now() - last_time.with_timezone(&chrono::Utc))
                .num_minutes() as f64
                / 60.0;

            if hours_since < MIN_HOURS_SINCE_LAST {
                return Ok(false);
            }

            // Count memories created after last consolidation
            let new_memories: i64 = conn.query_row(
                "SELECT COUNT(*) FROM contexts
                 WHERE context_type = 'memory'
                   AND superseded_by IS NULL
                   AND created_at > ?1
                   AND (?2 IS NULL OR uri LIKE 'rememora://projects/' || ?2 || '/%')",
                rusqlite::params![completed_at, project],
                |row| row.get(0),
            )?;

            Ok(new_memories >= MIN_NEW_MEMORIES)
        }
    }
}

/// Format memory clusters into text for the consolidation prompt.
fn format_clusters(clusters: &[evolve::MemoryCluster]) -> String {
    let now = chrono::Utc::now();
    let mut out = String::new();

    for (i, cluster) in clusters.iter().enumerate() {
        out.push_str(&format!("### Cluster {} ({} memories)\n\n", i + 1, cluster.memories.len()));

        // Sort by created_at to determine temporal labels
        let mut sorted: Vec<_> = cluster.memories.iter().collect();
        sorted.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        let newest = sorted.last().map(|m| &m.created_at);

        for mem in &sorted {
            let label = if Some(&mem.created_at) == newest {
                "[NEWER]"
            } else {
                "[OLDER]"
            };

            // Calculate age
            let age = chrono::DateTime::parse_from_rfc3339(&mem.created_at)
                .map(|dt| {
                    let days = (now - dt.with_timezone(&chrono::Utc)).num_days();
                    if days == 0 {
                        "today".to_string()
                    } else if days == 1 {
                        "1 day ago".to_string()
                    } else {
                        format!("{days} days ago")
                    }
                })
                .unwrap_or_else(|_| "unknown".to_string());

            out.push_str(&format!(
                "- {label} ID: `{}`\n  Category: {}\n  Importance: {:.1} | Accesses: {} | Created: {}\n  Text: {}\n\n",
                mem.id,
                mem.category.as_deref().unwrap_or("unknown"),
                mem.importance,
                mem.active_count,
                age,
                mem.content,
            ));
        }
    }

    out
}
