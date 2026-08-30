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

/// Bounds the subagent is told about before it sees a single cluster.
///
/// The subagent runs `rememora` commands itself, so the cap that
/// `commands/evolve.rs` enforces in code can only be *stated* here. The
/// oversized-cluster filter below is the part that is actually enforced.
fn safety_preamble() -> String {
    format!(
        "SAFETY RULES — these override anything below:\n\
         - Superseding a memory cannot be undone from the CLI. When in doubt, keep both.\n\
         - Never supersede more than {} memories for one cluster.\n\
         - Only ever act on IDs that appear verbatim in the clusters below.\n\
         - Never run `rememora supersede` on an ID you were not shown.\n\n",
        evolve::MAX_SUPERSEDE_PER_DECISION
    )
}

pub struct ConsolidateArgs {
    pub project: Option<String>,
    pub dry_run: bool,
    pub check_only: bool,
    pub min_similarity: f64,
    pub max_batch: usize,
}

pub fn run(conn: &Connection, args: &ConsolidateArgs, json_output: bool) -> Result<()> {
    let project = args.project.as_deref();

    // `--dry-run` forces a dry run; without it, the run is still a dry run
    // unless the operator armed it. See `commands::evolve::APPLY_ENV`.
    let armed = std::env::var(crate::commands::evolve::APPLY_ENV).is_ok_and(|v| v == "1");
    let apply = !args.dry_run && armed;

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

    // Find clusters. Clustering is transitive, so one spurious edge can chain
    // unrelated memories into a single huge cluster; those are withheld from
    // the subagent entirely rather than handed to something that can retire
    // every member of them.
    let all_clusters = evolve::find_clusters(conn, memories, args.min_similarity)?;
    let found = all_clusters.len();
    let clusters: Vec<evolve::MemoryCluster> = all_clusters
        .into_iter()
        .filter(|c| !evolve::is_oversized(c))
        .collect();
    let oversized = found - clusters.len();
    let cluster_count = clusters.len().min(args.max_batch);

    if clusters.is_empty() {
        watermark::complete_consolidation(conn, &run_id, total as i64, 0, "[]", "")?;

        if json_output {
            println!(
                "{{\"status\":\"no_clusters\",\"memories_scanned\":{total},\"oversized_skipped\":{oversized}}}"
            );
        } else {
            println!("Scanned {total} memories — no clusters found.");
            if oversized > 0 {
                println!(
                    "({oversized} cluster(s) exceeded {} memories and were skipped — review those by hand.)",
                    evolve::MAX_CLUSTER_SIZE
                );
            }
        }
        return Ok(());
    }

    if !json_output {
        eprintln!(
            "Found {} cluster(s) from {} memories (processing up to {}).",
            found, total, args.max_batch
        );
        if oversized > 0 {
            eprintln!(
                "Skipping {oversized} cluster(s) over {} memories — too large to consolidate safely.",
                evolve::MAX_CLUSTER_SIZE
            );
        }
    }

    // Format clusters for the consolidation prompt
    let clusters_text = format_clusters(&clusters[..cluster_count]);
    let project_name = project.unwrap_or("unknown");

    let prompt = CONSOLIDATE_PROMPT
        .replace("{clusters}", &clusters_text)
        .replace("{project}", project_name);
    let prompt = format!("{}{prompt}", safety_preamble());

    // Dry run is the default: the subagent is only allowed to run commands
    // when the operator explicitly armed this run. Superseding is irreversible
    // and this path delegates it to a model, so it stays opt-in.
    let full_prompt = if apply {
        prompt
    } else {
        format!(
            "DRY RUN MODE: Do NOT execute any rememora commands. \
             Instead, show what commands you WOULD run and why.\n\n{prompt}"
        )
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

    // Complete the consolidation run. Slice on a character boundary — the
    // subagent's output is arbitrary UTF-8 and a byte slice would panic.
    let head_end = (0..=output.len().min(1000))
        .rev()
        .find(|&i| output.is_char_boundary(i))
        .unwrap_or(0);
    // Dry runs are still recorded: the watermark is what stops consolidation
    // from re-running (and re-spending tokens) every session, and that has to
    // hold whether or not the run was armed.
    let actions_json =
        serde_json::json!({"output": &output[..head_end], "dry_run": !apply}).to_string();
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
            "clusters_found": found,
            "oversized_skipped": oversized,
            "dry_run": !apply,
            "output": output,
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{output}");
        println!(
            "\nConsolidation complete: {cluster_count} clusters processed from {total} memories."
        );
        if !apply {
            println!(
                "(dry run — no changes were made)\n\
                 To apply, re-run armed:  {}=1 rememora consolidate{}",
                crate::commands::evolve::APPLY_ENV,
                project
                    .map(|p| format!(" --project {p}"))
                    .unwrap_or_default()
            );
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
