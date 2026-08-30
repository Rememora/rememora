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

/// Window-scoped save accounting.
///
/// The three `*_memories` buckets **partition** `memories_created_in_window`:
///
/// ```text
/// memories_created_in_window
///     = attributed_memories            (session started inside the window)
///     + memories_from_earlier_sessions (session started before it)
///     + unattributed_memories          (no session at all)
/// ```
///
/// That identity is the point. The previous shape reported only the first and
/// third buckets, so a memory written today into a session that started before
/// the window — a long-running agent, a session the user never ended — was
/// counted in neither and simply vanished from the report. Every field here is
/// window-scoped; the all-time corpus counts live in [`RetrievalReach`].
#[derive(Debug, Serialize)]
struct MemorySaveRate {
    status: MetricStatus,
    unavailable_reason: Option<&'static str>,
    /// Every memory row created inside the window. The sum of the three
    /// buckets below, so nothing can disappear between them.
    memories_created_in_window: i64,
    /// Memories attributed to a session that started inside the window.
    /// The numerator of `avg_per_session` — the only bucket the rate uses.
    attributed_memories: i64,
    /// Memories created inside the window whose session started outside it.
    ///
    /// Real behavior, not a defect: a session that outlives the window still
    /// saves. Excluded from the rate because its session is not in the
    /// denominator, reported because it happened. Also catches a
    /// `source_session` pointing at a session that no longer exists, and —
    /// under `--project` — one belonging to a different project.
    memories_from_earlier_sessions: i64,
    /// Memories created inside the window carrying no `source_session`.
    ///
    /// Non-zero for every row written before save-time attribution existed,
    /// plus every legitimate save made outside a session. Those rows cannot be
    /// backfilled — nothing in the schema records which session was open when
    /// they were written — so they are reported, not imputed into
    /// `attributed_memories`.
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
/// Counts memories with `active_count > 0`. `active_count` is bumped by
/// `context::bump_active_count`, which has three callers today:
///
///   * `search::search` and `search::hybrid_search` — once per returned hit
///   * `commands::get` — a memory fetched directly by URI
///   * `commands::timeline` — the anchor row of a timeline
///
/// So a non-zero reach means **some retrieval path handed the memory to a
/// caller**. It does *not* prove search returned anything: a single
/// `rememora get` moves this metric on its own. Reading it as a search-recall
/// number is exactly the over-claim this file exists to stop.
///
/// `active_count` carries no timestamp and no session id, so this is an
/// **all-time, corpus-level** figure — it cannot be windowed by `--days`, it
/// cannot be attributed to a session, and it must not be read as a per-session
/// load rate.
///
/// The set of `bump_active_count` callers *is* the definition, so a new caller
/// widens this silently. `tests/test_eval_metrics.rs` pins both halves of the
/// claim: that `search` moves it, and that a bare `get` does too.
#[derive(Debug, Serialize)]
struct RetrievalReach {
    /// Memories some retrieval path has returned at least once (all-time).
    reached_memories: i64,
    /// Every live (non-superseded) memory in the store, all-time — URI-scoped
    /// when `--project` is given.
    ///
    /// Deliberately not named `total_memories`: it is a different quantity
    /// from anything in [`MemorySaveRate`], which is window-scoped and scoped
    /// by session as well as URI.
    corpus_memories: i64,
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
    let (where_clause, param_values) = build_session_filter(cutoff, project, "");

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

    let result = stmt.query_row(bind(&param_values).as_slice(), |row| {
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
/// This joins on `contexts.source_session`, which every memory-creation path
/// hardcoded to `None`. The column was never populated, so the rate was 0
/// regardless of behavior. `save` and `extract --save` now attribute to the
/// active session; rows written before that cannot be recovered, so they are
/// counted separately instead of silently dragging the rate to zero.
///
/// The counts are computed as one partition of "memories created in the
/// window" so that every such row lands in exactly one bucket — see
/// [`MemorySaveRate`]. Splitting them across separate queries is how a memory
/// saved into a session older than the window came to be reported nowhere.
fn query_memory_save_rate(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<MemorySaveRate> {
    let (session_where, mut params) = build_session_filter(cutoff, project, "");
    let (aliased_session_where, _) = build_session_filter(cutoff, project, "s.");

    // Total sessions in window — the denominator.
    let sql = format!("SELECT COUNT(*) FROM sessions WHERE {session_where}");
    let total_sessions: i64 = conn
        .prepare(&sql)?
        .query_row(bind(&params).as_slice(), |row| row.get(0))?;

    // Sessions with zero saves.
    let sql = format!(
        "SELECT COUNT(*)
         FROM sessions s
         WHERE {aliased_session_where}
           AND NOT EXISTS (
               SELECT 1 FROM contexts c
               WHERE c.source_session = s.id AND c.context_type = 'memory'
           )"
    );
    let zero_saves: i64 = conn
        .prepare(&sql)?
        .query_row(bind(&params).as_slice(), |row| row.get(0))?;

    // The save accounting: one pass over the memories created in the window,
    // counting the whole and the two buckets that can be stated positively.
    //
    // `contexts` has no project column. A memory belongs to a project if its
    // URI says so (`save --project`) or if the session that wrote it does
    // (`save` with no flag inside a project session writes a project-less
    // URI). Scoping by URI alone would drop the second kind out of the
    // partition while the session join still counted it.
    //
    // `datetime()` wraps both sides of the window comparison for the reason
    // spelled out on `build_session_filter`: `created_at` is RFC3339 and
    // `datetime('now', ...)` is not, and raw string comparison between them is
    // off by up to a day.
    //
    // `:uri_prefix` is bound only here, and pushed only now — the two queries
    // above do not mention it, and rusqlite rejects a named parameter the
    // statement does not use.
    let memory_scope = if let Some(proj) = project {
        params.push((
            ":uri_prefix",
            Box::new(format!("rememora://projects/{proj}/%")),
        ));
        "AND (c.uri LIKE :uri_prefix
              OR EXISTS (SELECT 1 FROM sessions sp
                         WHERE sp.id = c.source_session AND sp.project = :project))"
    } else {
        ""
    };

    let sql = format!(
        "SELECT
            COUNT(*) as created_in_window,
            COALESCE(SUM(CASE WHEN c.source_session IN (
                SELECT id FROM sessions WHERE {session_where}
            ) THEN 1 ELSE 0 END), 0) as attributed,
            COALESCE(SUM(CASE WHEN c.source_session IS NULL THEN 1 ELSE 0 END), 0) as unattributed
         FROM contexts c
         WHERE c.context_type = 'memory'
           AND datetime(c.created_at) >= datetime({cutoff})
           {memory_scope}"
    );

    let (created_in_window, attributed, unattributed): (i64, i64, i64) = conn
        .prepare(&sql)?
        .query_row(bind(&params).as_slice(), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;

    // The third bucket is the remainder, not a third predicate. Both counted
    // buckets are disjoint subsets of `created_in_window`, so this is an
    // arithmetic identity: whatever a memory's attribution turns out to be —
    // a session older than the window, a session belonging to another project,
    // a dangling `source_session`, or a case SQL's three-valued logic declines
    // to decide — it is still counted and still on the page. Writing it as a
    // `NOT IN` predicate instead would let a row match none of the three and
    // vanish, which is the defect this shape exists to make impossible.
    let from_earlier = created_in_window - attributed - unattributed;

    // With no sessions in the window there is no denominator. Report that
    // rather than a 0.0 that reads as "nobody saved anything". The counts
    // above still stand — they are observations, not derived rates.
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
        memories_created_in_window: created_in_window,
        attributed_memories: attributed,
        memories_from_earlier_sessions: from_earlier,
        unattributed_memories: unattributed,
        sessions_in_window: total_sessions,
        avg_per_session: available.then(|| attributed as f64 / total_sessions as f64),
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
/// every retrieval, so the metric was entangled with retrieval rather than
/// load.
///
/// The correct long-term signal is a per-retrieval `last_accessed_at`
/// timestamp on `contexts`, added by a migration on a separate branch; once it
/// lands, this becomes "sessions where some context was accessed between
/// `started_at` and `ended_at`" and the `Unavailable` branch can be deleted.
///
/// DEPENDENCY: that same branch also stops `bump_active_count` writing
/// `updated_at`. Nothing here reads `updated_at`, and [`RetrievalReach`] reads
/// only the `active_count` counter, so this file is deliberately unaffected by
/// that change either way — it is written against the schema as it exists
/// today.
fn query_context_load_rate(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<ContextLoadRate> {
    let (session_where, param_values) = build_session_filter(cutoff, project, "");

    let sql = format!("SELECT COUNT(*) FROM sessions WHERE {session_where}");
    let sessions_in_window: i64 = conn
        .prepare(&sql)?
        .query_row(bind(&param_values).as_slice(), |row| row.get(0))?;

    Ok(ContextLoadRate {
        status: MetricStatus::Unavailable,
        unavailable_reason: Some(LOAD_RATE_UNAVAILABLE),
        sessions_in_window,
        retrieval_reach: query_retrieval_reach(conn, project)?,
    })
}

/// All-time share of memories that a retrieval path has handed to a caller.
///
/// Deliberately does not touch `updated_at`: `active_count` is a monotonic
/// counter, so a non-zero reach is proof that memories are reaching callers.
/// It is a proxy, not the load rate — it cannot be windowed or attributed to a
/// session, and "a caller" means `search`, `get`, or `timeline`, not `search`
/// alone. See [`RetrievalReach`] for exactly what it does and does not claim.
fn query_retrieval_reach(conn: &Connection, project: Option<&str>) -> Result<RetrievalReach> {
    let uri_param = project.map(|p| format!("rememora://projects/{p}/%"));
    let uri_clause = if uri_param.is_some() {
        " AND uri LIKE ?1"
    } else {
        ""
    };
    let sql = format!(
        "SELECT
            COALESCE(SUM(CASE WHEN active_count > 0 THEN 1 ELSE 0 END), 0) as reached,
            COUNT(*) as corpus
         FROM contexts
         WHERE context_type = 'memory'
           AND superseded_by IS NULL{uri_clause}"
    );

    let to_reach = |row: &rusqlite::Row| -> rusqlite::Result<RetrievalReach> {
        let reached: i64 = row.get(0)?;
        let corpus: i64 = row.get(1)?;
        Ok(RetrievalReach {
            reached_memories: reached,
            corpus_memories: corpus,
            reach_pct: (corpus > 0).then(|| (reached as f64 / corpus as f64) * 100.0),
        })
    };

    let result = match &uri_param {
        Some(v) => conn.query_row(&sql, rusqlite::params![v], to_reach)?,
        None => conn.query_row(&sql, [], to_reach)?,
    };

    Ok(result)
}

/// Transfer pickup — how often a handed-off session was actually resumed.
///
/// The pickup window compares `datetime(s2.started_at)`, not the raw column.
/// `started_at` is RFC3339 and `datetime(t.ended_at, '+1 hour')` is not, so as
/// raw strings `'2026-08-28T21:05:00+00:00' <= '2026-08-28 22:00:00'` is
/// false: every same-day pickup failed the bound and the rate read 0. The
/// in-module fixtures seeded `started_at` with `datetime('now', ...)`, which
/// is not the format `session::start` writes, so they passed over it — they
/// now use RFC3339 like the real write path.
fn query_transfer_success(
    conn: &Connection,
    cutoff: &str,
    project: Option<&str>,
) -> Result<TransferSuccess> {
    let (session_where, param_values) = build_session_filter(cutoff, project, "");

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
               AND datetime(s2.started_at) <= datetime(t.ended_at, '+1 hour')
         )"
    );

    let mut stmt = conn.prepare(&sql)?;

    let result = stmt.query_row(bind(&param_values).as_slice(), |row| {
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
    let (session_where, param_values) = build_session_filter(cutoff, project, "");

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

    let rows = stmt
        .query_map(bind(&param_values).as_slice(), |row| {
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
    let (session_where, param_values) = build_session_filter(cutoff, project, "");

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

    let rows = stmt
        .query_map(bind(&param_values).as_slice(), |row| {
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

/// Named bind parameters, owned so they can outlive the query builder.
///
/// Named rather than positional: several queries below splice the session
/// filter into a larger statement that binds parameters of its own, and
/// positional indices would have to be renumbered at every splice site — the
/// kind of bookkeeping that silently binds the wrong value.
type NamedParams = Vec<(&'static str, Box<dyn rusqlite::types::ToSql>)>;

/// Borrow a [`NamedParams`] into the slice shape rusqlite binds from.
fn bind(params: &NamedParams) -> Vec<(&str, &dyn rusqlite::types::ToSql)> {
    params
        .iter()
        .map(|(name, value)| (*name, value.as_ref()))
        .collect()
}

/// Build WHERE clause and params for session queries filtered by time window and optional project.
///
/// `alias` is the table qualifier to prefix columns with (`""` or `"s."`).
/// It used to be applied by `String::replace` at the one call site that needed
/// it, which would have silently mangled this clause the moment its wording
/// changed.
///
/// Both sides of the window comparison go through `datetime()`. `started_at`
/// is RFC3339 (`2026-07-31T08:00:00+00:00`) while `datetime('now', ...)`
/// renders `2026-07-31 20:51:00`; compared as raw strings, `'T'` (0x54) sorts
/// above `' '` (0x20), so every session from earlier on the cutoff day tested
/// as inside the window and `--days N` really meant "N days plus up to one".
/// `datetime()` also normalizes the UTC offset, so a session recorded in a
/// non-UTC offset compares at the correct instant rather than by wall clock.
fn build_session_filter(cutoff: &str, project: Option<&str>, alias: &str) -> (String, NamedParams) {
    let mut conditions = vec![format!("datetime({alias}started_at) >= datetime({cutoff})")];
    let mut params: NamedParams = Vec::new();

    if let Some(proj) = project {
        conditions.push(format!("{alias}project = :project"));
        params.push((":project", Box::new(proj.to_string())));
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
    // The three buckets are printed unconditionally and always sum to the
    // first line. A memory saved into a session older than the window used to
    // appear on none of them and silently leave the report.
    println!(
        "  Memories saved in window:            {}",
        ms.memories_created_in_window
    );
    println!(
        "    in a session started in window:    {}  <- the rate below",
        ms.attributed_memories
    );
    println!(
        "    in a session started earlier:      {}",
        ms.memories_from_earlier_sessions
    );
    println!(
        "    with no session attribution:       {}",
        ms.unattributed_memories
    );
    match (ms.avg_per_session, ms.zero_save_pct) {
        (Some(avg), Some(zero_pct)) => {
            println!("  Sessions in window:                  {}", ms.sessions_in_window);
            println!("  Avg per session:                     {avg:.1}");
            println!(
                "  Sessions w/ 0 saves:                 {} ({zero_pct:.1}%)",
                ms.sessions_with_zero_saves
            );
        }
        _ => println!(
            "  Rate UNAVAILABLE — {}",
            ms.unavailable_reason.unwrap_or("reason unrecorded")
        ),
    }
    if ms.memories_from_earlier_sessions > 0 || ms.unattributed_memories > 0 {
        println!(
            "  Note: memories outside the first bucket are excluded from the rate — an\n\
             \x20       earlier session is not in the denominator, and an unattributed row\n\
             \x20       (saved outside a session, or written before save-time attribution)\n\
             \x20       cannot be backfilled. They are counted here, not dropped."
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
        // Says "a retrieval path", not "search": `active_count` is also bumped
        // by `get` and by `timeline`, so a single `rememora get` moves this.
        Some(pct) => {
            println!(
                "  Retrieval reach:       {} / {} memories reached a caller at least once \
                 ({pct:.1}%)",
                rr.reached_memories, rr.corpus_memories
            );
            println!(
                "                         all-time, whole corpus; counts search, get and\n\
                 \x20                        timeline alike — not proof that search returned\n\
                 \x20                        anything, and not windowed by --days"
            );
        }
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

    /// RFC3339, the format `session::start` and `context::insert` actually
    /// write (`chrono::Utc::now().to_rfc3339()`).
    ///
    /// These fixtures used to seed `datetime('now', ...)`, which renders
    /// `2026-08-25 20:51:00` — a format nothing in the codebase produces. That
    /// is why the unit tests passed over two string-comparison defects:
    /// sessions from earlier on the cutoff day counting as inside the window,
    /// and every transfer pickup failing its `+1 hour` bound. Fixtures must be
    /// written in the format under test or they test something else.
    fn rfc3339(offset: chrono::Duration) -> String {
        (chrono::Utc::now() + offset).to_rfc3339()
    }

    fn setup_test_db() -> Connection {
        let conn = db::open_memory().unwrap();

        let d = chrono::Duration::days;
        let m = chrono::Duration::minutes;

        // Insert test sessions
        conn.execute_batch(&format!(
            "INSERT INTO sessions (id, agent, project, started_at, ended_at, status, intent, summary)
             VALUES
                ('s1', 'claude-code', 'myapp', '{s1_start}', '{s1_end}', 'ended', 'fix bug', 'fixed it'),
                ('s2', 'claude-code', 'myapp', '{s2_start}', '{s2_end}', 'transferred', 'add feature', 'partial'),
                ('s3', 'codex', 'myapp', '{s3_start}', '{s3_end}', 'ended', 'continue feature', 'done'),
                ('s4', 'claude-code', 'other', '{s4_start}', NULL, 'active', 'debug', '');",
            s1_start = rfc3339(-d(5)),
            s1_end = rfc3339(-d(5) + m(60)),
            s2_start = rfc3339(-d(3)),
            s2_end = rfc3339(-d(3) + m(30)),
            s3_start = rfc3339(-d(3) + m(45)),
            s3_end = rfc3339(-d(3) + m(120)),
            s4_start = rfc3339(-d(1)),
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
        assert_eq!(result.attributed_memories, 3);
        assert!((result.avg_per_session.unwrap() - 0.75).abs() < 0.1);
        // s2 and s4 have zero saves
        assert_eq!(result.sessions_with_zero_saves, 2);
        // Every fixture memory carries a source_session on a window session.
        assert_eq!(result.unattributed_memories, 0);
        assert_eq!(result.memories_from_earlier_sessions, 0);
        assert_eq!(result.memories_created_in_window, 3);
    }

    /// The accounting hole: a memory written today into a session that started
    /// before the window belongs to no bucket under the old shape — not
    /// `total_memories` (its session is outside the window), not
    /// `unattributed_memories` (it *has* a session) — so it left the report
    /// entirely. Every memory created in the window must land somewhere.
    #[test]
    fn test_memory_created_in_window_from_an_older_session_is_not_lost() {
        let conn = setup_test_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute_batch(&format!(
            "INSERT INTO sessions (id, agent, project, started_at, status, intent)
             VALUES ('old', 'claude-code', 'myapp', '{old_start}', 'active', 'long runner');
             INSERT INTO contexts (id, uri, context_type, category, name, source_agent, source_session, importance, created_at, updated_at)
             VALUES ('mo', 'rememora://projects/myapp/memory/mo', 'memory', 'case', 'from old session', NULL, 'old', 0.5, '{now}', '{now}');",
            old_start = rfc3339(-chrono::Duration::days(40)),
        )).unwrap();

        let result = query_memory_save_rate(&conn, "datetime('now', '-30 days')", None).unwrap();

        assert_eq!(
            result.memories_from_earlier_sessions, 1,
            "a memory saved into a session older than the window must be \
             reported, not silently dropped"
        );
        assert_eq!(
            result.memories_created_in_window, 4,
            "the buckets must account for every memory created in the window"
        );
        assert_eq!(
            result.memories_created_in_window,
            result.attributed_memories
                + result.memories_from_earlier_sessions
                + result.unattributed_memories,
        );
        // And it must not be smuggled into the rate's numerator.
        assert_eq!(result.attributed_memories, 3);
    }

    /// Same hole, at its most visible: the *only* activity in the window is one
    /// memory from an older session. The report used to show 0 / 0 / no
    /// sessions and read as "nothing happened".
    #[test]
    fn test_lone_memory_from_older_session_is_visible_with_no_window_sessions() {
        let conn = db::open_memory().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute_batch(&format!(
            "INSERT INTO sessions (id, agent, project, started_at, status, intent)
             VALUES ('old', 'claude-code', 'myapp', '{old_start}', 'active', 'long runner');
             INSERT INTO contexts (id, uri, context_type, category, name, source_agent, source_session, importance, created_at, updated_at)
             VALUES ('mo', 'rememora://projects/myapp/memory/mo', 'memory', 'case', 'from old session', NULL, 'old', 0.5, '{now}', '{now}');",
            old_start = rfc3339(-chrono::Duration::days(40)),
        )).unwrap();

        let result = query_memory_save_rate(&conn, "datetime('now', '-30 days')", None).unwrap();

        // No denominator, so no rate — but the memory is still on the page.
        assert_eq!(result.status, MetricStatus::Unavailable);
        assert!(result.avg_per_session.is_none());
        assert_eq!(result.memories_created_in_window, 1);
        assert_eq!(result.memories_from_earlier_sessions, 1);
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
        assert_eq!(result.attributed_memories, 3);
        assert_eq!(result.memories_created_in_window, 5);
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
        assert_eq!(before.reached_memories, 0, "fixture starts unreached");
        assert_eq!(before.corpus_memories, 3);

        let hits = rememora::search::search(&conn, "mem1", None, None, 10).unwrap();
        assert!(!hits.is_empty(), "fixture must be findable via FTS5");

        let after = query_retrieval_reach(&conn, None).unwrap();
        assert!(
            after.reached_memories > 0,
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
        assert_eq!(result.corpus_memories, 0);
        assert!(result.reach_pct.is_none(), "0/0 must be null, not 0.0%");
    }

    #[test]
    fn test_transfer_success() {
        let conn = setup_test_db();
        let cutoff = "datetime('now', '-30 days')";
        let result = query_transfer_success(&conn, cutoff, None).unwrap();
        assert_eq!(result.transferred, 1);
        assert_eq!(
            result.picked_up, 1,
            "s3 started 15 minutes after s2 handed off; the `+1 hour` bound \
             must compare instants, not RFC3339 against `datetime()` output"
        );
        assert!((result.pickup_pct - 100.0).abs() < 0.1);
    }

    /// `--days N` must mean N days, not "N days plus up to one".
    ///
    /// `started_at` is RFC3339 and `datetime('now','-N days')` is not; compared
    /// as raw strings `'T'` (0x54) beats `' '` (0x20), so every session from
    /// earlier on the cutoff day tested as inside the window.
    #[test]
    fn test_window_excludes_sessions_started_before_the_cutoff_instant() {
        let conn = db::open_memory().unwrap();
        let h = chrono::Duration::hours;
        let d = chrono::Duration::days;
        conn.execute_batch(&format!(
            "INSERT INTO sessions (id, agent, project, started_at, status, intent)
             VALUES
                ('just_out', 'claude-code', 'p', '{just_out}', 'ended', 'before cutoff'),
                ('just_in', 'claude-code', 'p', '{just_in}', 'ended', 'after cutoff');",
            just_out = rfc3339(-d(30) - h(2)),
            just_in = rfc3339(-d(30) + h(2)),
        ))
        .unwrap();

        let result = query_session_compliance(&conn, "datetime('now', '-30 days')", None).unwrap();
        assert_eq!(
            result.total, 1,
            "the session two hours before the cutoff must be out, the one two \
             hours after it must be in"
        );
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
