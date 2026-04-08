use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

use crate::models::context;
use crate::search::SearchResult;
use crate::uri;

/// Configuration for hierarchical score propagation.
pub struct PropagationConfig {
    /// Score multiplier per hop (default 0.3).
    pub decay_factor: f64,
    /// Maximum hops to propagate (default 2).
    pub max_depth: usize,
}

impl Default for PropagationConfig {
    fn default() -> Self {
        Self {
            decay_factor: 0.3,
            max_depth: 2,
        }
    }
}

/// Convert a negative BM25 rank to a positive 0..1 score.
/// BM25 ranks are negative (lower = better match), so we normalize:
/// `score = 1.0 / (1.0 + |rank|)`
pub fn normalize_bm25_rank(rank: f64) -> f64 {
    1.0 / (1.0 + rank.abs())
}

/// Walk the URI tree and collect related contexts at each depth level.
///
/// Returns (ContextRecord, depth) pairs for parents, children, and siblings
/// reachable within `max_depth` hops from the given URI.
fn find_related(
    conn: &Connection,
    uri_str: &str,
    max_depth: usize,
) -> Result<Vec<(context::ContextRecord, usize)>> {
    let mut related = Vec::new();
    if max_depth == 0 {
        return Ok(related);
    }

    // Depth 1: parent
    if let Some(parent_uri) = uri::parent(uri_str)? {
        if let Some(parent_ctx) = context::get_by_uri(conn, &parent_uri)? {
            if parent_ctx.superseded_by.is_none() {
                related.push((parent_ctx, 1));
            }
        }

        // Depth 1: children (contexts whose parent_uri is this context's URI)
        let children = context::list_by_parent(conn, uri_str)?;
        for child in children {
            related.push((child, 1));
        }

        // Depth 2: siblings (other children of the same parent, excluding self)
        if max_depth >= 2 {
            let siblings = context::list_by_parent(conn, &parent_uri)?;
            for sib in siblings {
                if sib.uri != uri_str {
                    related.push((sib, 2));
                }
            }

            // Depth 2: grandparent
            if let Some(grandparent_uri) = uri::parent(&parent_uri)? {
                if let Some(gp_ctx) = context::get_by_uri(conn, &grandparent_uri)? {
                    if gp_ctx.superseded_by.is_none() {
                        related.push((gp_ctx, 2));
                    }
                }
            }
        }
    } else {
        // No parent — still check for children at depth 1
        let children = context::list_by_parent(conn, uri_str)?;
        for child in children {
            related.push((child, 1));
        }
    }

    Ok(related)
}

/// Apply hierarchical score propagation to search results.
///
/// For each direct match, walks the URI tree to find related contexts and
/// assigns propagated scores using `base_score * decay^depth`. Merges with
/// direct matches using max-score dedup, then re-sorts descending by score.
pub fn propagate_scores(
    conn: &Connection,
    results: Vec<SearchResult>,
    config: &PropagationConfig,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Map context_id → (best_score, SearchResult, is_direct_match)
    let mut merged: HashMap<String, (f64, SearchResult, bool)> = HashMap::new();

    // Insert all direct matches first
    for result in results {
        let score = normalize_bm25_rank(result.rank);
        merged.insert(
            result.context.id.clone(),
            (score, result, true),
        );
    }

    // Collect propagation candidates from all direct matches
    let direct_entries: Vec<(String, f64)> = merged
        .iter()
        .map(|(_, (score, r, _))| (r.context.uri.clone(), *score))
        .collect();

    for (uri_str, base_score) in &direct_entries {
        let related = find_related(conn, uri_str, config.max_depth)?;
        for (ctx, depth) in related {
            let propagated_score = base_score * config.decay_factor.powi(depth as i32);
            let id = ctx.id.clone();

            merged
                .entry(id)
                .and_modify(|(existing_score, _, _)| {
                    // Additive: propagated score stacks with existing score
                    *existing_score += propagated_score;
                })
                .or_insert_with(|| {
                    (
                        propagated_score,
                        SearchResult {
                            context: ctx,
                            rank: 0.0, // will be overwritten
                        },
                        false,
                    )
                });
        }
    }

    // Convert to sorted vec — use the merged score as the rank (positive, higher = better)
    let mut fused: Vec<SearchResult> = merged
        .into_values()
        .map(|(score, mut r, _)| {
            r.rank = score;
            r
        })
        .collect();

    fused.sort_by(|a, b| b.rank.partial_cmp(&a.rank).unwrap_or(std::cmp::Ordering::Equal));
    fused.truncate(limit);

    Ok(fused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_bm25_rank_converts_negative_to_positive() {
        // BM25 rank of -10 → score of 1/(1+10) ≈ 0.0909
        let score = normalize_bm25_rank(-10.0);
        assert!((score - 1.0 / 11.0).abs() < 1e-10);
    }

    #[test]
    fn normalize_bm25_rank_zero_gives_one() {
        assert!((normalize_bm25_rank(0.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn normalize_bm25_rank_large_negative_gives_small_positive() {
        let score = normalize_bm25_rank(-100.0);
        assert!(score > 0.0 && score < 0.02);
    }
}
