use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::hotness;
use crate::models::context::{self, ContextRecord};
use crate::models::session;

pub struct ContextAssembly {
    pub project_name: Option<String>,
    pub l0_abstracts: Vec<ScoredContext>,
    pub l1_overviews: Vec<ScoredContext>,
    pub latest_session: Option<session::SessionRecord>,
}

pub struct ScoredContext {
    pub context: ContextRecord,
    pub score: f64,
}

/// Get L0 map: scored, ranked abstracts.
///
/// - When `project` is `Some(...)`, returns global preferences plus the
///   project's own contexts (the original behavior).
/// - When `project` is `None` (Global mode — typically `context --auto` from
///   an unregistered cwd), aggregates across **every** project plus globals
///   so the user sees memories from the rest of their workspace instead of an
///   empty page. Issue #104.
pub fn get_l0_map(conn: &Connection, project: Option<&str>) -> Result<Vec<ScoredContext>> {
    let mut all = Vec::new();

    // Global preferences (always included)
    let globals = context::list_by_scope(conn, Some("memory"), Some("preference"), None, 50)?;
    for ctx in globals {
        // Only include global-scoped preferences
        if ctx.uri.starts_with("rememora://global/") {
            let score = compute_score(&ctx);
            all.push(ScoredContext { context: ctx, score });
        }
    }

    match project {
        Some(proj) => {
            // Project-scoped contexts
            let project_contexts = context::list_by_scope(conn, None, None, Some(proj), 100)?;
            for ctx in project_contexts {
                let score = compute_score(&ctx);
                all.push(ScoredContext { context: ctx, score });
            }
        }
        None => {
            // Global mode: aggregate across every registered project so the
            // L0 page is informative even when cwd is not a known project.
            // We pull a generous cap (200) and let the score-sort below
            // surface the highest-signal entries.
            let everything = context::list_by_scope(conn, Some("memory"), None, None, 200)?;
            // Skip globals already added above (avoid duplicates).
            let already: std::collections::HashSet<String> =
                all.iter().map(|s| s.context.id.clone()).collect();
            for ctx in everything {
                if already.contains(&ctx.id) {
                    continue;
                }
                let score = compute_score(&ctx);
                all.push(ScoredContext { context: ctx, score });
            }
        }
    }

    all.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(all)
}

/// Get L1 context: top-N contexts with overview text
pub fn get_l1_context(conn: &Connection, project: Option<&str>, limit: usize) -> Result<Vec<ScoredContext>> {
    let l0 = get_l0_map(conn, project)?;
    Ok(l0.into_iter().take(limit).collect())
}

/// Get latest session context for a project
pub fn get_session_context(conn: &Connection, project: &str) -> Result<Option<session::SessionRecord>> {
    session::get_latest_for_project(conn, project)
}

/// Get the latest session for a project (public wrapper for cheatsheet use).
pub fn get_latest_session(conn: &Connection, project: Option<&str>) -> Result<Option<session::SessionRecord>> {
    match project {
        Some(p) => get_session_context(conn, p),
        None => Ok(None),
    }
}

/// Full context assembly: L0 map + L1 details + session state → structured result
pub fn assemble(conn: &Connection, project: Option<&str>) -> Result<ContextAssembly> {
    let l0 = get_l0_map(conn, project)?;
    let l1 = get_l1_context(conn, project, 15)?;
    let latest_session = project.and_then(|p| get_session_context(conn, p).ok().flatten());

    Ok(ContextAssembly {
        project_name: project.map(String::from),
        l0_abstracts: l0,
        l1_overviews: l1,
        latest_session,
    })
}

fn compute_score(ctx: &ContextRecord) -> f64 {
    let updated_at: DateTime<Utc> = ctx
        .updated_at
        .parse()
        .unwrap_or_else(|_| Utc::now());
    hotness::final_score(ctx.importance, ctx.active_count, &updated_at)
}
