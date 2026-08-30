use anyhow::{bail, Context, Result};
use rusqlite::Connection;

use rememora::evolve::{self, MemoryCluster, MAX_CLUSTER_SIZE, MAX_SUPERSEDE_PER_DECISION};
use rememora::models::agent_invocation::{self, Caller};
use rememora::models::context::{self, ContextRecord, InsertContext};
use rememora::uri;

/// Environment variable that arms the destructive path.
///
/// Consolidation supersedes memories, so evolve is dry-run by default and only
/// writes when `--apply` is passed or this is set to `1`. `--dry-run` still
/// wins over both.
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

/// Options for one consolidation run.
pub struct EvolveArgs<'a> {
    pub project: Option<&'a str>,
    /// `--dry-run`: force a dry run. Overrides `--apply` and `REMEMORA_APPLY`.
    pub dry_run: bool,
    /// `--apply`: arm the destructive path for this run.
    pub apply: bool,
    /// `--undo-log`: print the undo journal instead of consolidating anything.
    pub undo_log: bool,
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
- Superseding retires a memory. When in doubt, answer "keep".

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
pub fn run(conn: &Connection, args: &EvolveArgs<'_>, json_output: bool) -> Result<()> {
    if args.undo_log {
        return print_undo_log(conn, args.project, json_output);
    }

    let apply = resolve_apply(args.dry_run, args.apply, apply_armed());
    run_with(conn, args, apply, json_output)
}

/// Decide whether this run is allowed to write.
///
/// Dry run is what happens unless the run is explicitly armed, and `--dry-run`
/// beats every way of arming it. Kept as a pure function of the three inputs so
/// the precedence can be tested without touching the process environment — the
/// only caller is [`run`], so testing it tests the real default.
fn resolve_apply(dry_run: bool, apply_flag: bool, env_armed: bool) -> bool {
    !dry_run && (apply_flag || env_armed)
}

/// True when the operator armed the destructive path through the environment.
fn apply_armed() -> bool {
    std::env::var(APPLY_ENV).is_ok_and(|v| v == "1")
}

fn run_with(
    conn: &Connection,
    args: &EvolveArgs<'_>,
    apply: bool,
    json_output: bool,
) -> Result<()> {
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
        let report = match apply_decision(conn, &cluster, &decision, apply, project) {
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
        if apply {
            println!(
                "\nEvery write above is recorded in the `{UNDO_TABLE}` table inside the\n\
                 database. Read it back with:  rememora evolve --undo-log{}",
                project_flag(project)
            );
        } else {
            println!(
                "\n(dry run — no changes were made)\n\
                 To apply, re-run armed:  rememora evolve{} --apply",
                project_flag(project)
            );
        }
    }

    Ok(())
}

fn project_flag(project: Option<&str>) -> String {
    project
        .map(|p| format!(" --project {p}"))
        .unwrap_or_default()
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
/// writes for one decision — including its undo-journal row — go in a single
/// transaction. A decision that fails validation is rejected whole.
fn apply_decision(
    conn: &Connection,
    cluster: &MemoryCluster,
    decision: &LlmDecision,
    apply: bool,
    project: Option<&str>,
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
                ensure_undo_table(&tx)?;

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

                let undo_sql = supersede_and_journal(&tx, supersede_ids, &id)?;
                append_undo(
                    &tx,
                    &UndoEntry::new("merge", project, supersede_ids, &id, Some(&id), undo_sql),
                )?;

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
                ensure_undo_table(&tx)?;

                let undo_sql = supersede_and_journal(&tx, supersede_ids, keep_id)?;
                append_undo(
                    &tx,
                    &UndoEntry::new("supersede", project, supersede_ids, keep_id, None, undo_sql),
                )?;

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

// --- Undo journal ---------------------------------------------------------

/// Table the undo journal lives in.
pub const UNDO_TABLE: &str = "evolve_undo";

/// The journal used to be a cleartext `evolve-undo/*.jsonl` sidecar next to the
/// database. That put memory ids, project names, actions and timestamps in
/// plaintext beside a deliberately SQLCipher-encrypted store, and nothing ever
/// told the user the file existed. It now lives *in* the database, so the
/// encryption that covers the memories covers the record of what happened to
/// them, and the journal row is written in the same transaction as the writes
/// it reverses: a journal row that cannot be written aborts the whole decision,
/// and a decision that rolls back leaves no journal row claiming it happened.
///
/// Created on demand rather than through `db::migrate` because only this
/// command touches it, and creating it inside the decision transaction keeps
/// it consistent with the rest of the write.
const UNDO_TABLE_DDL: &str = "CREATE TABLE IF NOT EXISTS evolve_undo (
    id             TEXT PRIMARY KEY,
    at             TEXT NOT NULL,
    action         TEXT NOT NULL,
    project        TEXT,
    superseded_ids TEXT NOT NULL,
    superseded_by  TEXT NOT NULL,
    created_id     TEXT,
    undo_sql       TEXT NOT NULL
)";

fn ensure_undo_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(UNDO_TABLE_DDL)
        .context("Failed to create the evolve undo journal table")?;
    Ok(())
}

/// One applied decision, written to the undo journal in the same transaction
/// as the writes it describes.
#[derive(Debug, serde::Serialize)]
struct UndoEntry {
    id: String,
    at: String,
    action: String,
    project: Option<String>,
    /// Memories that were active before this decision and are being retired.
    superseded_ids: Vec<String>,
    /// The memory they now point at.
    superseded_by: String,
    /// Set for "merge": the memory this run created, which did not exist before.
    created_id: Option<String>,
    /// Ready-to-run SQL that reverses this entry exactly.
    undo_sql: Vec<String>,
}

impl UndoEntry {
    fn new(
        action: &str,
        project: Option<&str>,
        superseded_ids: &[String],
        superseded_by: &str,
        created_id: Option<&str>,
        mut undo_sql: Vec<String>,
    ) -> Self {
        if let Some(id) = created_id {
            undo_sql.push(undo_delete_sql(id));
        }

        Self {
            id: ulid::Ulid::new().to_string(),
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

/// Retire `supersede_ids` in favour of `superseded_by` and return the SQL that
/// reverses exactly those writes.
///
/// The reversal is read back from the rows *after* they are updated so the
/// generated statement can pin all three things this run changed: the row, the
/// target it was pointed at, and the `updated_at` stamp this run left on it.
/// Without those predicates, replaying an old journal line would happily
/// un-retire a memory that a later run — or a manual `rememora supersede` —
/// retired for its own reasons.
fn supersede_and_journal(
    tx: &Connection,
    supersede_ids: &[String],
    superseded_by: &str,
) -> Result<Vec<String>> {
    let mut undo_sql = Vec::with_capacity(supersede_ids.len());
    for sid in supersede_ids {
        context::supersede(tx, sid, superseded_by)?;
        let updated_at: String = tx
            .query_row(
                "SELECT updated_at FROM contexts WHERE id = ?1",
                [sid],
                |row| row.get(0),
            )
            .with_context(|| format!("Failed to read back the supersession stamp for {sid}"))?;
        undo_sql.push(undo_supersede_sql(sid, superseded_by, &updated_at));
    }
    Ok(undo_sql)
}

/// SQL that reverses one supersession — and only that one.
fn undo_supersede_sql(id: &str, superseded_by: &str, updated_at: &str) -> String {
    format!(
        "UPDATE contexts SET superseded_by = NULL WHERE id = {} AND superseded_by = {} AND updated_at = {};",
        sql_str(id),
        sql_str(superseded_by),
        sql_str(updated_at),
    )
}

/// SQL that removes a memory this run created.
///
/// Guarded by `NOT EXISTS`: if the un-supersede above was refused (because the
/// rows have moved on since), deleting the merge target would orphan them, so
/// the delete refuses too.
fn undo_delete_sql(created_id: &str) -> String {
    format!(
        "DELETE FROM contexts WHERE id = {id} AND NOT EXISTS (SELECT 1 FROM contexts WHERE superseded_by = {id});",
        id = sql_str(created_id),
    )
}

fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn append_undo(tx: &Connection, entry: &UndoEntry) -> Result<()> {
    tx.execute(
        "INSERT INTO evolve_undo
            (id, at, action, project, superseded_ids, superseded_by, created_id, undo_sql)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            entry.id,
            entry.at,
            entry.action,
            entry.project,
            serde_json::to_string(&entry.superseded_ids)?,
            entry.superseded_by,
            entry.created_id,
            serde_json::to_string(&entry.undo_sql)?,
        ],
    )
    .context("Failed to write the evolve undo journal")?;
    Ok(())
}

fn undo_entries(conn: &Connection, project: Option<&str>) -> Result<Vec<UndoEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, at, action, project, superseded_ids, superseded_by, created_id, undo_sql
           FROM evolve_undo
          WHERE (?1 IS NULL OR project = ?1)
          ORDER BY at DESC, id DESC",
    )?;

    let rows = stmt.query_map(rusqlite::params![project], |row| {
        let ids: String = row.get(4)?;
        let sql: String = row.get(7)?;
        Ok(UndoEntry {
            id: row.get(0)?,
            at: row.get(1)?,
            action: row.get(2)?,
            project: row.get(3)?,
            superseded_ids: serde_json::from_str(&ids).unwrap_or_default(),
            superseded_by: row.get(5)?,
            created_id: row.get(6)?,
            undo_sql: serde_json::from_str(&sql).unwrap_or_default(),
        })
    })?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Print the undo journal so an operator can reverse a run by hand.
fn print_undo_log(conn: &Connection, project: Option<&str>, json_output: bool) -> Result<()> {
    ensure_undo_table(conn)?;
    let entries = undo_entries(conn, project)?;

    if json_output {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No consolidation has been applied — the undo journal is empty.");
        return Ok(());
    }

    println!(
        "Undo journal ({} entr{}), newest first. Lives in the `{UNDO_TABLE}` table\n\
         inside the database — under the same encryption as the memories, and\n\
         with no cleartext sidecar file.\n",
        entries.len(),
        if entries.len() == 1 { "y" } else { "ies" },
    );
    for entry in &entries {
        println!("{}  {}  [{}]", entry.at, entry.action, entry.id);
        if let Some(p) = &entry.project {
            println!("  Project:   {p}");
        }
        println!("  Retired:   {}", entry.superseded_ids.join(", "));
        println!("  Points at: {}", entry.superseded_by);
        if let Some(created) = &entry.created_id {
            println!("  Created:   {created}");
        }
        println!("  Undo SQL:");
        for sql in &entry.undo_sql {
            println!("    {sql}");
        }
        println!();
    }

    Ok(())
}

// --- Reporting ------------------------------------------------------------

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

    fn active_count(conn: &Connection) -> usize {
        context::list_by_scope(conn, Some("memory"), None, Some("t"), 100)
            .unwrap()
            .len()
    }

    fn test_db() -> Connection {
        let conn = rememora::db::open_memory().unwrap();
        rememora::models::project::add(&conn, "t", Some("/tmp/t"), "t", &[]).unwrap();
        conn
    }

    fn run_undo(conn: &Connection, entries: &[UndoEntry]) {
        for entry in entries {
            for sql in &entry.undo_sql {
                conn.execute_batch(sql).unwrap();
            }
        }
    }

    /// The blast radius of one decision is capped no matter what the model
    /// asks for, and an over-cap decision is rejected whole — not truncated,
    /// not partially applied.
    #[test]
    fn test_supersede_cap_is_enforced_and_rejects_whole_decision() {
        let conn = test_db();

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

        let err = apply_decision(&conn, &cluster, &decision, true, Some("t"))
            .expect_err("over-cap decision must be rejected");
        assert!(err.to_string().contains("cap"), "unexpected error: {err}");
        assert_eq!(active_count(&conn), before, "nothing may be superseded");
    }

    /// Ids the model invents are never touched.
    #[test]
    fn test_merge_rejects_ids_outside_the_cluster() {
        let conn = test_db();

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

        let err = apply_decision(&conn, &cluster, &decision, true, Some("t"))
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
        let conn = test_db();

        let cluster = cluster(&conn, 3);
        let before = active_count(&conn);

        let decision = LlmDecision {
            action: "supersede".into(),
            reason: "dedup".into(),
            merged_text: None,
            keep_id: Some(cluster.memories[0].id.clone()),
            supersede_ids: Some(vec![cluster.memories[1].id.clone()]),
        };

        let report = apply_decision(&conn, &cluster, &decision, false, Some("t")).unwrap();
        assert!(!report.applied);
        assert_eq!(active_count(&conn), before);
        // A dry run must not even create the journal table, let alone a row.
        assert!(!undo_table_exists(&conn), "dry run created the journal table");
    }

    fn undo_table_exists(conn: &Connection) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='evolve_undo')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    /// An armed run writes, records an undo row in the same database, and the
    /// recorded SQL really does put the memory back.
    #[test]
    fn test_applied_supersession_is_journalled_in_the_database() {
        let conn = test_db();

        let cluster = cluster(&conn, 3);
        let keep = cluster.memories[0].id.clone();
        let gone = cluster.memories[1].id.clone();

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
        )
        .unwrap();

        assert!(report.applied);
        assert_eq!(active_count(&conn), 2, "one memory should be retired");

        let entries = undo_entries(&conn, Some("t")).unwrap();
        assert_eq!(entries.len(), 1, "one decision, one journal row");
        assert_eq!(entries[0].superseded_ids, vec![gone.clone()]);
        assert_eq!(entries[0].superseded_by, keep);
        assert!(entries[0].created_id.is_none());

        run_undo(&conn, &entries);
        assert_eq!(active_count(&conn), 3, "undo SQL must restore the memory");
    }

    /// The merge path journals the created memory too, and its undo removes it.
    #[test]
    fn test_applied_merge_is_journalled_and_reversible() {
        let conn = test_db();

        let cluster = cluster(&conn, 3);
        let folded: Vec<String> = cluster.memories[..2].iter().map(|m| m.id.clone()).collect();

        let report = apply_decision(
            &conn,
            &cluster,
            &LlmDecision {
                action: "merge".into(),
                reason: "dedup".into(),
                merged_text: Some("Memories zero and one say the same thing".into()),
                keep_id: None,
                supersede_ids: Some(folded.clone()),
            },
            true,
            Some("t"),
        )
        .unwrap();

        let new_id = report.new_id.clone().expect("merge must create a memory");
        // 3 originals - 2 retired + 1 created
        assert_eq!(active_count(&conn), 2);

        let entries = undo_entries(&conn, Some("t")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].created_id.as_deref(), Some(new_id.as_str()));

        run_undo(&conn, &entries);
        assert_eq!(active_count(&conn), 3, "undo must restore the original three");
        assert!(
            context::get_by_id(&conn, &new_id).unwrap().is_none(),
            "undo must delete the memory the merge created"
        );
    }

    /// Replay guard: an old journal line must not un-retire a memory that a
    /// later, unrelated supersession retired.
    ///
    /// Before the guard the undo statement was
    /// `UPDATE contexts SET superseded_by = NULL WHERE id IN (...)` with no tie
    /// to the run that produced it, so replaying it reversed whatever the id
    /// happened to be pointing at.
    #[test]
    fn test_replaying_a_stale_journal_entry_does_not_undo_a_later_supersession() {
        let conn = test_db();

        let cluster = cluster(&conn, 3);
        let keep = cluster.memories[0].id.clone();
        let gone = cluster.memories[1].id.clone();
        let other = cluster.memories[2].id.clone();

        apply_decision(
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
        )
        .unwrap();

        let entries = undo_entries(&conn, Some("t")).unwrap();

        // The operator reverses the run by hand...
        run_undo(&conn, &entries);
        assert_eq!(active_count(&conn), 3);

        // ...then later retires the same memory deliberately, for a different
        // reason and against a different target.
        context::supersede(&conn, &gone, &other).unwrap();
        assert_eq!(active_count(&conn), 2);

        // Replaying the stale journal line must be a no-op.
        run_undo(&conn, &entries);
        assert_eq!(
            active_count(&conn),
            2,
            "a stale journal line must not resurrect a deliberately retired memory"
        );
        let row = context::get_by_id(&conn, &gone).unwrap().unwrap();
        assert_eq!(row.superseded_by.as_deref(), Some(other.as_str()));
    }

    /// A journal write that fails takes the whole decision with it: the
    /// supersession and its undo record are one transaction, so there is never
    /// a retired memory with no way back recorded.
    #[test]
    fn test_journal_failure_rolls_back_the_supersession() {
        let conn = test_db();
        // Squat on the journal table name with an incompatible shape so the
        // INSERT inside the decision transaction fails.
        conn.execute_batch("CREATE TABLE evolve_undo (nope INTEGER NOT NULL)")
            .unwrap();

        let cluster = cluster(&conn, 3);
        let before = active_count(&conn);

        let err = apply_decision(
            &conn,
            &cluster,
            &LlmDecision {
                action: "supersede".into(),
                reason: "dedup".into(),
                merged_text: None,
                keep_id: Some(cluster.memories[0].id.clone()),
                supersede_ids: Some(vec![cluster.memories[1].id.clone()]),
            },
            true,
            Some("t"),
        )
        .expect_err("a journal failure must fail the decision");
        assert!(
            err.to_string().contains("undo journal"),
            "unexpected error: {err}"
        );
        assert_eq!(
            active_count(&conn),
            before,
            "the supersession must roll back with the journal"
        );
    }

    /// The precedence the CLI actually uses: dry run unless armed, and
    /// `--dry-run` beats every way of arming it.
    #[test]
    fn test_apply_precedence() {
        // dry_run, apply_flag, env_armed -> writes?
        let cases = [
            (false, false, false, false), // the default: nothing is armed
            (false, true, false, true),   // --apply
            (false, false, true, true),   // REMEMORA_APPLY=1
            (false, true, true, true),
            (true, false, false, false),
            (true, true, false, false), // --dry-run beats --apply
            (true, false, true, false), // --dry-run beats the env var
            (true, true, true, false),
        ];
        for (dry_run, apply_flag, env_armed, expected) in cases {
            assert_eq!(
                resolve_apply(dry_run, apply_flag, env_armed),
                expected,
                "dry_run={dry_run} apply_flag={apply_flag} env_armed={env_armed}"
            );
        }
    }

    /// `apply_armed` reads the environment, and an unset variable is disarmed.
    #[test]
    fn test_apply_env_requires_exactly_one() {
        assert!(!apply_armed(), "tests must not run with {APPLY_ENV}=1");
    }
}
