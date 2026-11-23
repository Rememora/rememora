use anyhow::Result;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

use crate::models::context::{self, ContextRecord};

pub struct SearchResult {
    pub context: ContextRecord,
    /// The hit's FTS5 BM25 score: negative, lower is better. Results are
    /// ordered by `rerank`'s score, which also folds in concept coverage,
    /// proximity and a small prior, so `rank` need not be monotone in the
    /// returned order. It stays raw BM25 because `evolve` compares a hit's rank
    /// to the self-match rank as a similarity ratio.
    pub rank: f64,
}

/// Side effects a search is allowed to have on the rows it returns.
#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    /// Record every returned row as accessed — `active_count + 1` and a fresh
    /// `last_accessed_at` (see `context::bump_active_count`).
    ///
    /// Set `false` for machine-driven retrieval whose results no human or agent
    /// necessarily reads: a memory injected into a prompt by the search hook is
    /// not evidence that the memory was useful, and counting it as one lets the
    /// hook manufacture its own ranking signal.
    pub bump: bool,
}

impl Default for SearchOptions {
    /// Bumping on — what `rememora search` has always done.
    fn default() -> Self {
        Self { bump: true }
    }
}

// ---------------------------------------------------------------------------
// Ranking knobs
// ---------------------------------------------------------------------------

/// How much a full match of every query concept multiplies the BM25 score
/// (`1 + COVERAGE_WEIGHT * coverage`). Coverage is IDF-weighted, so missing a
/// rare term costs more than missing a common one.
const COVERAGE_WEIGHT: f64 = 1.0;

/// Bonus for adjacent query concepts found within `NEAR_DISTANCE` tokens of
/// each other, scaled by the share of adjacent pairs that are.
const PROXIMITY_WEIGHT: f64 = 0.5;
const NEAR_DISTANCE: usize = 3;

/// Ceiling on the importance and hotness prior's effect: at most this fraction
/// of the score. Small on purpose, so it only reorders near-ties and a hot but
/// irrelevant memory can never overtake a clearly better match.
const PRIOR_WEIGHT: f64 = 0.1;

/// Candidate pool fetched by BM25 before reranking.
const MIN_POOL: usize = 50;

/// Shortest word that also matches as a prefix (`word*`), so "auth" finds
/// "authentication". Shorter words would match too much.
const PREFIX_MIN_LEN: usize = 5;

// ---------------------------------------------------------------------------
// Query analysis
// ---------------------------------------------------------------------------

/// Strip ASCII control chars (incl. NUL) and trim whitespace.
fn sanitize_input(query: &str) -> String {
    query
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect::<String>()
        .trim()
        .to_string()
}

/// Tokens that, if present as standalone words, signal the user is writing
/// FTS5 query syntax. Detection is case-sensitive — FTS5 itself only treats
/// uppercase OR/AND/NOT/NEAR as operators.
fn looks_like_fts5_syntax(q: &str) -> bool {
    if q.contains('(') || q.contains(')') || q.contains('"') {
        return true;
    }
    if q.contains('*') {
        // prefix queries: a token ending with `*`
        if q.split_whitespace()
            .any(|t| t.ends_with('*') && t.len() > 1)
        {
            return true;
        }
    }
    for tok in q.split_whitespace() {
        if matches!(tok, "OR" | "AND" | "NOT" | "NEAR") {
            return true;
        }
    }
    false
}

/// English function words that carry no topic on their own. Natural-language
/// questions ("why did we pick postgres") are mostly these, and as OR terms
/// they let a memory that happens to say "we" outrank one about postgres.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "again", "all", "also", "am", "an", "and", "any", "are", "as", "at",
    "be", "because", "been", "before", "being", "between", "both", "but", "by", "can", "could",
    "did", "do", "does", "doing", "done", "for", "from", "get", "gets", "got", "had", "has",
    "have", "having", "he", "her", "here", "him", "his", "how", "i", "if", "in", "into", "is",
    "it", "its", "just", "me", "might", "more", "most", "my", "of", "on", "once", "only", "or",
    "other", "our", "ours", "out", "she", "should", "so", "some", "such", "than", "that", "the",
    "their", "them", "then", "there", "these", "they", "this", "those", "to", "too", "us", "very",
    "was", "we", "were", "what", "when", "where", "whether", "which", "while", "who", "whom",
    "why", "will", "with", "would", "you", "your",
];

/// Short developer vocabulary that people write both ways. Each row is one
/// equivalence set; a query word in a row also matches every other entry.
/// Multi-word entries match as phrases. Porter stemming already folds plurals
/// and verb forms, so only genuinely different spellings belong here.
const SYNONYMS: &[&[&str]] = &[
    &["auth", "authentication", "authn"],
    &["authz", "authorization"],
    &["db", "database"],
    &["config", "configuration", "cfg"],
    &["env", "environment"],
    &["repo", "repository"],
    &["deps", "dependencies", "dependency"],
    &["k8s", "kubernetes"],
    &["postgres", "postgresql", "psql"],
    &["js", "javascript"],
    &["ts", "typescript"],
    &["py", "python"],
    &["perf", "performance"],
    &["dir", "directory"],
    &["docs", "documentation"],
    &["prod", "production"],
    &["dev", "development"],
    &["ci", "continuous integration"],
    &["pr", "pull request"],
    &["args", "arguments"],
    &["param", "parameter"],
    &["async", "asynchronous"],
    &["creds", "credentials"],
    &["pkg", "package"],
    &["lib", "library"],
    &["impl", "implementation"],
    &["init", "initialize", "initialise"],
    &["msg", "message"],
    &["tmp", "temp", "temporary"],
    &["i18n", "internationalization", "internationalisation"],
    &["l10n", "localization", "localisation"],
    &["oom", "out of memory"],
    &["gc", "garbage collection"],
    &["regex", "regexp", "regular expression"],
    &["macos", "osx"],
    &["mins", "minutes"],
    &["secs", "seconds"],
    &["ms", "milliseconds"],
];

/// One query concept: alternatives that all count as "this concept matched".
#[derive(Debug, Clone)]
struct Concept {
    /// FTS5 expressions, each already quoted and safe to OR together.
    alts: Vec<String>,
    /// A bare lowercase word eligible for typo correction.
    word: Option<String>,
}

impl Concept {
    fn expr(&self) -> String {
        if self.alts.len() == 1 {
            self.alts[0].clone()
        } else {
            format!("({})", self.alts.join(" OR "))
        }
    }
}

fn quote(words: &[String]) -> String {
    format!("\"{}\"", words.join(" "))
}

/// Split a raw token the way FTS5's unicode61 tokenizer does: runs of
/// alphanumeric characters, lowercased.
fn fts_words(tok: &str) -> Vec<String> {
    tok.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Split camelCase / PascalCase into parts: `useEffect` -> [use, effect],
/// `RCTBridge` -> [rct, bridge]. Returns one part for a word with no inner
/// case change.
fn camel_parts(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    for i in 0..chars.len() {
        let c = chars[i];
        let boundary = i > 0
            && c.is_uppercase()
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_ascii_digit()
                || (chars[i - 1].is_uppercase()
                    && chars.get(i + 1).is_some_and(|n| n.is_lowercase())));
        if boundary && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn synonyms_of(word: &str) -> Option<&'static [&'static str]> {
    SYNONYMS.iter().copied().find(|row| row.contains(&word))
}

/// Turn a plain (non-operator) query into concepts.
///
/// - Identifiers (`snake_case`, `kebab-case`, `a.b.c`, paths, `--flags`) stay
///   one concept and match as a phrase of their parts, which is how the
///   tokenizer indexed them. camelCase also matches its split form.
/// - Stopwords are dropped unless the query is nothing but stopwords.
/// - Words in `SYNONYMS` expand to their equivalence set.
///
/// Returns `(concepts, stopwords)`. The stopword concepts are kept aside for
/// the backfill in `ranked_search`, or become the concepts when nothing else
/// is left.
fn analyze(query: &str) -> (Vec<Concept>, Vec<Concept>) {
    let mut concepts = Vec::new();
    let mut stop_only = Vec::new();
    let mut seen = HashSet::new();
    for raw in query.split_whitespace() {
        let words = fts_words(raw);
        if words.is_empty() {
            continue;
        }
        let key = words.join(" ");
        if !seen.insert(key) {
            continue;
        }
        if words.len() > 1 {
            concepts.push(Concept {
                alts: vec![quote(&words)],
                word: None,
            });
            continue;
        }
        let w = &words[0];
        let core: String = raw.chars().filter(|c| c.is_alphanumeric()).collect();
        let parts = camel_parts(&core);
        if parts.len() > 1 {
            concepts.push(Concept {
                alts: vec![quote(&words), quote(&parts)],
                word: None,
            });
            continue;
        }
        let concept = if let Some(row) = synonyms_of(w) {
            let alts = row
                .iter()
                .map(|s| quote(&s.split(' ').map(str::to_string).collect::<Vec<_>>()))
                .collect();
            Concept { alts, word: None }
        } else {
            let mut alts = vec![quote(&words)];
            if w.chars().count() >= PREFIX_MIN_LEN {
                alts.push(format!("{}*", quote(&words)));
            }
            Concept {
                alts,
                word: Some(w.clone()),
            }
        };
        if STOPWORDS.contains(&w.as_str()) {
            stop_only.push(concept);
        } else {
            concepts.push(concept);
        }
    }
    if concepts.is_empty() {
        (stop_only, vec![])
    } else {
        (concepts, stop_only)
    }
}

/// Pure bag-of-words OR query — the safe fallback. Drops tokens that are pure
/// FTS5 punctuation so the resulting expression is always parseable.
fn safe_or_query(q: &str) -> Option<String> {
    let toks: Vec<String> = q
        .split_whitespace()
        .map(|t| {
            t.chars()
                .filter(|c| !matches!(c, '(' | ')' | '"' | '*' | ':'))
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .map(|t| {
            // Wrap each token in double quotes to defang any leftover FTS5
            // operators (e.g. a bare "OR" token in the bag would otherwise
            // be re-interpreted as the operator on retry).
            format!("\"{}\"", t.replace('"', ""))
        })
        .collect();
    if toks.is_empty() {
        None
    } else {
        Some(toks.join(" OR "))
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

pub fn search(
    conn: &Connection,
    query: &str,
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    search_with_options(
        conn,
        query,
        project,
        category,
        limit,
        SearchOptions::default(),
    )
}

/// `search` with explicit control over retrieval side effects.
///
/// Two paths:
/// - Explicit FTS5 syntax (OR/AND/NOT/NEAR, parens, quotes, `word*`) is passed
///   through verbatim and ranked by column-weighted BM25. If FTS5 rejects it,
///   the query is retried as plain words.
/// - Plain words go through `analyze` (stopwords, synonyms, identifiers), a
///   BM25 candidate pool, and `rerank` (concept coverage plus a small prior).
pub fn search_with_options(
    conn: &Connection,
    query: &str,
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
    options: SearchOptions,
) -> Result<Vec<SearchResult>> {
    let cleaned = sanitize_input(query);
    if cleaned.is_empty() || limit == 0 {
        return Ok(vec![]);
    }
    let filter = Filter { project, category };
    let rows = if looks_like_fts5_syntax(&cleaned) {
        match fetch_candidates(conn, &cleaned, &filter, limit) {
            Ok(rows) => rows.into_iter().map(|c| c.result).collect(),
            Err(e) if is_fts5_syntax_error(&e) => {
                // Defense-in-depth: FTS5 rejected the user's syntax (unbalanced
                // parens, lone quote, malformed NEAR). Strip the operator
                // characters and run it as a plain-word query.
                match safe_or_query(&cleaned) {
                    Some(plain) => ranked_search(conn, &plain.replace('"', ""), &filter, limit)?,
                    None => vec![],
                }
            }
            Err(e) => return Err(e),
        }
    } else {
        ranked_search(conn, &cleaned, &filter, limit)?
    };

    // Record the retrieval against the returned results
    if options.bump {
        for result in &rows {
            context::bump_active_count(conn, &result.context.id)?;
        }
    }
    Ok(rows)
}

/// Heuristic: did a SQL execute fail because FTS5 rejected the user's query
/// expression? FTS5 raises a few distinct messages depending on what it
/// disliked — "syntax error near …", "unterminated string", "no such column"
/// (when an unbalanced phrase confuses its column-filter parser), etc. The
/// goal is to fall back to the safe-OR query whenever the user's input was
/// the cause, but bubble up genuine SQL errors (DB locked, missing table).
fn is_fts5_syntax_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("fts5")
        || msg.contains("unterminated string")
        || msg.contains("syntax error")
        || msg.contains("malformed match")
        || msg.contains("no such column")
}

// ---------------------------------------------------------------------------
// Plain-word ranking
// ---------------------------------------------------------------------------

fn ranked_search(
    conn: &Connection,
    query: &str,
    filter: &Filter,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let (mut concepts, stopwords) = analyze(query);
    if concepts.is_empty() {
        return Ok(vec![]);
    }
    let total_docs: f64 =
        conn.query_row("SELECT count(*) FROM contexts", [], |r| r.get::<_, i64>(0))? as f64;
    let mut dfs = Vec::with_capacity(concepts.len());
    for c in &mut concepts {
        let mut df = doc_freq(conn, &c.expr())?;
        if df == 0 {
            if let Some(w) = c.word.clone() {
                let fixes = typo_candidates(conn, &w)?;
                if !fixes.is_empty() {
                    c.alts
                        .extend(fixes.iter().map(|f| quote(std::slice::from_ref(f))));
                    df = doc_freq(conn, &c.expr())?;
                }
            }
        }
        dfs.push(df);
    }
    let live: Vec<(Concept, f64)> = concepts
        .into_iter()
        .zip(dfs)
        .filter(|(_, df)| *df > 0)
        .map(|(c, df)| {
            let df = df as f64;
            let idf = (1.0 + (total_docs - df + 0.5) / (df + 0.5)).ln();
            (c, idf)
        })
        .collect();
    if live.is_empty() {
        return Ok(vec![]);
    }

    let fts_query = live
        .iter()
        .map(|(c, _)| c.expr())
        .collect::<Vec<_>>()
        .join(" OR ");
    let pool = (limit * 5).max(MIN_POOL);
    let candidates = fetch_candidates(conn, &fts_query, filter, pool)?;
    if candidates.is_empty() {
        return Ok(vec![]);
    }
    let mut out = rerank(conn, candidates, &live, limit)?;

    // Backfill: when the topic words match fewer than `limit` memories, fill
    // the remaining slots with memories that match only the dropped stopwords,
    // ranked after every topic match. Stopwords can add recall but never
    // outrank a memory that matches what the query is about.
    if out.len() < limit && !stopwords.is_empty() {
        let seen: HashSet<String> = out.iter().map(|r| r.context.id.clone()).collect();
        let expr = stopwords
            .iter()
            .map(Concept::expr)
            .collect::<Vec<_>>()
            .join(" OR ");
        let extra = fetch_candidates(conn, &expr, filter, limit + seen.len())?;
        out.extend(
            extra
                .into_iter()
                .map(|c| c.result)
                .filter(|r| !seen.contains(&r.context.id))
                .take(limit - out.len()),
        );
    }
    Ok(out)
}

/// Rescore a BM25 candidate pool.
///
/// `score = |bm25| * (1 + COVERAGE_WEIGHT * coverage)
///                 * (1 + PROXIMITY_WEIGHT * proximity) * (1 + PRIOR_WEIGHT * prior)`
///
/// - coverage: IDF-weighted share of query concepts the memory contains, so a
///   memory matching every concept beats one that repeats a single term.
/// - proximity: share of adjacent query concept pairs found within
///   `NEAR_DISTANCE` tokens of each other.
/// - prior: `hotness::final_score` (importance plus usage and recency), in
///   [0, 1]. Bounded by `PRIOR_WEIGHT`, so it only settles near-ties.
///
/// Multiplying keeps relevance in charge: a memory with no lexical match has
/// no score to scale, and the prior can move a hit by at most 10%.
fn rerank(
    conn: &Connection,
    candidates: Vec<Candidate>,
    concepts: &[(Concept, f64)],
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let rowids: Vec<String> = candidates.iter().map(|c| c.rowid.to_string()).collect();
    let in_list = rowids.join(",");
    let idf_total: f64 = concepts.iter().map(|(_, idf)| idf).sum();
    let mut covered: HashMap<i64, f64> = HashMap::new();
    if concepts.len() > 1 && COVERAGE_WEIGHT > 0.0 {
        for (c, idf) in concepts {
            let sql = format!(
                "SELECT rowid FROM contexts_fts WHERE contexts_fts MATCH ?1 AND rowid IN ({in_list})"
            );
            let mut stmt = conn.prepare(&sql)?;
            let ids = stmt.query_map([c.expr()], |r| r.get::<_, i64>(0))?;
            for id in ids {
                *covered.entry(id?).or_default() += idf;
            }
        }
    }

    // Term proximity: adjacent query concepts that also appear near each other
    // in the memory (sequential dependence, NEAR within a few tokens).
    let mut near: HashMap<i64, f64> = HashMap::new();
    let pairs: Vec<String> = concepts
        .windows(2)
        .map(|w| {
            format!(
                "NEAR({} {}, {})",
                w[0].0.alts[0], w[1].0.alts[0], NEAR_DISTANCE
            )
        })
        .collect();
    if concepts.len() > 1 {
        for expr in &pairs {
            let sql = format!(
                "SELECT rowid FROM contexts_fts WHERE contexts_fts MATCH ?1 AND rowid IN ({in_list})"
            );
            let mut stmt = conn.prepare(&sql)?;
            let ids = stmt.query_map([expr], |r| r.get::<_, i64>(0))?;
            for id in ids {
                *near.entry(id?).or_default() += 1.0 / pairs.len() as f64;
            }
        }
    }
    let mut scored: Vec<(f64, SearchResult)> = candidates
        .into_iter()
        .map(|c| {
            let coverage = if concepts.len() > 1 {
                covered.get(&c.rowid).copied().unwrap_or(0.0) / idf_total.max(f64::EPSILON)
            } else {
                1.0
            };
            let proximity = near.get(&c.rowid).copied().unwrap_or(0.0);
            let prior = prior_of(&c.result.context);
            let magnitude = -c.result.rank;
            let score = magnitude
                * (1.0 + COVERAGE_WEIGHT * coverage)
                * (1.0 + PROXIMITY_WEIGHT * proximity)
                * (1.0 + PRIOR_WEIGHT * prior);
            (score, c.result)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(limit).map(|(_, r)| r).collect())
}

/// Importance plus usage and recency, in [0, 1]. Unparseable timestamps count
/// as old rather than failing the search.
fn prior_of(ctx: &ContextRecord) -> f64 {
    let updated =
        parse_timestamp(&ctx.updated_at).unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
    crate::hotness::final_score(ctx.importance.clamp(0.0, 1.0), ctx.active_count, &updated)
        .clamp(0.0, 1.0)
}

fn parse_timestamp(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|n| n.and_utc())
}

fn doc_freq(conn: &Connection, expr: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM contexts_fts WHERE contexts_fts MATCH ?1",
        [expr],
        |r| r.get(0),
    )?)
}

/// Indexed terms within a small edit distance of a word that matched nothing.
///
/// Only runs for words with zero document frequency, so correctly spelled
/// queries never pay for it. Reads the index vocabulary through a temp
/// `fts5vocab` table (no schema change). Terms are compared against the word
/// and its leading prefixes, because the index holds Porter stems
/// ("dropped" is stored as "drop").
fn typo_candidates(conn: &Connection, word: &str) -> Result<Vec<String>> {
    let len = word.chars().count();
    if len < 5 || !word.chars().all(|c| c.is_alphanumeric()) {
        return Ok(vec![]);
    }
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS temp.rememora_fts_vocab
         USING fts5vocab(main, contexts_fts, row);",
    )?;
    let max_dist = if len >= 8 { 2 } else { 1 };
    // Porter strips at most a few suffix characters, so a stem shorter than
    // the word by more than that is a different word, not a typo.
    let min_len = len.saturating_sub(3).max(4);
    let first: String = word.chars().take(1).collect();
    let mut stmt = conn.prepare(
        "SELECT term, doc FROM temp.rememora_fts_vocab
         WHERE substr(term, 1, 1) = ?1 AND length(term) BETWEEN ?2 AND ?3",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![first, min_len as i64, (len + max_dist) as i64],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )?;
    let q: Vec<char> = word.chars().collect();
    let mut best: Vec<(usize, i64, String)> = Vec::new();
    for row in rows {
        let (term, doc) = row?;
        let t: Vec<char> = term.chars().collect();
        // Whole word, or a stem: compare the term against prefixes of the
        // query word that are at least as long as the term.
        let mut d = edit_distance(&q, &t);
        for cut in t.len().max(min_len)..q.len() {
            let rest: String = q[cut..].iter().collect();
            if STEM_SUFFIXES.contains(&rest.as_str()) {
                d = d.min(edit_distance(&q[..cut], &t));
            }
        }
        if d <= max_dist && d > 0 {
            best.push((d, -doc, term));
        }
    }
    best.sort();
    let top = best.first().map(|b| b.0);
    Ok(best
        .into_iter()
        .filter(|b| Some(b.0) == top)
        .take(3)
        .map(|b| b.2)
        .collect())
}

/// Endings Porter stemming removes. A typo match against a stem must leave
/// one of these behind, otherwise "strategy" would match the stem "start".
const STEM_SUFFIXES: &[&str] = &[
    "s", "es", "e", "ed", "d", "ing", "er", "ers", "ly", "y", "ies", "ied", "ion", "ions", "ation",
    "ations", "ment", "ments", "ness", "al", "ally", "ity", "ive", "able",
];

/// Optimal string alignment distance (Levenshtein plus adjacent swaps).
fn edit_distance(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

// ---------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------

struct Filter<'a> {
    project: Option<&'a str>,
    category: Option<&'a str>,
}

struct Candidate {
    rowid: i64,
    result: SearchResult,
}

/// SQL for "this project's memories plus global ones", bound to parameter
/// `?{idx}` holding the project URI prefix.
///
/// Compares a literal prefix with `substr` rather than `LIKE`, so `_` and `%`
/// in a project name are ordinary characters, not wildcards.
pub(crate) fn project_scope_sql(idx: usize) -> String {
    format!(
        "(substr(c.uri, 1, length(?{idx})) = ?{idx} \
         OR substr(c.uri, 1, 18) = 'rememora://global/')"
    )
}

pub(crate) fn project_prefix(project: &str) -> String {
    format!("rememora://projects/{project}/")
}

fn fetch_candidates(
    conn: &Connection,
    fts_query: &str,
    filter: &Filter,
    limit: usize,
) -> Result<Vec<Candidate>> {
    let mut sql = String::from(
        "SELECT c.id, c.uri, c.parent_uri, c.context_type, c.category, c.name,
                c.abstract, c.overview, c.content, c.tags, c.source_agent,
                c.source_session, c.importance, c.active_count, c.created_at,
                c.updated_at, c.superseded_by, c.worktree, c.branch,
                bm25(contexts_fts) AS score, c.rowid
         FROM contexts_fts
         JOIN contexts c ON c.rowid = contexts_fts.rowid
         WHERE contexts_fts MATCH ?1
         AND c.superseded_by IS NULL
         AND c.context_type != 'project'",
    );

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(fts_query.to_string()));
    let mut param_idx = 2;

    if let Some(proj) = filter.project {
        sql.push_str(&format!(" AND {}", project_scope_sql(param_idx)));
        param_values.push(Box::new(project_prefix(proj)));
        param_idx += 1;
    }

    if let Some(cat) = filter.category {
        sql.push_str(&format!(" AND c.category = ?{param_idx}"));
        param_values.push(Box::new(cat.to_string()));
    }

    sql.push_str(&format!(" ORDER BY score LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            Ok(Candidate {
                rowid: row.get(20)?,
                result: SearchResult {
                    context: record_from_row(row)?,
                    rank: row.get(19)?,
                },
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Columns 0..=18 of the shared SELECT list into a `ContextRecord`.
fn record_from_row(row: &rusqlite::Row) -> rusqlite::Result<ContextRecord> {
    Ok(ContextRecord {
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
        worktree: row.get(17)?,
        branch: row.get(18)?,
    })
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
    search_with_propagation_options(
        conn,
        query,
        project,
        category,
        limit,
        config,
        SearchOptions::default(),
    )
}

/// `search_with_propagation` with explicit control over retrieval side effects.
///
/// Note that the bump applies to the *pre-propagation* candidate set (3x the
/// requested limit), matching `search_with_propagation`'s existing behavior —
/// rows dropped during propagation have still been counted as retrieved.
pub fn search_with_propagation_options(
    conn: &Connection,
    query: &str,
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
    config: &crate::propagate::PropagationConfig,
    options: SearchOptions,
) -> Result<Vec<SearchResult>> {
    let expanded_limit = limit * 3;
    let results = search_with_options(conn, query, project, category, expanded_limit, options)?;
    crate::propagate::propagate_scores(conn, results, config, limit)
}

/// Store a precomputed embedding for a context in the context_embeddings table.
pub fn store_embedding(
    conn: &Connection,
    context_id: &str,
    embedding: &[f32],
    model_name: &str,
) -> Result<()> {
    let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
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
        let bm25_ids: std::collections::HashSet<&str> =
            bm25_results.iter().map(|r| r.context.id.as_str()).collect();
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
               c.updated_at, c.superseded_by, c.worktree, c.branch, knn.distance
        FROM knn
        JOIN contexts c ON c.id = knn.context_id
        WHERE c.superseded_by IS NULL",
    );

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(blob));
    param_values.push(Box::new(k as i64));
    let mut param_idx = 3;

    if let Some(proj) = project {
        sql.push_str(&format!(" AND {}", project_scope_sql(param_idx)));
        param_values.push(Box::new(project_prefix(proj)));
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
            let distance: f64 = row.get(19)?;
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
                    worktree: row.get(17)?,
                    branch: row.get(18)?,
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
    fused.sort_by(|a, b| {
        b.rank
            .partial_cmp(&a.rank)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    fused.truncate(limit);

    Ok(fused)
}
