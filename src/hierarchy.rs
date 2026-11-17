use std::cmp::Ordering;
use std::collections::HashSet;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;

use crate::hotness;
use crate::models::context::{self, ContextRecord};
use crate::models::project::{self, ProjectInfo};
use crate::models::session::{self, SessionRecord};
use crate::search::{self, SearchOptions};

/// A memory counts as "recent" when it was created or updated this many days ago or less.
pub const RECENT_DAYS: i64 = 7;
/// Most memories listed under "Recent".
pub const RECENT_LIMIT: usize = 5;
/// Most search hits listed under "Relevant to".
pub const RELEVANT_LIMIT: usize = 5;
/// Most active sessions listed.
pub const ACTIVE_SESSION_LIMIT: usize = 10;

pub struct ContextAssembly {
    pub project_name: Option<String>,
    /// Registry row for the project, shown once in the header.
    pub project_info: Option<ProjectInfo>,
    /// Latest ended or transferred session that left a working state or summary.
    pub handoff: Option<SessionRecord>,
    /// Sessions still active on the project, newest first.
    pub active_sessions: Vec<SessionRecord>,
    /// The task the "Relevant to" section was searched for.
    pub query: Option<String>,
    /// True when `query` came from the newest active session's intent rather
    /// than from `--query`.
    pub query_from_session: bool,
    /// Best search hits for `query`, in search order.
    pub relevant: Vec<ScoredContext>,
    /// Memories created or updated in the last `RECENT_DAYS`, newest first,
    /// excluding anything already in `relevant`.
    pub recent: Vec<ScoredContext>,
    /// Every other memory in scope, best score first. The renderer prints the
    /// head in full and the tail as one-line abstracts.
    pub ranked: Vec<ScoredContext>,
}

impl ContextAssembly {
    /// Number of distinct memories across all sections.
    pub fn memory_count(&self) -> usize {
        self.relevant.len() + self.recent.len() + self.ranked.len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContextOptions {
    /// What the agent is about to do. When `None`, the newest active session's
    /// intent is used if it has one.
    pub query: Option<String>,
}

pub struct ScoredContext {
    pub context: ContextRecord,
    pub score: f64,
}

/// Every memory in scope, scored and ranked (best first, deterministic on ties).
///
/// - When `project` is `Some(...)`, returns global preferences plus the
///   project's own memories.
/// - When `project` is `None` (Global mode, typically `context --auto` from
///   an unregistered cwd), aggregates across every project plus globals so
///   the user sees memories from the rest of their workspace instead of an
///   empty page. Issue #104.
///
/// Only `memory` rows are returned: the project registry row is metadata, and
/// `assemble` shows it once in the header instead.
pub fn get_l0_map(conn: &Connection, project: Option<&str>) -> Result<Vec<ScoredContext>> {
    let now = Utc::now();
    let mut all: Vec<ScoredContext> = context::list_memories_in_scope(conn, project)?
        .into_iter()
        .map(|ctx| {
            let score = compute_score(&ctx, now);
            ScoredContext { context: ctx, score }
        })
        .collect();
    all.sort_by(rank_order);
    Ok(all)
}

/// Get L1 context: top-N contexts by score.
pub fn get_l1_context(conn: &Connection, project: Option<&str>, limit: usize) -> Result<Vec<ScoredContext>> {
    let l0 = get_l0_map(conn, project)?;
    Ok(l0.into_iter().take(limit).collect())
}

/// The handoff to show for a project (see `session::get_latest_handoff`).
pub fn get_session_context(conn: &Connection, project: &str) -> Result<Option<SessionRecord>> {
    session::get_latest_handoff(conn, project)
}

/// The latest handoff for a project, if any (cheatsheet use).
pub fn get_latest_session(conn: &Connection, project: Option<&str>) -> Result<Option<SessionRecord>> {
    match project {
        Some(p) => get_session_context(conn, p),
        None => Ok(None),
    }
}

/// Full context assembly with default options.
pub fn assemble(conn: &Connection, project: Option<&str>) -> Result<ContextAssembly> {
    assemble_with(conn, project, &ContextOptions::default())
}

/// Full context assembly: handoff, active sessions, relevant, recent and
/// ranked memories. Each memory appears in exactly one section.
///
/// Pure read: search runs with `bump: false`, so loading context does not
/// count as retrieving every memory it lists.
pub fn assemble_with(conn: &Connection, project: Option<&str>, opts: &ContextOptions) -> Result<ContextAssembly> {
    let now = Utc::now();
    let mut ranked = get_l0_map(conn, project)?;

    let (handoff, active_sessions, project_info) = match project {
        Some(p) => (
            session::get_latest_handoff(conn, p)?,
            session::list_active_for_project(conn, p, ACTIVE_SESSION_LIMIT)?,
            project::get_info(conn, p)?,
        ),
        None => (None, Vec::new(), None),
    };

    let explicit = opts
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(String::from);
    let from_session = active_sessions
        .first()
        .map(|s| s.intent.trim().to_string())
        .filter(|q| !q.is_empty());
    let query_from_session = explicit.is_none() && from_session.is_some();
    let query = explicit.or(from_session);

    // Relevant: search hits, kept only when they are memories in scope.
    let mut relevant = Vec::new();
    if let Some(q) = query.as_deref() {
        let hits = search::search_with_options(
            conn,
            q,
            project,
            None,
            RELEVANT_LIMIT * 4,
            SearchOptions { bump: false },
        )?;
        for hit in hits {
            if relevant.len() >= RELEVANT_LIMIT {
                break;
            }
            if let Some(pos) = ranked.iter().position(|s| s.context.id == hit.context.id) {
                relevant.push(ranked.remove(pos));
            }
        }
    }

    // Recent: created or updated in the last RECENT_DAYS, newest first.
    let cutoff = now - Duration::days(RECENT_DAYS);
    let mut recent_idx: Vec<(DateTime<Utc>, usize)> = ranked
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let t = last_touched(&s.context)?;
            (t >= cutoff).then_some((t, i))
        })
        .collect();
    recent_idx.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| ranked[b.1].context.id.cmp(&ranked[a.1].context.id))
    });
    recent_idx.truncate(RECENT_LIMIT);
    let picked: HashSet<usize> = recent_idx.iter().map(|(_, i)| *i).collect();
    let mut slots: Vec<Option<ScoredContext>> = ranked.into_iter().map(Some).collect();
    let recent: Vec<ScoredContext> = recent_idx
        .iter()
        .filter_map(|(_, i)| slots[*i].take())
        .collect();
    let ranked: Vec<ScoredContext> = slots
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !picked.contains(i))
        .filter_map(|(_, s)| s)
        .collect();

    Ok(ContextAssembly {
        project_name: project.map(String::from),
        project_info,
        handoff,
        active_sessions,
        query,
        query_from_session,
        relevant,
        recent,
        ranked,
    })
}

/// Score order: score desc, then most recently updated, then id, so equal
/// scores never shuffle between runs.
fn rank_order(a: &ScoredContext, b: &ScoredContext) -> Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| b.context.updated_at.cmp(&a.context.updated_at))
        .then_with(|| a.context.id.cmp(&b.context.id))
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// Later of created_at and updated_at.
fn last_touched(ctx: &ContextRecord) -> Option<DateTime<Utc>> {
    match (parse_ts(&ctx.created_at), parse_ts(&ctx.updated_at)) {
        (Some(c), Some(u)) => Some(c.max(u)),
        (c, u) => c.or(u),
    }
}

fn compute_score(ctx: &ContextRecord, now: DateTime<Utc>) -> f64 {
    let updated_at = parse_ts(&ctx.updated_at).unwrap_or(now);
    hotness::final_score(ctx.importance, ctx.active_count, &updated_at)
}
