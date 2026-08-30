use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::io::Write;
use std::path::{Path, PathBuf};

use rememora::evolve::{self, MemoryCluster, MAX_CLUSTER_SIZE, MAX_SUPERSEDE_PER_DECISION};
use rememora::models::agent_invocation::{self, Caller};
use rememora::models::context::{self, ContextRecord, InsertContext};
use rememora::uri;

/// Environment variable that arms the destructive path.
///
/// Consolidation supersedes memories and nothing in the CLI can put them
/// back, so evolve is dry-run by default and only writes when this is set to
/// `1`. `--dry-run` still wins over it.
pub const APPLY_ENV: &str = "REMEMORA_APPLY";

/// Summary of evolution results.
#[derive(Debug, Default, serde::Serialize)]
pub struct EvolveSummary {
    pub memories_scanned: usize,
    pub clusters_found: usize,
    pub merges: usize,
    pub supersessions: usize,
    pub kept: usize,
    /// Clusters refused before the LLM ever saw them (too large to be safe).
    pub skipped: usize,
    /// Decisions the model returned that failed validation and were discarded.
    pub rejected: usize,
    pub actions: Vec<ActionReport>,
}

#[derive(Debug, serde::Serialize)]
pub struct ActionReport {
    pub cluster_ids: Vec<String>,
    pub action: String,
    pub reason: String,
    /// False for every dry-run decision — nothing was written to the DB.
    pub applied: bool,
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

/// One applied decision, written to the undo journal *before* the writes it
/// describes are committed.
#[derive(Debug, serde::Serialize)]
struct UndoEntry {
    at: String,
    action: String,
    project: Option<String>,
    /// Memories that were active before this decision and are being retired.
    superseded_ids: Vec<String>,
    /// The memory they now point at.
    superseded_by: String,
    /// Set for "merge": the memory this run created, which did not exist before.
    #[serde(skip_serializing_if = "Option::is_none")]
    created_id: Option<String>,
    /// Ready-to-run SQL that reverses this entry exactly.
    undo_sql: Vec<String>,
}

/// Options for one consolidation run.
pub struct EvolveArgs<'a> {
    pub project: Option<&'a str>,
    /// Write decisions to the database. Dry run is the default.
    pub apply: bool,
    pub min_similarity: f64,
    pub max_batch: usize,
}

fn consolidation_prompt() -> String {
    format!(
        r#"You are consolidating a knowledge base. Below are memories from the same category that appear related.

For each cluster, decide ONE action:
- MERGE: Combine into a single, better memory (provide the merged text and the exact IDs it replaces)
- SUPERSEDE: One memory clearly replaces others (specify which ID to keep and which to supersede)
- KEEP: All memories are distinct enough to keep separately

Hard rules — a response that breaks any of these is discarded:
- "supersede_ids" is REQUIRED for both "merge" and "supersede".
- Every ID in "supersede_ids" must be copied verbatim from the list below. Never invent one.
- "supersede_ids" must never contain more than {MAX_SUPERSEDE_PER_DECISION} IDs. If more than that are
  redundant, pick the {MAX_SUPERSEDE_PER_DECISION} clearest and answer only for those.
- For "merge", "merged_text" must cover exactly the memories in "supersede_ids" and nothing else.
- For "supersede", "keep_id" must not also appear in "supersede_ids".
- Superseding is irreversible. When in doubt, answer "keep".

Consider:
- Higher importance scores indicate more critical knowledge
- Higher active_count indicates more frequently accessed knowledge
- Prefer newer information when facts conflict
- Preserve specific details (file paths, error messages, exact decisions)

Respond with ONLY a JSON object (no markdown fences):
{{
  "action": "merge" | "supersede" | "keep",
  "reason": "brief explanation",
  "merged_text": "...",
  "keep_id": "...",
  "supersede_ids": ["..."]
}}

Where:
- "merged_text" and "supersede_ids" are required for "merge"
- "keep_id" and "supersede_ids" are required for "supersede"

Here are the memories:

"#
    )
}

/// CLI entry point.
///
/// `dry_run` mirrors the `--dry-run` flag, but a dry run is what happens
/// either way unless the run is explicitly armed: evolve writes only when
/// `REMEMORA_APPLY=1` is set *and* `--dry-run` was not passed. The command
/// supersedes memories and the CLI has no way to reverse that, so the
/// destructive path is opt-in rather than default.
pub fn run(
    conn: &Connection,
    project: Option<&str>,
    dry_run: bool,
    min_similarity: f64,
    max_batch: usize,
    json_output: bool,
) -> Result<()> {
    let apply = !dry_run && apply_armed();

    run_with(
        conn,
        &EvolveArgs {
            project,
            apply,
            min_similarity,
            max_batch,
        },
        json_output,
    )
}

/// True when the operator explicitly armed the destructive path.
fn apply_armed() -> bool {
    std::env::var(APPLY_ENV).is_ok_and(|v| v == "1")
}

pub fn run_with(conn: &Connection, args: &EvolveArgs<'_>, json_output: bool) -> Result<()> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY environment variable not set. The evolve command requires an LLM to consolidate memories.")?;

    let project = args.project;

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

    let clusters = evolve::find_clusters(conn, memories, args.min_similarity)?;
    let cluster_count = clusters.len().min(args.max_batch);

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
            args.max_batch
        );
    }

    // Phase 2 & 3: Consolidate and apply
    let mut summary = EvolveSummary {
        memories_scanned: total_scanned,
        clusters_found: clusters.len(),
        ..Default::default()
    };

    let journal = UndoJournal::for_connection(conn);

    for cluster in clusters.into_iter().take(cluster_count) {
        // Refuse oversized clusters before spending a token on them. Clustering
        // is transitive, so a large cluster is more likely to be one bad edge
        // chaining unrelated memories than a genuine pile of duplicates.
        if evolve::is_oversized(&cluster) {
            let report = ActionReport {
                cluster_ids: cluster.memories.iter().map(|m| m.id.clone()).collect(),
                action: "skipped".into(),
                reason: format!(
                    "cluster has {} memories, over the safe limit of {MAX_CLUSTER_SIZE} — review it by hand",
                    cluster.memories.len()
                ),
                applied: false,
                merged_text: None,
                new_id: None,
                keep_id: None,
                supersede_ids: None,
            };
            summary.skipped += 1;
            if !json_output {
                print_report(&report);
            }
            summary.actions.push(report);
            continue;
        }

        let decision = consolidate_cluster(&api_key, &cluster, conn, project)?;

        // A decision that fails validation is discarded, not partially applied,
        // and the run continues with the next cluster.
        let report = match apply_decision(conn, &cluster, &decision, args.apply, project, &journal) {
            Ok(report) => report,
            Err(e) => {
                summary.rejected += 1;
                ActionReport {
                    cluster_ids: cluster.memories.iter().map(|m| m.id.clone()).collect(),
                    action: "rejected".into(),
                    reason: format!("{e}"),
                    applied: false,
                    merged_text: None,
                    new_id: None,
                    keep_id: None,
                    supersede_ids: None,
                }
            }
        };

        match report.action.as_str() {
            "merge" => summary.merges += 1,
            "supersede" => summary.supersessions += 1,
            "keep" => summary.kept += 1,
            _ => {}
        }

        if !json_output {
            print_report(&report);
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
        println!("Skipped (large):  {}", summary.skipped);
        println!("Rejected:         {}", summary.rejected);
        if args.apply {
            println!("\nUndo journal: {}", journal.path.display());
        } else {
            println!(
                "\n(dry run — no changes were made)\n\
                 To apply, re-run armed:  {APPLY_ENV}=1 rememora evolve{}",
                project
                    .map(|p| format!(" --project {p}"))
                    .unwrap_or_default()
            );
        }
    }

    Ok(())
}

/// Load all non-superseded memories for a given project scope.
fn load_active_memories(conn: &Connection, project: Option<&str>) -> Result<Vec<ContextRecord>> {
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
    let mut prompt = consolidation_prompt();

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
        let end = json_str.rfind('}').map(|i| i + 1).unwrap_or(json_str.len());
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
            if decision.supersede_ids.is_none() {
                bail!("LLM returned 'merge' action but no supersede_ids");
            }
        }
        "supersede" => {
            if decision.keep_id.is_none() || decision.supersede_ids.is_none() {
                bail!("LLM returned 'supersede' action but missing keep_id or supersede_ids");
            }
        }
        "keep" => {} // no extra fields needed
        other => bail!("Unknown LLM action: {other}"),
    }

    Ok(decision)
}

/// Apply a consolidation decision to the database.
///
/// Every id the model names is checked against the cluster it was shown and
/// the per-decision supersession cap before anything is written, and the
/// writes for one decision go in a single transaction behind an undo journal
/// entry. A decision that fails validation is rejected whole.
fn apply_decision(
    conn: &Connection,
    cluster: &MemoryCluster,
    decision: &LlmDecision,
    apply: bool,
    project: Option<&str>,
    journal: &UndoJournal,
) -> Result<ActionReport> {
    let cluster_ids: Vec<String> = cluster.memories.iter().map(|m| m.id.clone()).collect();

    match decision.action.as_str() {
        "merge" => {
            let merged_text = decision.merged_text.as_deref().unwrap_or("");
            let supersede_ids = decision.supersede_ids.as_deref().unwrap_or(&[]);

            // The model must name exactly which memories the merged one
            // replaces; the rest of the cluster stays active untouched.
            evolve::validate_fold_ids(cluster, supersede_ids)?;

            if merged_text.trim().is_empty() {
                bail!("LLM returned 'merge' action with empty merged_text");
            }

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
            if apply {
                let tx = conn.unchecked_transaction()?;

                // Slugs come from free text, and `contexts.uri` is UNIQUE — a
                // merge of "Use Zustand ..." would otherwise collide with the
                // very memory it replaces. Disambiguate by run time.
                let slug = format!(
                    "{}-merged-{}",
                    uri::slugify(&merged_text.chars().take(60).collect::<String>()),
                    chrono::Utc::now().format("%Y%m%d%H%M%S"),
                );
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
                    &tx,
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

                // Journal first: if the undo record cannot be written the
                // transaction is dropped and nothing changes.
                journal.append(&UndoEntry::new(
                    "merge",
                    project,
                    supersede_ids,
                    &id,
                    Some(&id),
                ))?;

                for sid in supersede_ids {
                    context::supersede(&tx, sid, &id)?;
                }

                tx.commit()?;
                new_id = Some(id);
            }

            Ok(ActionReport {
                cluster_ids,
                action: "merge".into(),
                reason: decision.reason.clone(),
                applied: apply,
                merged_text: Some(merged_text.to_string()),
                new_id,
                keep_id: None,
                supersede_ids: Some(supersede_ids.to_vec()),
            })
        }
        "supersede" => {
            let keep_id = decision.keep_id.as_deref().unwrap_or("");
            let supersede_ids = decision.supersede_ids.as_deref().unwrap_or(&[]);

            // Validate that all IDs are in the cluster and within the cap
            evolve::validate_supersede_plan(cluster, keep_id, supersede_ids)?;

            if apply {
                let tx = conn.unchecked_transaction()?;

                journal.append(&UndoEntry::new(
                    "supersede",
                    project,
                    supersede_ids,
                    keep_id,
                    None,
                ))?;

                for sid in supersede_ids {
                    context::supersede(&tx, sid, keep_id)?;
                }

                tx.commit()?;
            }

            Ok(ActionReport {
                cluster_ids,
                action: "supersede".into(),
                reason: decision.reason.clone(),
                applied: apply,
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
            applied: false,
            merged_text: None,
            new_id: None,
            keep_id: None,
            supersede_ids: None,
        }),
        other => bail!("Unknown action: {other}"),
    }
}

impl UndoEntry {
    fn new(
        action: &str,
        project: Option<&str>,
        superseded_ids: &[String],
        superseded_by: &str,
        created_id: Option<&str>,
    ) -> Self {
        let quoted = superseded_ids
            .iter()
            .map(|id| format!("'{}'", id.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");

        let mut undo_sql = vec![format!(
            "UPDATE contexts SET superseded_by = NULL WHERE id IN ({quoted});"
        )];
        if let Some(id) = created_id {
            undo_sql.push(format!(
                "DELETE FROM contexts WHERE id = '{}';",
                id.replace('\'', "''")
            ));
        }

        Self {
            at: chrono::Utc::now().to_rfc3339(),
            action: action.to_string(),
            project: project.map(str::to_string),
            superseded_ids: superseded_ids.to_vec(),
            superseded_by: superseded_by.to_string(),
            created_id: created_id.map(str::to_string),
            undo_sql,
        }
    }
}

/// Append-only record of everything an armed run wrote.
///
/// `context::supersede` has no inverse in the CLI, so every supersession is
/// written here — with the SQL that reverses it — before it is committed.
struct UndoJournal {
    path: PathBuf,
}

impl UndoJournal {
    fn for_connection(conn: &Connection) -> Self {
        // Sit next to the database so the journal travels with the data it
        // describes. In-memory connections (tests) fall back to a temp dir.
        let dir = conn
            .path()
            .filter(|p| !p.is_empty())
            .map(Path::new)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);

        let name = format!("evolve-{}.jsonl", chrono::Utc::now().format("%Y%m%d"));
        Self {
            path: dir.join("evolve-undo").join(name),
        }
    }

    fn append(&self, entry: &UndoEntry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create undo journal directory: {}", parent.display())
            })?;
        }

        let line = serde_json::to_string(entry)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("Failed to open undo journal: {}", self.path.display()))?;

        writeln!(file, "{line}")
            .with_context(|| format!("Failed to write undo journal: {}", self.path.display()))?;
        file.sync_all()
            .with_context(|| format!("Failed to flush undo journal: {}", self.path.display()))?;

        Ok(())
    }
}

fn print_report(report: &ActionReport) {
    println!("Cluster: {}", report.cluster_ids.join(", "));
    println!(
        "  Action: {} ({})",
        report.action,
        if report.applied { "applied" } else { "not applied" }
    );
    println!("  Reason: {}", report.reason);
    match report.action.as_str() {
        "merge" => {
            if let Some(text) = &report.merged_text {
                println!("  Merged text: {}", truncate(text, 120));
            }
            if let Some(sids) = &report.supersede_ids {
                println!("  Supersede: {}", sids.join(", "));
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

/// Truncate on a character boundary — merged text comes from an LLM and may
/// hold multi-byte characters, which a byte slice would panic on.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let cut = (0..=max).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    format!("{}...", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rememora::models::context::InsertContext;

    fn memory(conn: &Connection, slug: &str, name: &str) -> String {
        context::insert(
            conn,
            &InsertContext {
                uri: format!("rememora://projects/t/memories/decision/{slug}"),
                parent_uri: Some("rememora://projects/t/memories/decision".into()),
                context_type: "memory".into(),
                category: Some("decision".into()),
                name: name.into(),
                abstract_text: name.into(),
                overview: name.into(),
                content: name.into(),
                tags: "[]".into(),
                source_agent: Some("claude-code".into()),
                source_session: None,
                importance: 0.5,
            },
        )
        .unwrap()
    }

    fn cluster(conn: &Connection, n: usize) -> MemoryCluster {
        let memories = (0..n)
            .map(|i| {
                let id = memory(conn, &format!("m{i}"), &format!("Memory number {i}"));
                context::get_by_id(conn, &id).unwrap().unwrap()
            })
            .collect();
        MemoryCluster { memories }
    }

    fn journal() -> UndoJournal {
        UndoJournal {
            path: std::env::temp_dir()
                .join("rememora-evolve-test")
                .join(format!("undo-{}.jsonl", ulid::Ulid::new())),
        }
    }

    fn active_count(conn: &Connection) -> usize {
        context::list_by_scope(conn, Some("memory"), None, Some("t"), 100)
            .unwrap()
            .len()
    }

    /// The blast radius of one decision is capped no matter what the model
    /// asks for, and an over-cap decision is rejected whole — not truncated,
    /// not partially applied.
    #[test]
    fn test_supersede_cap_is_enforced_and_rejects_whole_decision() {
        let conn = rememora::db::open_memory().unwrap();
        rememora::models::project::add(&conn, "t", Some("/tmp/t"), "t", &[]).unwrap();

        let cluster = cluster(&conn, MAX_SUPERSEDE_PER_DECISION + 2);
        let keep = cluster.memories[0].id.clone();
        let over: Vec<String> = cluster.memories[1..].iter().map(|m| m.id.clone()).collect();
        assert!(over.len() > MAX_SUPERSEDE_PER_DECISION);

        let before = active_count(&conn);
        let decision = LlmDecision {
            action: "supersede".into(),
            reason: "dedup".into(),
            merged_text: None,
            keep_id: Some(keep),
            supersede_ids: Some(over),
        };

        let err = apply_decision(&conn, &cluster, &decision, true, Some("t"), &journal())
            .expect_err("over-cap decision must be rejected");
        assert!(err.to_string().contains("cap"), "unexpected error: {err}");
        assert_eq!(active_count(&conn), before, "nothing may be superseded");
    }

    /// Ids the model invents are never touched.
    #[test]
    fn test_merge_rejects_ids_outside_the_cluster() {
        let conn = rememora::db::open_memory().unwrap();
        rememora::models::project::add(&conn, "t", Some("/tmp/t"), "t", &[]).unwrap();

        let cluster = cluster(&conn, 2);
        let before = active_count(&conn);

        let decision = LlmDecision {
            action: "merge".into(),
            reason: "dedup".into(),
            merged_text: Some("Merged memory".into()),
            keep_id: None,
            supersede_ids: Some(vec![
                cluster.memories[0].id.clone(),
                "01HALLUCINATED0000000000000".into(),
            ]),
        };

        let err = apply_decision(&conn, &cluster, &decision, true, Some("t"), &journal())
            .expect_err("hallucinated id must be rejected");
        assert!(
            err.to_string().contains("not in the cluster"),
            "unexpected error: {err}"
        );
        assert_eq!(active_count(&conn), before, "nothing may be written");
    }

    /// Dry run is the default and it never writes.
    #[test]
    fn test_dry_run_does_not_mutate() {
        let conn = rememora::db::open_memory().unwrap();
        rememora::models::project::add(&conn, "t", Some("/tmp/t"), "t", &[]).unwrap();

        let cluster = cluster(&conn, 3);
        let before = active_count(&conn);

        let decision = LlmDecision {
            action: "supersede".into(),
            reason: "dedup".into(),
            merged_text: None,
            keep_id: Some(cluster.memories[0].id.clone()),
            supersede_ids: Some(vec![cluster.memories[1].id.clone()]),
        };

        let report =
            apply_decision(&conn, &cluster, &decision, false, Some("t"), &journal()).unwrap();
        assert!(!report.applied);
        assert_eq!(active_count(&conn), before);
    }

    /// An armed run writes, and writes an undo record for what it did.
    #[test]
    fn test_applied_supersession_is_journalled() {
        let conn = rememora::db::open_memory().unwrap();
        rememora::models::project::add(&conn, "t", Some("/tmp/t"), "t", &[]).unwrap();

        let cluster = cluster(&conn, 3);
        let keep = cluster.memories[0].id.clone();
        let gone = cluster.memories[1].id.clone();
        let journal = journal();

        let report = apply_decision(
            &conn,
            &cluster,
            &LlmDecision {
                action: "supersede".into(),
                reason: "dedup".into(),
                merged_text: None,
                keep_id: Some(keep.clone()),
                supersede_ids: Some(vec![gone.clone()]),
            },
            true,
            Some("t"),
            &journal,
        )
        .unwrap();

        assert!(report.applied);
        assert_eq!(active_count(&conn), 2, "one memory should be retired");

        let written = std::fs::read_to_string(&journal.path).unwrap();
        assert!(written.contains(&gone), "journal must name the retired id");
        assert!(
            written.contains("superseded_by = NULL"),
            "journal must carry the SQL that reverses the change"
        );

        // The journalled SQL really does put it back.
        for line in written.lines() {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            for sql in entry["undo_sql"].as_array().unwrap() {
                conn.execute_batch(sql.as_str().unwrap()).unwrap();
            }
        }
        assert_eq!(active_count(&conn), 3, "undo SQL must restore the memory");

        let _ = std::fs::remove_file(&journal.path);
    }

    #[test]
    fn test_apply_is_disarmed_by_default() {
        // The environment of a normal CLI run has no REMEMORA_APPLY set.
        assert!(
            !std::env::var(APPLY_ENV).is_ok_and(|v| v == "1"),
            "tests must not run with {APPLY_ENV}=1"
        );
        assert!(!apply_armed());
    }
}
