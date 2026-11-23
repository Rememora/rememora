//! Cheap near-duplicate detection for the save path.
//!
//! Agents re-save the same fact in slightly different words, and nothing
//! noticed: two live memories that say the same thing split hotness, double the
//! tokens `context` spends, and drift apart as one gets edited. This module
//! finds the obvious cases at save time with no LLM and no embedding: one FTS5
//! query fetches a handful of candidates, then token-set overlap decides.
//!
//! It only ever *suggests*. Nothing here merges, blocks or rewrites a memory,
//! and every failure degrades to "no suggestions" so a save never fails on it.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::{params, Connection};

/// How many FTS candidates to score. Small on purpose: this runs on every save.
const CANDIDATES: usize = 8;

/// Most distinct tokens sent to FTS. Longer memories are capped so the query
/// stays one cheap index lookup instead of a wide OR over every word.
const MAX_QUERY_TOKENS: usize = 16;

/// Overlap thresholds. Conservative: a false "similar" hint trains agents to
/// ignore the hint, which is worse than missing a borderline pair.
const JACCARD_MIN: f64 = 0.6;
const CONTAINMENT_MIN: f64 = 0.85;

/// Below this many tokens, overlap is too noisy to mean anything.
const MIN_TOKENS: usize = 4;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "been", "but", "by", "do", "does", "for", "from",
    "has", "have", "if", "in", "into", "is", "it", "its", "not", "of", "on", "or", "so", "than",
    "that", "the", "then", "there", "these", "this", "to", "too", "use", "uses", "was", "we",
    "were", "when", "which", "while", "with", "you",
];

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SimilarMemory {
    pub id: String,
    pub name: String,
    pub uri: String,
    /// The larger of Jaccard and containment, rounded for display.
    pub score: f64,
}

/// Lowercased, stopword-stripped word tokens.
pub fn tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// Similarity of two token sets: `max(jaccard, containment)`, or 0.0 when
/// either set is too small for the number to be meaningful.
///
/// Containment (overlap over the smaller set) catches the common "same fact,
/// one extra clause" re-save that Jaccard alone scores too low.
pub fn score(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.len() < MIN_TOKENS || b.len() < MIN_TOKENS {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    let jaccard = inter / union;
    let containment = inter / a.len().min(b.len()) as f64;
    if jaccard >= JACCARD_MIN || containment >= CONTAINMENT_MIN {
        jaccard.max(containment)
    } else {
        0.0
    }
}

/// Live memories in `project` (plus global) that look like `text`.
///
/// `exclude_id` is the memory just written, which would otherwise match itself.
pub fn find_similar(
    conn: &Connection,
    text: &str,
    project: Option<&str>,
    exclude_id: &str,
) -> Result<Vec<SimilarMemory>> {
    let mine = tokens(text);
    if mine.len() < MIN_TOKENS {
        return Ok(Vec::new());
    }

    // Longest tokens first: they are the rarest and say the most about topic.
    let mut query_tokens: Vec<&String> = mine.iter().collect();
    query_tokens.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    let fts_query = query_tokens
        .into_iter()
        .take(MAX_QUERY_TOKENS)
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ");

    let project_prefix = project.map(|p| format!("rememora://projects/{p}/"));
    let mut stmt = conn.prepare(
        "SELECT c.id, c.name, c.uri, c.content
         FROM contexts_fts f JOIN contexts c ON c.rowid = f.rowid
         WHERE contexts_fts MATCH ?1
           AND c.context_type = 'memory'
           AND c.superseded_by IS NULL
           AND c.id != ?2
           AND (c.uri LIKE 'rememora://global/%' OR (?3 IS NOT NULL AND substr(c.uri, 1, length(?3)) = ?3))
         ORDER BY bm25(contexts_fts)
         LIMIT ?4",
    )?;
    let rows = stmt
        .query_map(
            params![fts_query, exclude_id, project_prefix, CANDIDATES as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut out: Vec<SimilarMemory> = rows
        .into_iter()
        .filter_map(|(id, name, uri, content)| {
            let s = score(&mine, &tokens(&content));
            (s > 0.0).then(|| SimilarMemory {
                id,
                name,
                uri,
                score: (s * 100.0).round() / 100.0,
            })
        })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_drop_stopwords_and_case() {
        let t = tokens("The Build uses Bazel, and the cache!");
        assert!(t.contains("bazel") && t.contains("build") && t.contains("cache"));
        assert!(!t.contains("the") && !t.contains("and"));
    }

    #[test]
    fn rewording_of_the_same_fact_scores_high() {
        let a = tokens("Search ranking uses BM25 over the FTS5 index with hotness boost");
        let b = tokens("search ranking: BM25 over FTS5 index plus a hotness boost");
        assert!(score(&a, &b) >= JACCARD_MIN);
    }

    #[test]
    fn unrelated_memories_score_zero() {
        let a = tokens("Search ranking uses BM25 over the FTS5 index");
        let b = tokens("Prefer pnpm over npm for the web workspace installs");
        assert_eq!(score(&a, &b), 0.0);
    }

    #[test]
    fn tiny_texts_never_match() {
        assert_eq!(score(&tokens("bazel cache"), &tokens("bazel cache")), 0.0);
    }
}
