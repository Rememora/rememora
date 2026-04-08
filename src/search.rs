use anyhow::Result;
use rusqlite::Connection;

use crate::models::context::{self, ContextRecord};

pub struct SearchResult {
    pub context: ContextRecord,
    pub rank: f64,
}

pub fn search(
    conn: &Connection,
    query: &str,
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // Build FTS5 query — escape special characters
    let fts_query = query
        .replace('"', "\"\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" OR ");

    if fts_query.is_empty() {
        return Ok(vec![]);
    }

    let mut sql = String::from(
        "SELECT c.id, c.uri, c.parent_uri, c.context_type, c.category, c.name,
                c.abstract, c.overview, c.content, c.tags, c.source_agent,
                c.source_session, c.importance, c.active_count, c.created_at,
                c.updated_at, c.superseded_by, rank
         FROM contexts_fts fts
         JOIN contexts c ON c.rowid = fts.rowid
         WHERE contexts_fts MATCH ?1
         AND c.superseded_by IS NULL",
    );

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(fts_query));
    let mut param_idx = 2;

    if let Some(proj) = project {
        let prefix = format!("rememora://projects/{proj}/");
        sql.push_str(&format!(" AND c.uri LIKE ?{param_idx}"));
        param_values.push(Box::new(format!("{prefix}%")));
        param_idx += 1;

        // Also include global memories
        sql = sql.replace(
            &format!("AND c.uri LIKE ?{}", param_idx - 1),
            &format!("AND (c.uri LIKE ?{} OR c.uri LIKE 'rememora://global/%')", param_idx - 1),
        );
    }

    if let Some(cat) = category {
        sql.push_str(&format!(" AND c.category = ?{param_idx}"));
        param_values.push(Box::new(cat.to_string()));
        param_idx += 1;
    }

    let _ = param_idx;
    sql.push_str(" ORDER BY rank");
    sql.push_str(&format!(" LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            let rank: f64 = row.get(17)?;
            Ok(SearchResult {
                context: ContextRecord {
                    id: row.get(0)?,
                    uri: row.get(1)?,
                    parent_uri: row.get(2)?,
                    context_type: row.get(3)?,
                    category: row.get(4)?,
                    name: row.get(5)?,
                    abstract_text: row.get(6)?,
                    overview: row.get(7)?,
                    content: row.get(8)?,
                    tags: row.get(9)?,
                    source_agent: row.get(10)?,
                    source_session: row.get(11)?,
                    importance: row.get(12)?,
                    active_count: row.get(13)?,
                    created_at: row.get(14)?,
                    updated_at: row.get(15)?,
                    superseded_by: row.get(16)?,
                },
                rank,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Bump active_count for returned results
    for result in &rows {
        context::bump_active_count(conn, &result.context.id)?;
    }

    Ok(rows)
}

/// Search with hierarchical score propagation.
///
/// Runs a normal BM25 search with an expanded limit (3x), then applies
/// URI-tree propagation to boost related contexts. Results are re-sorted
/// by propagated score (positive, higher = better).
pub fn search_with_propagation(
    conn: &Connection,
    query: &str,
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
    config: &crate::propagate::PropagationConfig,
) -> Result<Vec<SearchResult>> {
    let expanded_limit = limit * 3;
    let results = search(conn, query, project, category, expanded_limit)?;
    crate::propagate::propagate_scores(conn, results, config, limit)
}

/// Store a precomputed embedding for a context in the context_embeddings table.
pub fn store_embedding(
    conn: &Connection,
    context_id: &str,
    embedding: &[f32],
    model_name: &str,
) -> Result<()> {
    let blob: Vec<u8> = embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT OR REPLACE INTO context_embeddings (context_id, embedding, dimensions, model_name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![context_id, blob, embedding.len() as i64, model_name, now],
    )?;

    // Also insert into sqlite-vec virtual table when feature is enabled.
    // vec0 doesn't support INSERT OR REPLACE, so delete first if exists.
    #[cfg(feature = "embed-candle")]
    {
        conn.execute(
            "DELETE FROM vec_contexts WHERE context_id = ?1",
            rusqlite::params![context_id],
        )?;
        conn.execute(
            "INSERT INTO vec_contexts (context_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![context_id, blob],
        )?;
    }

    Ok(())
}

/// Hybrid search combining BM25 (FTS5) and vector cosine similarity via
/// Reciprocal Rank Fusion (RRF).
///
/// `query_embedding` is the vector representation of the search query.
/// When the `embed-candle` feature is not enabled, this falls back to
/// BM25-only search.
pub fn hybrid_search(
    conn: &Connection,
    query: &str,
    #[allow(unused_variables)] query_embedding: Option<&[f32]>,
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    // BM25 results — fetch more than limit to allow fusion
    let fuse_pool = limit * 3;
    let bm25_results = search(conn, query, project, category, fuse_pool)?;

    #[cfg(feature = "embed-candle")]
    if let Some(qe) = query_embedding {
        let vec_results = vector_search(conn, qe, project, category, fuse_pool)?;
        // Bump active_count for vector-only results not already bumped by BM25 search
        let bm25_ids: std::collections::HashSet<&str> = bm25_results
            .iter()
            .map(|r| r.context.id.as_str())
            .collect();
        for result in &vec_results {
            if !bm25_ids.contains(result.context.id.as_str()) {
                context::bump_active_count(conn, &result.context.id)?;
            }
        }
        return reciprocal_rank_fusion(bm25_results, vec_results, limit);
    }

    // Without embeddings, just return BM25 results truncated to limit
    Ok(bm25_results.into_iter().take(limit).collect())
}

/// Vector-only search using sqlite-vec cosine distance.
///
/// Uses a CTE to isolate the KNN MATCH query from post-filters on the
/// `contexts` table — sqlite-vec's query planner requires MATCH to be the
/// sole constraint on the virtual table. Over-fetches (limit * 5) to
/// compensate for rows filtered out by project/category/superseded checks.
#[cfg(feature = "embed-candle")]
fn vector_search(
    conn: &Connection,
    query_embedding: &[f32],
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let blob: Vec<u8> = query_embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();

    // Over-fetch from the vector index, then filter in the outer query
    let k = limit * 5;

    let mut sql = String::from(
        "WITH knn AS (
            SELECT context_id, distance
            FROM vec_contexts
            WHERE embedding MATCH ?1 AND k = ?2
        )
        SELECT c.id, c.uri, c.parent_uri, c.context_type, c.category, c.name,
               c.abstract, c.overview, c.content, c.tags, c.source_agent,
               c.source_session, c.importance, c.active_count, c.created_at,
               c.updated_at, c.superseded_by, knn.distance
        FROM knn
        JOIN contexts c ON c.id = knn.context_id
        WHERE c.superseded_by IS NULL",
    );

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(blob));
    param_values.push(Box::new(k as i64));
    let mut param_idx = 3;

    if let Some(proj) = project {
        let prefix = format!("rememora://projects/{proj}/");
        sql.push_str(&format!(
            " AND (c.uri LIKE ?{param_idx} OR c.uri LIKE 'rememora://global/%')"
        ));
        param_values.push(Box::new(format!("{prefix}%")));
        param_idx += 1;
    }

    if let Some(cat) = category {
        sql.push_str(&format!(" AND c.category = ?{param_idx}"));
        param_values.push(Box::new(cat.to_string()));
        param_idx += 1;
    }

    let _ = param_idx;
    sql.push_str(" ORDER BY knn.distance ASC");
    sql.push_str(&format!(" LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            let distance: f64 = row.get(17)?;
            // Cosine distance: 0 = identical, 2 = opposite → similarity = 1 - distance
            let similarity = 1.0 - distance;
            Ok(SearchResult {
                context: ContextRecord {
                    id: row.get(0)?,
                    uri: row.get(1)?,
                    parent_uri: row.get(2)?,
                    context_type: row.get(3)?,
                    category: row.get(4)?,
                    name: row.get(5)?,
                    abstract_text: row.get(6)?,
                    overview: row.get(7)?,
                    content: row.get(8)?,
                    tags: row.get(9)?,
                    source_agent: row.get(10)?,
                    source_session: row.get(11)?,
                    importance: row.get(12)?,
                    active_count: row.get(13)?,
                    created_at: row.get(14)?,
                    updated_at: row.get(15)?,
                    superseded_by: row.get(16)?,
                },
                rank: similarity,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Reciprocal Rank Fusion: merges two ranked lists into one.
///
/// RRF_score(d) = Σ 1 / (k + rank_i(d))
/// where k = 60 (standard constant to dampen high-rank dominance).
#[cfg(feature = "embed-candle")]
fn reciprocal_rank_fusion(
    bm25_results: Vec<SearchResult>,
    vec_results: Vec<SearchResult>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    use std::collections::HashMap;

    const K: f64 = 60.0;

    // Map context_id → (rrf_score, SearchResult)
    let mut scores: HashMap<String, (f64, SearchResult)> = HashMap::new();

    for (rank, result) in bm25_results.into_iter().enumerate() {
        let rrf = 1.0 / (K + (rank + 1) as f64);
        scores
            .entry(result.context.id.clone())
            .and_modify(|(s, _)| *s += rrf)
            .or_insert((rrf, result));
    }

    for (rank, result) in vec_results.into_iter().enumerate() {
        let rrf = 1.0 / (K + (rank + 1) as f64);
        scores
            .entry(result.context.id.clone())
            .and_modify(|(s, _)| *s += rrf)
            .or_insert((rrf, result));
    }

    let mut fused: Vec<SearchResult> = scores
        .into_values()
        .map(|(score, mut r)| {
            r.rank = score;
            r
        })
        .collect();

    // Sort descending by RRF score
    fused.sort_by(|a, b| b.rank.partial_cmp(&a.rank).unwrap_or(std::cmp::Ordering::Equal));
    fused.truncate(limit);

    Ok(fused)
}
