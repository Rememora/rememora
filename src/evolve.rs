//! Memory evolution: cluster detection and consolidation logic.
//!
//! This module contains the pure library logic for finding clusters of similar
//! memories. The CLI command in `commands/evolve.rs` orchestrates the full
//! pipeline (cluster detection + LLM consolidation + application).

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;

use crate::models::context::ContextRecord;
use crate::search;
use crate::uri;

/// A cluster of related memories that may be candidates for consolidation.
#[derive(Debug)]
pub struct MemoryCluster {
    pub memories: Vec<ContextRecord>,
}

/// Find clusters of similar memories using BM25 cross-search.
///
/// Algorithm:
/// 1. Group memories by category.
/// 2. Within each category, search each memory's key text against others.
/// 3. If BM25 rank indicates similarity (above threshold), link them.
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
                for result in &results {
                    // Skip self-matches
                    if result.context.id == mem.id {
                        continue;
                    }

                    // Only consider results in our current category group
                    if let Some(&j) = id_to_idx.get(result.context.id.as_str()) {
                        // BM25 rank is negative (lower = better match).
                        // Convert to a similarity score: similarity = 1 / (1 + |rank|)
                        // This gives a 0..1 range where higher = more similar.
                        let similarity = 1.0 / (1.0 + result.rank.abs());

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
