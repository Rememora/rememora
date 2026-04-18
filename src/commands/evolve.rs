use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::collections::HashSet;

use rememora::evolve::{self, MemoryCluster};
use rememora::models::agent_invocation::{self, Caller};
use rememora::models::context::{self, ContextRecord, InsertContext};
use rememora::uri;

/// Summary of evolution results.
#[derive(Debug, Default, serde::Serialize)]
pub struct EvolveSummary {
    pub memories_scanned: usize,
    pub clusters_found: usize,
    pub merges: usize,
    pub supersessions: usize,
    pub kept: usize,
    pub actions: Vec<ActionReport>,
}

#[derive(Debug, serde::Serialize)]
pub struct ActionReport {
    pub cluster_ids: Vec<String>,
    pub action: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersede_ids: Option<Vec<String>>,
}

/// LLM response for a single cluster.
#[derive(Debug, serde::Deserialize)]
struct LlmDecision {
    action: String,
    reason: String,
    #[serde(default)]
    merged_text: Option<String>,
    #[serde(default)]
    keep_id: Option<String>,
    #[serde(default)]
    supersede_ids: Option<Vec<String>>,
}

const CONSOLIDATION_PROMPT: &str = r#"You are consolidating a knowledge base. Below are memories from the same category that appear related.

For each cluster, decide ONE action:
- MERGE: Combine into a single, better memory (provide the merged text)
- SUPERSEDE: One memory clearly replaces another (specify which ID to keep and which to supersede)
- KEEP: All memories are distinct enough to keep separately

Consider:
- Higher importance scores indicate more critical knowledge
- Higher active_count indicates more frequently accessed knowledge
- Prefer newer information when facts conflict
- Preserve specific details (file paths, error messages, exact decisions)

Respond with ONLY a JSON object (no markdown fences):
{
  "action": "merge" | "supersede" | "keep",
  "reason": "brief explanation",
  "merged_text": "...",
  "keep_id": "...",
  "supersede_ids": ["..."]
}

Where:
- "merged_text" is only required for "merge" action
- "keep_id" and "supersede_ids" are only required for "supersede" action

Here are the memories:

"#;

pub fn run(
    conn: &Connection,
    project: Option<&str>,
    dry_run: bool,
    min_similarity: f64,
    max_batch: usize,
    json_output: bool,
) -> Result<()> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY environment variable not set. The evolve command requires an LLM to consolidate memories.")?;

    // Phase 1: Load memories and find clusters
    let memories = load_active_memories(conn, project)?;
    if memories.is_empty() {
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&EvolveSummary::default())?
            );
        } else {
            println!("No active memories found for consolidation.");
        }
        return Ok(());
    }

    let total_scanned = memories.len();
    if !json_output {
        println!("Scanning {} memories for consolidation...", total_scanned);
    }

    let clusters = evolve::find_clusters(conn, memories, min_similarity)?;
    let cluster_count = clusters.len().min(max_batch);

    if clusters.is_empty() {
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&EvolveSummary {
                    memories_scanned: total_scanned,
                    ..Default::default()
                })?
            );
        } else {
            println!("No clusters of similar memories found.");
        }
        return Ok(());
    }

    if !json_output {
        println!(
            "Found {} cluster(s) of related memories (processing up to {}).\n",
            clusters.len(),
            max_batch
        );
    }

    // Phase 2 & 3: Consolidate and apply
    let mut summary = EvolveSummary {
        memories_scanned: total_scanned,
        clusters_found: clusters.len(),
        ..Default::default()
    };

    for cluster in clusters.into_iter().take(cluster_count) {
        let decision = consolidate_cluster(&api_key, &cluster, conn, project)?;
        let report = apply_decision(conn, &cluster, &decision, dry_run, project)?;

        match report.action.as_str() {
            "merge" => summary.merges += 1,
            "supersede" => summary.supersessions += 1,
            "keep" => summary.kept += 1,
            _ => {}
        }

        if !json_output && dry_run {
            print_dry_run_report(&report);
        }

        summary.actions.push(report);
    }

    // Print summary
    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("\n--- Evolution Summary ---");
        println!("Memories scanned: {}", summary.memories_scanned);
        println!("Clusters found:   {}", summary.clusters_found);
        println!("Merges:           {}", summary.merges);
        println!("Supersessions:    {}", summary.supersessions);
        println!("Kept as-is:       {}", summary.kept);
        if dry_run {
            println!("\n(dry run — no changes were made)");
        }
    }

    Ok(())
}

/// Load all non-superseded memories for a given project scope.
fn load_active_memories(
    conn: &Connection,
    project: Option<&str>,
) -> Result<Vec<ContextRecord>> {
    context::list_by_scope(conn, Some("memory"), None, project, 10_000)
}

/// Call the Anthropic API to decide how to consolidate a cluster.
fn consolidate_cluster(
    api_key: &str,
    cluster: &MemoryCluster,
    conn: &Connection,
    project: Option<&str>,
) -> Result<LlmDecision> {
    const EVOLVE_MODEL: &str = "claude-haiku-4-5-20251001";
    let mut prompt = String::from(CONSOLIDATION_PROMPT);

    for mem in &cluster.memories {
        prompt.push_str(&format!(
            "---\nID: {}\nName: {}\nCategory: {}\nImportance: {:.1}\nActive count: {}\nCreated: {}\nContent: {}\n\n",
            mem.id,
            mem.name,
            mem.category.as_deref().unwrap_or("unknown"),
            mem.importance,
            mem.active_count,
            mem.created_at,
            mem.content,
        ));
    }

    let body = serde_json::json!({
        "model": EVOLVE_MODEL,
        "max_tokens": 1024,
        "messages": [
            {"role": "user", "content": prompt}
        ]
    });

    let resp = ureq::post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", api_key)
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .send_json(&body)
        .context("Failed to call Claude API for consolidation")?;

    let resp_body: serde_json::Value = resp.into_json().context("Failed to parse API response")?;

    agent_invocation::try_insert(
        conn,
        &agent_invocation::record_from_anthropic_api(
            Caller::Evolve,
            EVOLVE_MODEL,
            project.map(str::to_string),
            None,
            &resp_body,
            false,
        ),
    );

    let content_text = resp_body["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or("{}");

    // Parse JSON, stripping markdown code fences if present
    let json_str = content_text.trim();
    let json_str = if json_str.starts_with("```") {
        let start = json_str.find('{').unwrap_or(0);
        let end = json_str
            .rfind('}')
            .map(|i| i + 1)
            .unwrap_or(json_str.len());
        &json_str[start..end]
    } else {
        json_str
    };

    let decision: LlmDecision =
        serde_json::from_str(json_str).context("Failed to parse LLM consolidation response")?;

    // Validate the decision
    match decision.action.as_str() {
        "merge" => {
            if decision.merged_text.is_none() {
                bail!("LLM returned 'merge' action but no merged_text");
            }
        }
        "supersede" => {
            if decision.keep_id.is_none() || decision.supersede_ids.is_none() {
                bail!(
                    "LLM returned 'supersede' action but missing keep_id or supersede_ids"
                );
            }
        }
        "keep" => {} // no extra fields needed
        other => bail!("Unknown LLM action: {other}"),
    }

    Ok(decision)
}

/// Apply a consolidation decision to the database.
fn apply_decision(
    conn: &Connection,
    cluster: &MemoryCluster,
    decision: &LlmDecision,
    dry_run: bool,
    project: Option<&str>,
) -> Result<ActionReport> {
    let cluster_ids: Vec<String> = cluster.memories.iter().map(|m| m.id.clone()).collect();

    match decision.action.as_str() {
        "merge" => {
            let merged_text = decision.merged_text.as_deref().unwrap_or("");

            // Pick the highest-importance memory as the template for category/agent
            let best = cluster
                .memories
                .iter()
                .max_by(|a, b| {
                    a.importance
                        .partial_cmp(&b.importance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();

            let mut new_id = None;
            if !dry_run {
                let slug = uri::slugify(&merged_text.chars().take(60).collect::<String>());
                let mem_uri = uri::build_memory_uri(
                    project,
                    best.category.as_deref().unwrap_or("entity"),
                    &slug,
                );
                let parent = uri::parent(&mem_uri)?.unwrap_or_default();

                // Aggregate importance: max of cluster + small boost
                let max_importance = cluster
                    .memories
                    .iter()
                    .map(|m| m.importance)
                    .fold(0.0_f64, f64::max);
                let importance = (max_importance + 0.05).min(1.0);

                let id = context::insert(
                    conn,
                    &InsertContext {
                        uri: mem_uri,
                        parent_uri: Some(parent),
                        context_type: "memory".into(),
                        category: best.category.clone(),
                        name: truncate(merged_text, 80),
                        abstract_text: truncate(merged_text, 200),
                        overview: merged_text.to_string(),
                        content: merged_text.to_string(),
                        tags: best.tags.clone(),
                        source_agent: best.source_agent.clone(),
                        source_session: None,
                        importance,
                    },
                )?;

                // Supersede all cluster members to point to the new merged memory
                for mem in &cluster.memories {
                    context::supersede(conn, &mem.id, &id)?;
                }

                new_id = Some(id);
            }

            Ok(ActionReport {
                cluster_ids,
                action: "merge".into(),
                reason: decision.reason.clone(),
                merged_text: Some(merged_text.to_string()),
                new_id,
                keep_id: None,
                supersede_ids: None,
            })
        }
        "supersede" => {
            let keep_id = decision.keep_id.as_deref().unwrap_or("");
            let supersede_ids = decision.supersede_ids.as_deref().unwrap_or(&[]);

            // Validate that all IDs are in the cluster
            let cluster_id_set: HashSet<&str> =
                cluster.memories.iter().map(|m| m.id.as_str()).collect();

            if !cluster_id_set.contains(keep_id) {
                bail!(
                    "LLM returned keep_id '{}' which is not in the cluster",
                    keep_id
                );
            }

            for sid in supersede_ids {
                if !cluster_id_set.contains(sid.as_str()) {
                    bail!(
                        "LLM returned supersede_id '{}' which is not in the cluster",
                        sid
                    );
                }
            }

            if !dry_run {
                for sid in supersede_ids {
                    context::supersede(conn, sid, keep_id)?;
                }
            }

            Ok(ActionReport {
                cluster_ids,
                action: "supersede".into(),
                reason: decision.reason.clone(),
                merged_text: None,
                new_id: None,
                keep_id: Some(keep_id.to_string()),
                supersede_ids: Some(supersede_ids.to_vec()),
            })
        }
        "keep" => Ok(ActionReport {
            cluster_ids,
            action: "keep".into(),
            reason: decision.reason.clone(),
            merged_text: None,
            new_id: None,
            keep_id: None,
            supersede_ids: None,
        }),
        other => bail!("Unknown action: {other}"),
    }
}

fn print_dry_run_report(report: &ActionReport) {
    println!("Cluster: {}", report.cluster_ids.join(", "));
    println!("  Action: {}", report.action);
    println!("  Reason: {}", report.reason);
    match report.action.as_str() {
        "merge" => {
            if let Some(text) = &report.merged_text {
                println!("  Merged text: {}", truncate(text, 120));
            }
        }
        "supersede" => {
            if let Some(kid) = &report.keep_id {
                println!("  Keep: {kid}");
            }
            if let Some(sids) = &report.supersede_ids {
                println!("  Supersede: {}", sids.join(", "));
            }
        }
        _ => {}
    }
    println!();
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
