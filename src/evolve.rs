//! Memory evolution: cluster detection and consolidation logic.
//!
//! This module contains the pure library logic for finding clusters of similar
//! memories. The CLI command in `commands/evolve.rs` orchestrates the full
//! pipeline (cluster detection + LLM consolidation + application).

use anyhow::{bail, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

use crate::models::context::ContextRecord;
use crate::search;
use crate::uri;

/// A cluster of related memories that may be candidates for consolidation.
#[derive(Debug)]
pub struct MemoryCluster {
    pub memories: Vec<ContextRecord>,
}

/// Largest cluster a single automated decision is allowed to act on.
///
/// Clustering is transitive (union-find), so one spurious edge can chain
/// unrelated memories together. Consolidation is irreversible, so anything
/// bigger than this is handed back for a human to look at instead of being
/// sent to a model that can supersede all of it at once.
pub const MAX_CLUSTER_SIZE: usize = 8;

/// Largest number of memories one consolidation decision may supersede.
///
/// This is the blast radius of a single LLM call. It is deliberately much
/// smaller than `MAX_CLUSTER_SIZE` so that even a fully-hallucinated decision
/// can only retire a handful of records.
pub const MAX_SUPERSEDE_PER_DECISION: usize = 5;

/// Magnitude below which a BM25 rank is noise rather than a match.
///
/// FTS5 clamps non-positive IDF — a term present in more than half the
/// documents — to 1e-6, so a hit whose entire score comes from such terms
/// lands around 1e-6..1e-5. A hit carrying even one mildly discriminating
/// term measures ~1 in the same corpus. This floor sits in that empty band,
/// five orders of magnitude above the clamped tier and an order of magnitude
/// below the weakest real match, so it rejects stopword-only hits without
/// being tuned against any particular corpus. It is a degeneracy guard, not
/// the similarity threshold — `min_similarity` still does the deciding.
const MIN_MATCH_RANK: f64 = 0.1;

/// Convert a raw FTS5 BM25 `rank` into a 0..1 similarity, relative to the
/// self-match rank of the query that produced it.
///
/// FTS5 `rank` is negative and *more* negative means a better match. Its
/// magnitude is unbounded and scales with corpus size, document length and
/// query length, so no fixed constant can turn a raw rank into a meaningful
/// 0..1 score — that is exactly what the previous `1 / (1 + |rank|)` mapping
/// got wrong, and it inverted the ordering on top of it: a stopword-only hit
/// (rank -1e-6) scored 1.0 while a genuine near-duplicate (rank -5) scored
/// 0.17.
///
/// Dividing by the query's own self-match rank cancels every per-query and
/// per-corpus factor and asks a question that is comparable across queries:
/// "how close does this document come to scoring as well as the record the
/// query was built from?" 1.0 means it matches the query's own text as well
/// as the query memory itself does.
///
/// Returns 0.0 when either rank is in the degenerate near-zero regime (see
/// `MIN_MATCH_RANK`) or is not a negative, finite BM25 score.
pub fn bm25_similarity(rank: f64, self_rank: f64) -> f64 {
    let magnitude = -rank;
    let self_magnitude = -self_rank;

    if !magnitude.is_finite() || !self_magnitude.is_finite() {
        return 0.0;
    }
    if magnitude < MIN_MATCH_RANK || self_magnitude < MIN_MATCH_RANK {
        return 0.0;
    }

    (magnitude / self_magnitude).clamp(0.0, 1.0)
}

/// True when a cluster is too large for a single automated decision.
pub fn is_oversized(cluster: &MemoryCluster) -> bool {
    cluster.memories.len() > MAX_CLUSTER_SIZE
}

/// Validate the ids a consolidation decision wants to fold away.
///
/// Every id must name a memory that is actually in the cluster the model was
/// shown, the list must be non-empty and free of duplicates, and it must not
/// exceed `MAX_SUPERSEDE_PER_DECISION`. A decision that fails any of these is
/// rejected outright — it is never partially applied.
pub fn validate_fold_ids(cluster: &MemoryCluster, supersede_ids: &[String]) -> Result<()> {
    if supersede_ids.is_empty() {
        bail!("decision names no ids to supersede");
    }
    if supersede_ids.len() > MAX_SUPERSEDE_PER_DECISION {
        bail!(
            "decision would supersede {} memories, over the cap of {}",
            supersede_ids.len(),
            MAX_SUPERSEDE_PER_DECISION
        );
    }

    let cluster_ids: HashSet<&str> = cluster.memories.iter().map(|m| m.id.as_str()).collect();
    let mut seen: HashSet<&str> = HashSet::new();

    for sid in supersede_ids {
        if !cluster_ids.contains(sid.as_str()) {
            bail!("supersede id '{sid}' is not in the cluster");
        }
        if !seen.insert(sid.as_str()) {
            bail!("supersede id '{sid}' is listed twice");
        }
    }

    Ok(())
}

/// Validate a "supersede" decision: one cluster member survives, the named
/// others are folded into it.
pub fn validate_supersede_plan(
    cluster: &MemoryCluster,
    keep_id: &str,
    supersede_ids: &[String],
) -> Result<()> {
    let cluster_ids: HashSet<&str> = cluster.memories.iter().map(|m| m.id.as_str()).collect();

    if !cluster_ids.contains(keep_id) {
        bail!("keep_id '{keep_id}' is not in the cluster");
    }
    if supersede_ids.iter().any(|s| s == keep_id) {
        bail!("keep_id '{keep_id}' is also listed for supersession");
    }

    validate_fold_ids(cluster, supersede_ids)
}

/// Find clusters of similar memories using BM25 cross-search.
///
/// Algorithm:
/// 1. Group memories by category.
/// 2. Within each category, search each memory's key text against others.
/// 3. Score each hit relative to that query's own self-match (see
///    `bm25_similarity`) and link the pair when the score clears
///    `min_similarity`.
/// 4. Use union-find to form connected clusters of 2+ memories.
pub fn find_clusters(
    conn: &Connection,
    memories: Vec<ContextRecord>,
    min_similarity: f64,
) -> Result<Vec<MemoryCluster>> {
    // Group by category
    let mut by_category: HashMap<String, Vec<ContextRecord>> = HashMap::new();
    for mem in memories {
        let cat = mem
            .category
            .clone()
            .unwrap_or_else(|| "uncategorized".into());
        by_category.entry(cat).or_default().push(mem);
    }

    let mut all_clusters = Vec::new();

    for mems in by_category.values() {
        if mems.len() < 2 {
            continue;
        }

        // Build id-to-index map
        let id_to_idx: HashMap<&str, usize> = mems
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.as_str(), i))
            .collect();

        // Union-find parent array
        let n = mems.len();
        let mut parent: Vec<usize> = (0..n).collect();

        // For each memory, search its text against all others via BM25
        for (i, mem) in mems.iter().enumerate() {
            let query_text = build_search_query(mem);
            if query_text.is_empty() {
                continue;
            }

            // Extract project from URI for scoped search
            let project = uri::extract_project(&mem.uri);

            let results = search::search(
                conn,
                &query_text,
                project.as_deref(),
                mem.category.as_deref(),
                n,
            );

            if let Ok(results) = results {
                // The self-match is the normalisation anchor: the score this
                // query achieves against the record it was built from. Without
                // it there is no scale to judge the other hits on, so the query
                // is skipped entirely — a missing anchor must never be allowed
                // to manufacture cluster edges.
                let Some(self_rank) = results
                    .iter()
                    .find(|r| r.context.id == mem.id)
                    .map(|r| r.rank)
                else {
                    continue;
                };

                for result in &results {
                    // Skip self-matches
                    if result.context.id == mem.id {
                        continue;
                    }

                    // Only consider results in our current category group
                    if let Some(&j) = id_to_idx.get(result.context.id.as_str()) {
                        let similarity = bm25_similarity(result.rank, self_rank);

                        if similarity >= min_similarity {
                            union(&mut parent, i, j);
                        }
                    }
                }
            }
        }

        // Collect clusters
        let mut cluster_map: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            let root = find(&mut parent, i);
            cluster_map.entry(root).or_default().push(i);
        }

        for indices in cluster_map.values() {
            if indices.len() >= 2 {
                let cluster_mems: Vec<ContextRecord> =
                    indices.iter().map(|&i| mems[i].clone()).collect();
                all_clusters.push(MemoryCluster {
                    memories: cluster_mems,
                });
            }
        }
    }

    Ok(all_clusters)
}

/// Build a concise search query from a memory's text fields.
fn build_search_query(mem: &ContextRecord) -> String {
    // Use the name (most distinctive text) as the search query.
    // Limit to significant words for better BM25 matching.
    let words: Vec<&str> = mem
        .name
        .split_whitespace()
        .filter(|w| w.len() > 2) // skip short words
        .take(8) // limit query length
        .collect();
    words.join(" ")
}

// --- Union-Find helpers ---

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]]; // path compression
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let ra = find(parent, a);
    let rb = find(parent, b);
    if ra != rb {
        parent[rb] = ra;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, name: &str) -> ContextRecord {
        ContextRecord {
            id: id.into(),
            uri: format!("rememora://projects/test/memories/decisions/{id}"),
            parent_uri: None,
            context_type: "memory".into(),
            category: Some("decision".into()),
            name: name.into(),
            abstract_text: String::new(),
            overview: String::new(),
            content: String::new(),
            tags: "[]".into(),
            source_agent: None,
            source_session: None,
            importance: 0.5,
            active_count: 0,
            created_at: String::new(),
            updated_at: String::new(),
            superseded_by: None,
        }
    }

    fn cluster_of(n: usize) -> MemoryCluster {
        MemoryCluster {
            memories: (0..n)
                .map(|i| record(&format!("id{i}"), &format!("memory {i}")))
                .collect(),
        }
    }

    fn ids(range: std::ops::Range<usize>) -> Vec<String> {
        range.map(|i| format!("id{i}")).collect()
    }

    /// Measured against a real FTS5 corpus: a genuine near-duplicate scores
    /// rank -4.96 against a self-match of -6.02, while a hit that shares only
    /// a term present in most documents scores -1.5e-6. The old mapping
    /// `1 / (1 + |rank|)` ranked the stopword hit (1.0000) *above* the real
    /// match (0.1677); the relative mapping must not.
    #[test]
    fn test_bm25_similarity_ranks_strong_match_above_stopword_match() {
        const SELF_RANK: f64 = -6.02;
        const STRONG: f64 = -4.96;
        const STOPWORD: f64 = -0.0000015;

        let old = |rank: f64| 1.0 / (1.0 + rank.abs());
        assert!(
            old(STRONG) < old(STOPWORD),
            "sanity: the old mapping really was inverted"
        );

        let strong = bm25_similarity(STRONG, SELF_RANK);
        let stopword = bm25_similarity(STOPWORD, SELF_RANK);

        assert!(strong > stopword, "strong={strong}, stopword={stopword}");
        assert!(strong > 0.5, "strong match should clear a sane threshold");
        assert_eq!(stopword, 0.0, "stopword-tier hits are rejected by the floor");
    }

    #[test]
    fn test_bm25_similarity_is_scale_free() {
        // Doubling every rank in a query (a bigger corpus, a longer query)
        // must not change the relative judgement.
        let a = bm25_similarity(-4.0, -8.0);
        let b = bm25_similarity(-40.0, -80.0);
        assert!((a - b).abs() < 1e-12, "a={a}, b={b}");
        assert!((a - 0.5).abs() < 1e-12, "a={a}");
    }

    #[test]
    fn test_bm25_similarity_clamps_and_rejects_degenerate_ranks() {
        // A hit that beats the query's own record is capped at 1.0.
        assert_eq!(bm25_similarity(-9.0, -6.0), 1.0);
        // A degenerate anchor cannot manufacture similarity.
        assert_eq!(bm25_similarity(-0.05, -0.05), 0.0);
        // Positive / zero ranks are not BM25 scores.
        assert_eq!(bm25_similarity(1.0, -6.0), 0.0);
        assert_eq!(bm25_similarity(f64::NAN, -6.0), 0.0);
    }

    #[test]
    fn test_fold_ids_capped() {
        let cluster = cluster_of(MAX_CLUSTER_SIZE);

        assert!(validate_fold_ids(&cluster, &ids(0..MAX_SUPERSEDE_PER_DECISION)).is_ok());

        let over = validate_fold_ids(&cluster, &ids(0..MAX_SUPERSEDE_PER_DECISION + 1));
        let err = over.expect_err("over-cap decision must be rejected").to_string();
        assert!(err.contains("cap"), "unexpected error: {err}");
    }

    #[test]
    fn test_fold_ids_reject_unknown_and_duplicate_ids() {
        let cluster = cluster_of(3);

        assert!(validate_fold_ids(&cluster, &["not-in-cluster".into()]).is_err());
        assert!(validate_fold_ids(&cluster, &["id0".into(), "id0".into()]).is_err());
        assert!(validate_fold_ids(&cluster, &[]).is_err());
    }

    #[test]
    fn test_supersede_plan_requires_keep_id_in_cluster() {
        let cluster = cluster_of(3);

        assert!(validate_supersede_plan(&cluster, "id0", &["id1".into()]).is_ok());
        assert!(validate_supersede_plan(&cluster, "ghost", &["id1".into()]).is_err());
        assert!(validate_supersede_plan(&cluster, "id0", &["id0".into()]).is_err());
    }

    #[test]
    fn test_union_find_basic() {
        let mut parent: Vec<usize> = (0..5).collect();
        union(&mut parent, 0, 1);
        union(&mut parent, 2, 3);
        union(&mut parent, 1, 3);

        assert_eq!(find(&mut parent, 0), find(&mut parent, 3));
        assert_ne!(find(&mut parent, 0), find(&mut parent, 4));
    }

    #[test]
    fn test_build_search_query() {
        let mem = ContextRecord {
            id: "test".into(),
            uri: "rememora://projects/test/memories/decisions/foo".into(),
            parent_uri: None,
            context_type: "memory".into(),
            category: Some("decision".into()),
            name: "Use Zustand for state management in the app".into(),
            abstract_text: String::new(),
            overview: String::new(),
            content: String::new(),
            tags: "[]".into(),
            source_agent: None,
            source_session: None,
            importance: 0.5,
            active_count: 0,
            created_at: String::new(),
            updated_at: String::new(),
            superseded_by: None,
        };
        let query = build_search_query(&mem);
        assert!(query.contains("Zustand"));
        assert!(query.contains("state"));
    }
}
