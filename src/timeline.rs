//! Library-level timeline primitives.
//!
//! The `rememora timeline` CLI verb is a thin wrapper over [`build_timeline`];
//! the heavy lifting lives here so that behavior tests can exercise it
//! without reaching into the binary crate.
//!
//! Timeline is the middle layer in the progressive-disclosure trio:
//!
//! - `search` — filter (many hits, tiny per-hit payload)
//! - `timeline` — context (neighbours around one anchor)
//! - `get` — full L2 content for a single URI

use anyhow::{bail, Result};
use chrono::DateTime;
use rusqlite::Connection;

use crate::hotness;
use crate::models::context::{self, ContextRecord};
use crate::uri;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineOrder {
    Ts,
    Hotness,
}

impl TimelineOrder {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "ts" | "time" | "created" => Ok(Self::Ts),
            "hotness" | "hot" => Ok(Self::Hotness),
            other => bail!("unknown --by value: {other} (expected ts|hotness)"),
        }
    }
}

pub struct TimelineArgs {
    pub anchor: String,
    pub before: usize,
    pub after: usize,
    pub project: Option<String>,
    pub by: TimelineOrder,
}

/// Rendered timeline: `before` (oldest-first for `Ts`, hottest-first for
/// `Hotness`), then the anchor, then `after` (same ordering rule).
#[derive(Debug)]
pub struct Timeline {
    pub anchor: ContextRecord,
    pub before: Vec<ContextRecord>,
    pub after: Vec<ContextRecord>,
}

/// Build a timeline around `args.anchor`.
///
/// Does NOT bump the anchor's `active_count` — callers that want the same
/// side effect as `rememora get` should do so after build.
pub fn build_timeline(conn: &Connection, args: &TimelineArgs) -> Result<Timeline> {
    let anchor = match context::get_by_uri(conn, &args.anchor)? {
        Some(c) => c,
        None => bail!("Context not found: {}", args.anchor),
    };

    // Scope: explicit --project wins, otherwise infer from the anchor's URI.
    // Global anchors fall back to global scope.
    let scope_project = args
        .project
        .clone()
        .or_else(|| uri::extract_project(&anchor.uri));

    let peers = list_peers(conn, scope_project.as_deref(), &anchor.id)?;

    let (before, after) = match args.by {
        TimelineOrder::Ts => slice_by_ts(&anchor, peers, args.before, args.after),
        TimelineOrder::Hotness => slice_by_hotness(peers, args.before, args.after),
    };

    Ok(Timeline {
        anchor,
        before,
        after,
    })
}

fn list_peers(
    conn: &Connection,
    project: Option<&str>,
    exclude_id: &str,
) -> Result<Vec<ContextRecord>> {
    let mut sql = String::from(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract,
                overview, content, tags, source_agent, source_session,
                importance, active_count, created_at, updated_at, superseded_by
         FROM contexts
         WHERE superseded_by IS NULL AND id != ?1",
    );

    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    params.push(Box::new(exclude_id.to_string()));

    if let Some(proj) = project {
        sql.push_str(
            " AND (uri LIKE ?2 OR uri LIKE 'rememora://global/%')",
        );
        params.push(Box::new(format!("rememora://projects/{proj}/%")));
    } else {
        sql.push_str(" AND uri LIKE 'rememora://global/%'");
    }

    sql.push_str(" ORDER BY created_at ASC");

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
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
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

fn slice_by_ts(
    anchor: &ContextRecord,
    peers: Vec<ContextRecord>,
    before: usize,
    after: usize,
) -> (Vec<ContextRecord>, Vec<ContextRecord>) {
    // peers are already sorted ascending by created_at.
    let anchor_ts = anchor.created_at.as_str();

    let mut older: Vec<ContextRecord> = peers
        .iter()
        .filter(|c| c.created_at.as_str() < anchor_ts)
        .cloned()
        .collect();
    let newer: Vec<ContextRecord> = peers
        .iter()
        .filter(|c| c.created_at.as_str() > anchor_ts)
        .cloned()
        .collect();

    // `older` is oldest → newest; take the newest `before` entries while
    // preserving oldest-first display order.
    let older_len = older.len();
    if older_len > before {
        older.drain(0..(older_len - before));
    }

    let after_slice: Vec<ContextRecord> = newer.into_iter().take(after).collect();

    (older, after_slice)
}

fn slice_by_hotness(
    peers: Vec<ContextRecord>,
    before: usize,
    after: usize,
) -> (Vec<ContextRecord>, Vec<ContextRecord>) {
    let mut scored: Vec<(f64, ContextRecord)> = peers
        .into_iter()
        .map(|c| {
            let updated = DateTime::parse_from_rfc3339(&c.updated_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let score = hotness::final_score(c.importance, c.active_count, &updated);
            (score, c)
        })
        .collect();

    // Hottest first.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let top: Vec<ContextRecord> = scored
        .into_iter()
        .take(before + after)
        .map(|(_, c)| c)
        .collect();

    // Split into before / after buckets. Scores are monotonically descending
    // in `top`, so the hottest peers end up in `before`; callers display
    // before → anchor → after so hot neighbours sit closest to the anchor.
    let mut b = top;
    let after_vec: Vec<ContextRecord> = b.split_off(b.len().min(before));
    let before_vec = b;
    (before_vec, after_vec.into_iter().take(after).collect())
}
