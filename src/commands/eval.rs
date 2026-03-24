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

#[derive(Debug, Serialize)]
struct MemorySaveRate {
    total_memories: i64,
    avg_per_session: f64,
    sessions_with_zero_saves: i64,
    zero_save_pct: f64,
}

#[derive(Debug, Serialize)]
struct ContextLoadRate {
    sessions_with_load: i64,
    total_sessions: i64,
    load_pct: f64,
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

    Ok(MemorySaveRate {
        total_memories,
        avg_per_session: if total_sessions > 0 {
            total_memories as f64 / total_sessions as f64
        } else {
            0.0
        },
        sessions_with_zero_saves: zero_saves,
        zero_save_pct: if total_sessions > 0 {
            (zero_saves as f64 / total_sessions as f64) * 100.0
        } else {
            0.0
        },
    })
}

fn query_context_load_rate(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<ContextLoadRate> {
    let (session_where, param_values) = build_session_filter(cutoff, project);

    // Sessions where at least one context had active_count bumped within 60s of session start
    let sql_cte = format!(
        "WITH window_sessions AS (
            SELECT id, started_at FROM sessions WHERE {session_where}
         )
         SELECT
            (SELECT COUNT(*) FROM window_sessions) as total_sessions,
            COUNT(DISTINCT ws.id) as sessions_with_load
         FROM window_sessions ws
         WHERE EXISTS (
             SELECT 1 FROM contexts c
             WHERE c.updated_at >= ws.started_at
               AND c.updated_at <= datetime(ws.started_at, '+60 seconds')
               AND c.active_count > 0
         )"
    );

    let mut stmt = conn.prepare(&sql_cte)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let result = stmt.query_row(params_ref.as_slice(), |row| {
        let total: i64 = row.get(0)?;
        let with_load: i64 = row.get(1)?;
        Ok(ContextLoadRate {
            sessions_with_load: with_load,
            total_sessions: total,
            load_pct: if total > 0 {
                (with_load as f64 / total as f64) * 100.0
            } else {
                0.0
            },
        })
    })?;

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
                  AND c.source_agent = ws.agent
                  AND c.source_session IN (SELECT id FROM window_sessions)
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
    println!("  Total memories:      {}", ms.total_memories);
    println!("  Avg per session:     {:.1}", ms.avg_per_session);
    println!("  Sessions w/ 0 saves: {} ({:.1}%)\n", ms.sessions_with_zero_saves, ms.zero_save_pct);

    println!("Context Load Rate");
    println!("-----------------");
    println!("  Sessions with load: {} / {}", cl.sessions_with_load, cl.total_sessions);
    println!("  Load rate:          {:.1}%\n", cl.load_pct);

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
        assert_eq!(result.total_memories, 3);
        assert!((result.avg_per_session - 0.75).abs() < 0.1);
        // s2 and s4 have zero saves
        assert_eq!(result.sessions_with_zero_saves, 2);
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
