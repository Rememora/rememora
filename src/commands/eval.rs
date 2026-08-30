use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

pub struct EvalArgs {
    pub project: Option<String>,
    pub days: u32,
}

#[derive(Debug, Serialize)]
struct EvalReport {
    window_days: u32,
    project: Option<String>,
    session_compliance: SessionCompliance,
    memory_save_rate: MemorySaveRate,
    context_load_rate: ContextLoadRate,
    transfer_success: TransferSuccess,
    per_agent: Vec<AgentBreakdown>,
    per_project: Vec<ProjectBreakdown>,
}

#[derive(Debug, Serialize)]
struct SessionCompliance {
    total: i64,
    ended: i64,
    transferred: i64,
    orphaned: i64,
    compliance_pct: f64,
}

/// Whether a metric could be computed at all.
///
/// A metric that always reads `0` is worse than no metric, because people
/// believe it — that is how the team came to think recall was 0%. "No signal
/// exists" and "the signal exists and is zero" are different claims, so they
/// get different values here and derived rates are `None` rather than `0.0`
/// when nothing can be said.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MetricStatus {
    Available,
    Unavailable,
}

#[derive(Debug, Serialize)]
struct MemorySaveRate {
    status: MetricStatus,
    unavailable_reason: Option<&'static str>,
    /// Memories attributed to a session that started inside the window.
    total_memories: i64,
    /// Memories created inside the window carrying no `source_session`.
    ///
    /// Non-zero for every row written before save-time attribution existed.
    /// Those rows cannot be backfilled — nothing in the schema records which
    /// session was open when they were written — so they are reported, not
    /// imputed into `total_memories`.
    unattributed_memories: i64,
    sessions_in_window: i64,
    avg_per_session: Option<f64>,
    sessions_with_zero_saves: i64,
    zero_save_pct: Option<f64>,
}

#[derive(Debug, Serialize)]
struct ContextLoadRate {
    status: MetricStatus,
    unavailable_reason: Option<&'static str>,
    sessions_in_window: i64,
    retrieval_reach: RetrievalReach,
}

/// The one honest retrieval signal observable with today's schema.
///
/// `active_count` is incremented only by `search::search` / `hybrid_search`,
/// i.e. only when a memory was actually returned to a caller. It carries no
/// timestamp and no session id, so this is an **all-time, corpus-level**
/// figure — it cannot be windowed by `--days` or attributed to a session, and
/// must not be read as a per-session load rate.
#[derive(Debug, Serialize)]
struct RetrievalReach {
    retrieved_memories: i64,
    total_memories: i64,
    reach_pct: Option<f64>,
}

#[derive(Debug, Serialize)]
struct TransferSuccess {
    transferred: i64,
    picked_up: i64,
    pickup_pct: f64,
}

#[derive(Debug, Serialize)]
struct AgentBreakdown {
    agent: String,
    sessions: i64,
    ended: i64,
    orphaned: i64,
    compliance_pct: f64,
    memories: i64,
    avg_memories: f64,
}

#[derive(Debug, Serialize)]
struct ProjectBreakdown {
    project: String,
    sessions: i64,
    memories: i64,
    avg_memories: f64,
}

pub fn run(conn: &Connection, args: &EvalArgs, json: bool) -> Result<()> {
    let cutoff = format!(
        "datetime('now', '-{} days')",
        args.days
    );

    let project_filter = args.project.as_deref();

    let report = EvalReport {
        window_days: args.days,
        project: args.project.clone(),
        session_compliance: query_session_compliance(conn, &cutoff, project_filter)?,
        memory_save_rate: query_memory_save_rate(conn, &cutoff, project_filter)?,
        context_load_rate: query_context_load_rate(conn, &cutoff, project_filter)?,
        transfer_success: query_transfer_success(conn, &cutoff, project_filter)?,
        per_agent: query_per_agent(conn, &cutoff, project_filter)?,
        per_project: query_per_project(conn, &cutoff, project_filter)?,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }

    Ok(())
}

fn query_session_compliance(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<SessionCompliance> {
    let (where_clause, param_values) = build_session_filter(cutoff, project);

    let sql = format!(
        "SELECT
            COUNT(*) as total,
            COALESCE(SUM(CASE WHEN status = 'ended' THEN 1 ELSE 0 END), 0) as ended,
            COALESCE(SUM(CASE WHEN status = 'transferred' THEN 1 ELSE 0 END), 0) as transferred,
            COALESCE(SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END), 0) as orphaned
         FROM sessions
         WHERE {where_clause}"
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let result = stmt.query_row(params_ref.as_slice(), |row| {
        let total: i64 = row.get(0)?;
        let ended: i64 = row.get(1)?;
        let transferred: i64 = row.get(2)?;
        let orphaned: i64 = row.get(3)?;
        Ok(SessionCompliance {
            total,
            ended,
            transferred,
            orphaned,
            compliance_pct: if total > 0 {
                ((ended + transferred) as f64 / total as f64) * 100.0
            } else {
                0.0
            },
        })
    })?;

    Ok(result)
}

/// Memory-save rate — how much each session actually writes down.
///
/// This joins on `contexts.source_session`, which `commands::save` hardcoded
/// to `None`. The column was never populated by any user-reachable path, so
/// the rate was 0 regardless of behavior. `save` now attributes to the active
/// session; rows written before that cannot be recovered, so they are counted
/// separately as `unattributed_memories` instead of silently dragging the
/// rate to zero.
fn query_memory_save_rate(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<MemorySaveRate> {
    let (session_where, param_values) = build_session_filter(cutoff, project);

    // Total memories saved in sessions within the window
    let sql = format!(
        "SELECT COUNT(*)
         FROM contexts c
         WHERE c.context_type = 'memory'
           AND c.source_session IN (SELECT id FROM sessions WHERE {session_where})"
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let total_memories: i64 = stmt.query_row(params_ref.as_slice(), |row| row.get(0))?;

    // Total sessions in window
    let sql2 = format!(
        "SELECT COUNT(*) FROM sessions WHERE {session_where}"
    );
    let mut stmt2 = conn.prepare(&sql2)?;
    let params_ref2: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let total_sessions: i64 = stmt2.query_row(params_ref2.as_slice(), |row| row.get(0))?;

    // Sessions with zero saves
    let sql3 = format!(
        "SELECT COUNT(*)
         FROM sessions s
         WHERE {session_where}
           AND NOT EXISTS (
               SELECT 1 FROM contexts c
               WHERE c.source_session = s.id AND c.context_type = 'memory'
           )",
        session_where = session_where.replace("started_at", "s.started_at")
            .replace("project ", "s.project ")
    );
    let mut stmt3 = conn.prepare(&sql3)?;
    let params_ref3: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let zero_saves: i64 = stmt3.query_row(params_ref3.as_slice(), |row| row.get(0))?;

    // Memories created in the window that carry no session attribution.
    // `contexts` has no project column, so scope by URI prefix the way
    // `context::list_by_scope` does.
    //
    // `datetime()` wraps both sides deliberately: `created_at` is RFC3339
    // (`...T12:00:00+00:00`) while `datetime('now', ...)` renders a space
    // separator, and comparing those as raw strings is off by up to a day at
    // the boundary — the same class of mismatch that pinned the load rate to
    // zero (see `query_context_load_rate`).
    let uri_param = project.map(|p| format!("rememora://projects/{p}/%"));
    let uri_clause = if uri_param.is_some() {
        " AND c.uri LIKE ?1"
    } else {
        ""
    };
    let sql4 = format!(
        "SELECT COUNT(*)
         FROM contexts c
         WHERE c.context_type = 'memory'
           AND c.source_session IS NULL
           AND datetime(c.created_at) >= datetime({cutoff}){uri_clause}"
    );
    let unattributed: i64 = match &uri_param {
        Some(v) => conn.query_row(&sql4, rusqlite::params![v], |row| row.get(0))?,
        None => conn.query_row(&sql4, [], |row| row.get(0))?,
    };

    // With no sessions in the window there is no denominator. Report that
    // rather than a 0.0 that reads as "nobody saved anything".
    let available = total_sessions > 0;

    Ok(MemorySaveRate {
        status: if available {
            MetricStatus::Available
        } else {
            MetricStatus::Unavailable
        },
        unavailable_reason: if available {
            None
        } else {
            Some("no sessions started in this window — no denominator to rate against")
        },
        total_memories,
        unattributed_memories: unattributed,
        sessions_in_window: total_sessions,
        avg_per_session: available.then(|| total_memories as f64 / total_sessions as f64),
        sessions_with_zero_saves: zero_saves,
        zero_save_pct: available.then(|| (zero_saves as f64 / total_sessions as f64) * 100.0),
    })
}

/// Explains why the per-session load rate is reported rather than computed.
const LOAD_RATE_UNAVAILABLE: &str = "no per-retrieval event is recorded, so a load cannot be \
     attributed to a session; needs the `last_accessed_at` column";

/// Context-load rate — the fraction of sessions that actually pulled memory in.
///
/// **Not computable today, and reported as such.** The previous implementation
/// asked whether any context row's `updated_at` moved within 60s of a session's
/// `started_at` with `active_count > 0`. That returned 0 for three independent
/// reasons, any one of which is sufficient:
///
///   1. Nothing records a load. The SessionStart hook runs `context --auto`,
///      and `hierarchy::assemble` is pure read — it writes no row.
///   2. Ordering. That hook injects context *before* it runs `session start`,
///      so even a write would land before `started_at`, outside the window.
///   3. Timestamp formats. `updated_at` is RFC3339 (`...T12:00:30+00:00`)
///      while `datetime(started_at, '+60 seconds')` renders `... 12:01:00`.
///      Compared as strings, `'T'` (0x54) sorts above `' '` (0x20), so the
///      upper bound was false for every same-day pair — the predicate could
///      only ever hold across a midnight rollover.
///
/// It was also keyed off `updated_at`, which `bump_active_count` writes on
/// every search, so the metric was entangled with retrieval rather than load.
///
/// The correct long-term signal is a per-retrieval `last_accessed_at`
/// timestamp on `contexts`, added by a migration on a separate branch; once it
/// lands, this becomes "sessions where some context was accessed between
/// `started_at` and `ended_at`". Until then we report `Unavailable` and
/// surface the retrieval signal that needs no new column: `active_count`.
fn query_context_load_rate(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<ContextLoadRate> {
    let (session_where, param_values) = build_session_filter(cutoff, project);

    let sql = format!("SELECT COUNT(*) FROM sessions WHERE {session_where}");
    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let sessions_in_window: i64 = stmt.query_row(params_ref.as_slice(), |row| row.get(0))?;

    Ok(ContextLoadRate {
        status: MetricStatus::Unavailable,
        unavailable_reason: Some(LOAD_RATE_UNAVAILABLE),
        sessions_in_window,
        retrieval_reach: query_retrieval_reach(conn, project)?,
    })
}

/// All-time share of memories that `search` has returned at least once.
///
/// Deliberately does not touch `updated_at`: `active_count` is a monotonic
/// counter incremented only on the retrieval paths in `search.rs`, so a
/// non-zero reach is proof that memories are reaching callers. It is a proxy,
/// not the load rate — it cannot be windowed or attributed to a session.
fn query_retrieval_reach(conn: &Connection, project: Option<&str>) -> Result<RetrievalReach> {
    let uri_param = project.map(|p| format!("rememora://projects/{p}/%"));
    let uri_clause = if uri_param.is_some() {
        " AND uri LIKE ?1"
    } else {
        ""
    };
    let sql = format!(
        "SELECT
            COALESCE(SUM(CASE WHEN active_count > 0 THEN 1 ELSE 0 END), 0) as retrieved,
            COUNT(*) as total
         FROM contexts
         WHERE context_type = 'memory'
           AND superseded_by IS NULL{uri_clause}"
    );

    let to_reach = |row: &rusqlite::Row| -> rusqlite::Result<RetrievalReach> {
        let retrieved: i64 = row.get(0)?;
        let total: i64 = row.get(1)?;
        Ok(RetrievalReach {
            retrieved_memories: retrieved,
            total_memories: total,
            reach_pct: (total > 0).then(|| (retrieved as f64 / total as f64) * 100.0),
        })
    };

    let result = match &uri_param {
        Some(v) => conn.query_row(&sql, rusqlite::params![v], to_reach)?,
        None => conn.query_row(&sql, [], to_reach)?,
    };

    Ok(result)
}

fn query_transfer_success(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<TransferSuccess> {
    let (session_where, param_values) = build_session_filter(cutoff, project);

    // Transferred sessions that got a follow-up within 1hr
    let sql = format!(
        "WITH transferred AS (
            SELECT id, ended_at, project FROM sessions
            WHERE {session_where} AND status = 'transferred'
         )
         SELECT
            (SELECT COUNT(*) FROM transferred) as total_transferred,
            COUNT(DISTINCT t.id) as picked_up
         FROM transferred t
         WHERE EXISTS (
             SELECT 1 FROM sessions s2
             WHERE s2.parent_session = t.id
               AND s2.started_at <= datetime(t.ended_at, '+1 hour')
         )"
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let result = stmt.query_row(params_ref.as_slice(), |row| {
        let transferred: i64 = row.get(0)?;
        let picked_up: i64 = row.get(1)?;
        Ok(TransferSuccess {
            transferred,
            picked_up,
            pickup_pct: if transferred > 0 {
                (picked_up as f64 / transferred as f64) * 100.0
            } else {
                0.0
            },
        })
    })?;

    Ok(result)
}

/// Per-agent breakdown.
///
/// The `memories` column attributes through the session's `agent`, the way
/// `query_per_project` already attributes through its `project`. It used to
/// additionally require `c.source_agent = ws.agent`, which was a third
/// structural zero in this file: `--agent` is optional on `save` and no
/// caller passes it — not the curator prompt, not the skills, not the hooks —
/// so `source_agent` is NULL on every real row and the count was always 0.
fn query_per_agent(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<Vec<AgentBreakdown>> {
    let (session_where, param_values) = build_session_filter(cutoff, project);

    let sql_cte = format!(
        "WITH window_sessions AS (
            SELECT * FROM sessions WHERE {session_where}
         )
         SELECT
            ws.agent,
            COUNT(*) as sessions,
            SUM(CASE WHEN ws.status = 'ended' THEN 1 ELSE 0 END) as ended,
            SUM(CASE WHEN ws.status = 'active' THEN 1 ELSE 0 END) as orphaned,
            COALESCE((
                SELECT COUNT(*) FROM contexts c
                WHERE c.context_type = 'memory'
                  AND c.source_session IN (
                      SELECT id FROM window_sessions ws2 WHERE ws2.agent = ws.agent
                  )
            ), 0) as memories
         FROM window_sessions ws
         GROUP BY ws.agent
         ORDER BY sessions DESC"
    );

    let mut stmt = conn.prepare(&sql_cte)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            let sessions: i64 = row.get(1)?;
            let ended: i64 = row.get(2)?;
            let orphaned: i64 = row.get(3)?;
            let memories: i64 = row.get(4)?;
            Ok(AgentBreakdown {
                agent: row.get(0)?,
                sessions,
                ended,
                orphaned,
                compliance_pct: if sessions > 0 {
                    ((sessions - orphaned) as f64 / sessions as f64) * 100.0
                } else {
                    0.0
                },
                memories,
                avg_memories: if sessions > 0 {
                    memories as f64 / sessions as f64
                } else {
                    0.0
                },
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

fn query_per_project(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<Vec<ProjectBreakdown>> {
    let (session_where, param_values) = build_session_filter(cutoff, project);

    let sql = format!(
        "WITH window_sessions AS (
            SELECT * FROM sessions WHERE {session_where} AND project IS NOT NULL
         )
         SELECT
            ws.project,
            COUNT(*) as sessions,
            COALESCE((
                SELECT COUNT(*) FROM contexts c
                WHERE c.context_type = 'memory'
                  AND c.source_session IN (SELECT id FROM window_sessions ws2 WHERE ws2.project = ws.project)
            ), 0) as memories
         FROM window_sessions ws
         GROUP BY ws.project
         ORDER BY sessions DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            let sessions: i64 = row.get(1)?;
            let memories: i64 = row.get(2)?;
            Ok(ProjectBreakdown {
                project: row.get(0)?,
                sessions,
                memories,
                avg_memories: if sessions > 0 {
                    memories as f64 / sessions as f64
                } else {
                    0.0
                },
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Build WHERE clause and params for session queries filtered by time window and optional project.
fn build_session_filter(
    cutoff: &str,
    project: Option<&str>,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let mut conditions = vec![format!("started_at >= {cutoff}")];
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(proj) = project {
        conditions.push(format!("project = ?{}", params.len() + 1));
        params.push(Box::new(proj.to_string()));
    }

    (conditions.join(" AND "), params)
}

fn print_report(report: &EvalReport) {
    let sc = &report.session_compliance;
    let ms = &report.memory_save_rate;
    let cl = &report.context_load_rate;
    let ts = &report.transfer_success;

    println!("Rememora Eval Report");
    println!("====================");
    if let Some(ref proj) = report.project {
        println!("  Project: {proj}");
    }
    println!("  Window:  last {} days\n", report.window_days);

    println!("Session Compliance");
    println!("------------------");
    println!("  Total sessions:  {}", sc.total);
    println!("  Properly ended:  {}", sc.ended);
    println!("  Transferred:     {}", sc.transferred);
    println!("  Orphaned:        {}", sc.orphaned);
    println!("  Compliance rate: {:.1}%\n", sc.compliance_pct);

    println!("Memory Save Rate");
    println!("----------------");
    match (ms.avg_per_session, ms.zero_save_pct) {
        (Some(avg), Some(zero_pct)) => {
            println!("  Total memories:      {}", ms.total_memories);
            println!("  Avg per session:     {avg:.1}");
            println!(
                "  Sessions w/ 0 saves: {} ({zero_pct:.1}%)",
                ms.sessions_with_zero_saves
            );
        }
        _ => println!(
            "  UNAVAILABLE — {}",
            ms.unavailable_reason.unwrap_or("reason unrecorded")
        ),
    }
    if ms.unattributed_memories > 0 {
        println!(
            "  Unattributed:        {} memories saved in this window with no session\n\
             \x20                      (written before save-time attribution, or outside a\n\
             \x20                      session — not backfillable, so not counted above)",
            ms.unattributed_memories
        );
    }
    println!();

    println!("Context Load Rate");
    println!("-----------------");
    println!(
        "  Per-session load rate: UNAVAILABLE (not 0%) — {}",
        cl.unavailable_reason.unwrap_or("reason unrecorded")
    );
    println!("  Sessions in window:    {}", cl.sessions_in_window);
    let rr = &cl.retrieval_reach;
    match rr.reach_pct {
        Some(pct) => println!(
            "  Retrieval reach:       {} / {} memories returned by search at least once \
             ({pct:.1}%, all-time)",
            rr.retrieved_memories, rr.total_memories
        ),
        None => println!("  Retrieval reach:       no memories stored yet"),
    }
    println!();

    println!("Transfer Success");
    println!("----------------");
    println!("  Transferred: {}", ts.transferred);
    println!("  Picked up:   {}", ts.picked_up);
    println!("  Pickup rate: {:.1}%\n", ts.pickup_pct);

    if !report.per_agent.is_empty() {
        println!("Per-Agent Breakdown");
        println!("-------------------");
        for a in &report.per_agent {
            println!(
                "  {}: {} sessions, {:.1}% compliance, {} memories ({:.1}/session)",
                a.agent, a.sessions, a.compliance_pct, a.memories, a.avg_memories
            );
        }
        println!();
    }

    if !report.per_project.is_empty() {
        println!("Per-Project Breakdown");
        println!("---------------------");
        for p in &report.per_project {
            println!(
                "  {}: {} sessions, {} memories ({:.1}/session)",
                p.project, p.sessions, p.memories, p.avg_memories
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn setup_test_db() -> Connection {
        let conn = db::open_memory().unwrap();

        // Insert test sessions
        conn.execute_batch(&format!(
            "INSERT INTO sessions (id, agent, project, started_at, ended_at, status, intent, summary)
             VALUES
                ('s1', 'claude-code', 'myapp', datetime('now', '-5 days'), datetime('now', '-5 days', '+1 hour'), 'ended', 'fix bug', 'fixed it'),
                ('s2', 'claude-code', 'myapp', datetime('now', '-3 days'), datetime('now', '-3 days', '+30 minutes'), 'transferred', 'add feature', 'partial'),
                ('s3', 'codex', 'myapp', datetime('now', '-3 days', '+45 minutes'), datetime('now', '-3 days', '+2 hours'), 'ended', 'continue feature', 'done'),
                ('s4', 'claude-code', 'other', datetime('now', '-1 day'), NULL, 'active', 'debug', '');"
        )).unwrap();

        // s3 is a follow-up to s2 (transfer chain)
        conn.execute(
            "UPDATE sessions SET parent_session = 's2' WHERE id = 's3'",
            [],
        ).unwrap();

        // Insert test memories linked to sessions
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute_batch(&format!(
            "INSERT INTO contexts (id, uri, context_type, category, name, source_agent, source_session, importance, created_at, updated_at)
             VALUES
                ('m1', 'rememora://projects/myapp/memory/m1', 'memory', 'entity', 'mem1', 'claude-code', 's1', 0.5, '{now}', '{now}'),
                ('m2', 'rememora://projects/myapp/memory/m2', 'memory', 'decision', 'mem2', 'claude-code', 's1', 0.8, '{now}', '{now}'),
                ('m3', 'rememora://projects/myapp/memory/m3', 'memory', 'pattern', 'mem3', 'codex', 's3', 0.6, '{now}', '{now}');"
        )).unwrap();

        conn
    }

    #[test]
    fn test_session_compliance() {
        let conn = setup_test_db();
        let cutoff = "datetime('now', '-30 days')";
        let result = query_session_compliance(&conn, cutoff, None).unwrap();
        assert_eq!(result.total, 4);
        assert_eq!(result.ended, 2);
        assert_eq!(result.transferred, 1);
        assert_eq!(result.orphaned, 1);
        assert!((result.compliance_pct - 75.0).abs() < 0.1);
    }

    #[test]
    fn test_session_compliance_project_filter() {
        let conn = setup_test_db();
        let cutoff = "datetime('now', '-30 days')";
        let result = query_session_compliance(&conn, cutoff, Some("myapp")).unwrap();
        assert_eq!(result.total, 3);
        assert_eq!(result.orphaned, 0);
        assert!((result.compliance_pct - 100.0).abs() < 0.1);
    }

    #[test]
    fn test_memory_save_rate() {
        let conn = setup_test_db();
        let cutoff = "datetime('now', '-30 days')";
        let result = query_memory_save_rate(&conn, cutoff, None).unwrap();
        assert_eq!(result.status, MetricStatus::Available);
        assert_eq!(result.total_memories, 3);
        assert!((result.avg_per_session.unwrap() - 0.75).abs() < 0.1);
        // s2 and s4 have zero saves
        assert_eq!(result.sessions_with_zero_saves, 2);
        // Every fixture memory carries a source_session.
        assert_eq!(result.unattributed_memories, 0);
    }

    /// The failure mode this whole change exists to prevent: memories are
    /// being written, the save rate reads 0, and nothing says why.
    ///
    /// Rows with a NULL `source_session` — everything written before
    /// save-time attribution landed — must surface as `unattributed`, not
    /// vanish into a rate of zero.
    #[test]
    fn test_memory_save_rate_surfaces_unattributed_memories() {
        let conn = setup_test_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute_batch(&format!(
            "INSERT INTO contexts (id, uri, context_type, category, name, source_agent, source_session, importance, created_at, updated_at)
             VALUES
                ('u1', 'rememora://projects/myapp/memory/u1', 'memory', 'case', 'legacy1', 'claude-code', NULL, 0.5, '{now}', '{now}'),
                ('u2', 'rememora://projects/myapp/memory/u2', 'memory', 'case', 'legacy2', 'claude-code', NULL, 0.5, '{now}', '{now}');"
        )).unwrap();

        let result = query_memory_save_rate(&conn, "datetime('now', '-30 days')", None).unwrap();
        assert_eq!(result.unattributed_memories, 2);
        // The attributed count must not silently absorb them.
        assert_eq!(result.total_memories, 3);
    }

    /// An empty window has no denominator. Report that, don't emit 0.0%.
    #[test]
    fn test_memory_save_rate_unavailable_without_sessions() {
        let conn = db::open_memory().unwrap();
        let result = query_memory_save_rate(&conn, "datetime('now', '-30 days')", None).unwrap();
        assert_eq!(result.status, MetricStatus::Unavailable);
        assert!(result.avg_per_session.is_none());
        assert!(result.zero_save_pct.is_none());
        assert!(result.unavailable_reason.is_some());
    }

    /// The per-session load rate has no signal behind it yet. It must say so
    /// rather than report a number people will act on.
    #[test]
    fn test_context_load_rate_reports_unavailable_not_zero() {
        let conn = setup_test_db();
        let result = query_context_load_rate(&conn, "datetime('now', '-30 days')", None).unwrap();
        assert_eq!(result.status, MetricStatus::Unavailable);
        assert!(result.unavailable_reason.is_some());
        // The denominator is still honest and observable.
        assert_eq!(result.sessions_in_window, 4);
    }

    /// Structural-zero guard for the retrieval proxy: drive the real search
    /// path, then assert reach is non-zero. Seeding `active_count` by hand
    /// would pass even if `search` stopped bumping it.
    #[test]
    fn test_retrieval_reach_nonzero_after_real_search() {
        let conn = setup_test_db();

        let before = query_retrieval_reach(&conn, None).unwrap();
        assert_eq!(before.retrieved_memories, 0, "fixture starts unretrieved");
        assert_eq!(before.total_memories, 3);

        let hits = rememora::search::search(&conn, "mem1", None, None, 10).unwrap();
        assert!(!hits.is_empty(), "fixture must be findable via FTS5");

        let after = query_retrieval_reach(&conn, None).unwrap();
        assert!(
            after.retrieved_memories > 0,
            "search returned {} hits but retrieval reach stayed 0 — the metric \
             has come unhooked from the retrieval path",
            hits.len()
        );
        assert!(after.reach_pct.unwrap() > 0.0);
    }

    #[test]
    fn test_retrieval_reach_none_on_empty_corpus() {
        let conn = db::open_memory().unwrap();
        let result = query_retrieval_reach(&conn, None).unwrap();
        assert_eq!(result.total_memories, 0);
        assert!(result.reach_pct.is_none(), "0/0 must be null, not 0.0%");
    }

    #[test]
    fn test_transfer_success() {
        let conn = setup_test_db();
        let cutoff = "datetime('now', '-30 days')";
        let result = query_transfer_success(&conn, cutoff, None).unwrap();
        assert_eq!(result.transferred, 1);
        assert_eq!(result.picked_up, 1);
        assert!((result.pickup_pct - 100.0).abs() < 0.1);
    }

    #[test]
    fn test_per_agent_breakdown() {
        let conn = setup_test_db();
        let cutoff = "datetime('now', '-30 days')";
        let result = query_per_agent(&conn, cutoff, None).unwrap();
        assert_eq!(result.len(), 2);
        let claude = result.iter().find(|a| a.agent == "claude-code").unwrap();
        assert_eq!(claude.sessions, 3);
        assert_eq!(claude.orphaned, 1);
        assert_eq!(claude.memories, 2);
    }

    /// `--agent` is optional on `save` and nothing passes it, so `source_agent`
    /// is NULL on real rows. Per-agent counts must come from the session.
    #[test]
    fn test_per_agent_counts_memories_with_null_source_agent() {
        let conn = setup_test_db();
        conn.execute("UPDATE contexts SET source_agent = NULL", []).unwrap();

        let result = query_per_agent(&conn, "datetime('now', '-30 days')", None).unwrap();
        let claude = result.iter().find(|a| a.agent == "claude-code").unwrap();
        assert_eq!(
            claude.memories, 2,
            "memories must be attributed via the session's agent, not the \
             never-populated `source_agent` column"
        );
    }

    #[test]
    fn test_per_project_breakdown() {
        let conn = setup_test_db();
        let cutoff = "datetime('now', '-30 days')";
        let result = query_per_project(&conn, cutoff, None).unwrap();
        assert_eq!(result.len(), 2);
        let myapp = result.iter().find(|p| p.project == "myapp").unwrap();
        assert_eq!(myapp.sessions, 3);
        assert_eq!(myapp.memories, 3);
    }

    #[test]
    fn test_empty_db() {
        let conn = db::open_memory().unwrap();
        let args = EvalArgs { project: None, days: 30 };
        // Should not error on empty DB
        run(&conn, &args, false).unwrap();
    }

    #[test]
    fn test_json_output() {
        let conn = setup_test_db();
        let args = EvalArgs { project: None, days: 30 };
        // Should not error
        run(&conn, &args, true).unwrap();
    }
}
