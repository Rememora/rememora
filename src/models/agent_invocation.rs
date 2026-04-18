//! Per-invocation telemetry for every LLM call (subagent or direct API).
//!
//! Writers land here from four places:
//! - `curator::call_subagent` (signal_gate + curator)
//! - `commands::extract` (direct Anthropic API)
//! - `commands::evolve` (direct Anthropic API)
//! - `commands::agent_run` (`claude -p` for GitHub issue dispatch)
//!
//! Readers are `rememora usage` (aggregation) and the TUI cost tile.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    SignalGate,
    Curator,
    Extract,
    Evolve,
    Consolidate,
    AgentRun,
}

impl Caller {
    pub fn as_str(self) -> &'static str {
        match self {
            Caller::SignalGate => "signal_gate",
            Caller::Curator => "curator",
            Caller::Extract => "extract",
            Caller::Evolve => "evolve",
            Caller::Consolidate => "consolidate",
            Caller::AgentRun => "agent_run",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct InvocationRecord {
    pub caller: &'static str,
    pub model: String,
    pub project: Option<String>,
    pub parent_session: Option<String>,
    pub child_session: Option<String>,
    pub duration_ms: Option<i64>,
    pub duration_api_ms: Option<i64>,
    pub num_turns: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub stop_reason: Option<String>,
    pub terminal_reason: Option<String>,
    pub is_error: bool,
    pub permission_denials_json: Option<String>,
}

/// Build an `InvocationRecord` from subagent telemetry plus the caller's
/// surrounding context (caller kind, project, parent session).
pub fn record_from_subagent(
    caller: Caller,
    project: Option<String>,
    parent_session: Option<String>,
    telemetry: &crate::curator::SubagentTelemetry,
) -> InvocationRecord {
    InvocationRecord {
        caller: caller.as_str(),
        model: telemetry.model.clone(),
        project,
        parent_session,
        child_session: telemetry.child_session_id.clone(),
        duration_ms: telemetry.duration_ms,
        duration_api_ms: telemetry.duration_api_ms,
        num_turns: telemetry.num_turns,
        input_tokens: telemetry.input_tokens,
        output_tokens: telemetry.output_tokens,
        cache_read_tokens: telemetry.cache_read_tokens,
        cache_creation_tokens: telemetry.cache_creation_tokens,
        cost_usd: telemetry.cost_usd,
        stop_reason: telemetry.stop_reason.clone(),
        terminal_reason: telemetry.terminal_reason.clone(),
        is_error: telemetry.is_error,
        permission_denials_json: telemetry.permission_denials_json.clone(),
    }
}

/// Build an `InvocationRecord` from an Anthropic Messages API response body.
///
/// Looks at `response.usage.{input_tokens,output_tokens,cache_*}`. Cost is
/// left `None` — the API doesn't return it and pricing drifts, so we prefer
/// truth over an unmaintained price table.
pub fn record_from_anthropic_api(
    caller: Caller,
    model: &str,
    project: Option<String>,
    parent_session: Option<String>,
    resp_body: &serde_json::Value,
    is_error: bool,
) -> InvocationRecord {
    let usage = resp_body.get("usage");
    InvocationRecord {
        caller: caller.as_str(),
        model: model.to_string(),
        project,
        parent_session,
        child_session: None,
        duration_ms: None,
        duration_api_ms: None,
        num_turns: Some(1),
        input_tokens: usage.and_then(|u| u.get("input_tokens")).and_then(|v| v.as_i64()),
        output_tokens: usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_i64()),
        cache_read_tokens: usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|v| v.as_i64()),
        cache_creation_tokens: usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|v| v.as_i64()),
        cost_usd: None,
        stop_reason: resp_body
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        terminal_reason: None,
        is_error,
        permission_denials_json: None,
    }
}

pub fn insert(conn: &Connection, record: &InvocationRecord) -> Result<String> {
    let id = ulid::Ulid::new().to_string();
    let ts = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO agent_invocations (
            id, ts, caller, model, project, parent_session, child_session,
            duration_ms, duration_api_ms, num_turns,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            cost_usd, stop_reason, terminal_reason, is_error, permission_denials_json
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7,
            ?8, ?9, ?10,
            ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18, ?19
        )",
        params![
            id,
            ts,
            record.caller,
            record.model,
            record.project,
            record.parent_session,
            record.child_session,
            record.duration_ms,
            record.duration_api_ms,
            record.num_turns,
            record.input_tokens,
            record.output_tokens,
            record.cache_read_tokens,
            record.cache_creation_tokens,
            record.cost_usd,
            record.stop_reason,
            record.terminal_reason,
            record.is_error as i64,
            record.permission_denials_json,
        ],
    )?;

    Ok(id)
}

/// Best-effort insert that never raises.
///
/// Telemetry must not break the caller. If the DB is unreachable or the
/// schema drifted, swallow the error and return without propagating.
pub fn try_insert(conn: &Connection, record: &InvocationRecord) {
    if let Err(e) = insert(conn, record) {
        eprintln!("warn: agent_invocation insert failed: {e}");
    }
}

// ── Aggregation ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageAggregate {
    pub bucket: String,
    pub invocations: i64,
    pub errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd: f64,
    pub avg_duration_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Total,
    Caller,
    Model,
    Project,
    ParentSession,
}

impl GroupBy {
    fn column(self) -> Option<&'static str> {
        match self {
            GroupBy::Total => None,
            GroupBy::Caller => Some("caller"),
            GroupBy::Model => Some("model"),
            GroupBy::Project => Some("project"),
            GroupBy::ParentSession => Some("parent_session"),
        }
    }
}

/// Aggregate usage since an ISO8601 cutoff (inclusive).
///
/// `since` is `NULL`-safe: pass `None` to aggregate all history.
pub fn aggregate(
    conn: &Connection,
    since: Option<&str>,
    group_by: GroupBy,
) -> Result<Vec<UsageAggregate>> {
    let (bucket_expr, group_clause) = match group_by.column() {
        None => ("'total'", String::new()),
        Some(col) => (col, format!(" GROUP BY {col}")),
    };

    let sql = format!(
        "SELECT
            COALESCE({bucket_expr}, 'unknown') AS bucket,
            COUNT(*) AS invocations,
            SUM(CASE WHEN is_error = 1 THEN 1 ELSE 0 END) AS errors,
            COALESCE(SUM(input_tokens), 0)          AS input_tokens,
            COALESCE(SUM(output_tokens), 0)         AS output_tokens,
            COALESCE(SUM(cache_read_tokens), 0)     AS cache_read_tokens,
            COALESCE(SUM(cache_creation_tokens), 0) AS cache_creation_tokens,
            COALESCE(SUM(cost_usd), 0.0)            AS cost_usd,
            AVG(duration_ms)                        AS avg_duration_ms
         FROM agent_invocations
         WHERE (?1 IS NULL OR ts >= ?1){group_clause}
         ORDER BY cost_usd DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![since], |row| {
            Ok(UsageAggregate {
                bucket: row.get(0)?,
                invocations: row.get(1)?,
                errors: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                cache_creation_tokens: row.get(6)?,
                cost_usd: row.get(7)?,
                avg_duration_ms: row.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn sample(caller: Caller, model: &str, cost: f64) -> InvocationRecord {
        InvocationRecord {
            caller: caller.as_str(),
            model: model.into(),
            input_tokens: Some(10),
            output_tokens: Some(20),
            cache_read_tokens: Some(100),
            cost_usd: Some(cost),
            duration_ms: Some(1500),
            ..Default::default()
        }
    }

    #[test]
    fn insert_and_aggregate_total() {
        let conn = db::open_memory().unwrap();
        insert(&conn, &sample(Caller::Curator, "sonnet", 0.05)).unwrap();
        insert(&conn, &sample(Caller::SignalGate, "haiku", 0.01)).unwrap();

        let totals = aggregate(&conn, None, GroupBy::Total).unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].invocations, 2);
        assert!((totals[0].cost_usd - 0.06).abs() < 1e-9);
        assert_eq!(totals[0].input_tokens, 20);
        assert_eq!(totals[0].output_tokens, 40);
    }

    #[test]
    fn all_time_total_spans_multiple_parent_sessions() {
        let conn = db::open_memory().unwrap();

        let mut first = sample(Caller::Curator, "sonnet", 0.05);
        first.parent_session = Some("session-a".into());
        insert(&conn, &first).unwrap();

        let mut second = sample(Caller::SignalGate, "haiku", 0.01);
        second.parent_session = Some("session-b".into());
        insert(&conn, &second).unwrap();

        let totals = aggregate(&conn, None, GroupBy::Total).unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].invocations, 2);
        assert!((totals[0].cost_usd - 0.06).abs() < 1e-9);

        let sessions = aggregate(&conn, None, GroupBy::ParentSession).unwrap();
        let buckets: std::collections::HashSet<_> =
            sessions.into_iter().map(|row| row.bucket).collect();
        assert!(buckets.contains("session-a"));
        assert!(buckets.contains("session-b"));
    }

    #[test]
    fn aggregate_by_caller_sorts_by_cost() {
        let conn = db::open_memory().unwrap();
        insert(&conn, &sample(Caller::SignalGate, "haiku", 0.01)).unwrap();
        insert(&conn, &sample(Caller::Curator, "sonnet", 0.50)).unwrap();
        insert(&conn, &sample(Caller::Curator, "sonnet", 0.30)).unwrap();

        let by_caller = aggregate(&conn, None, GroupBy::Caller).unwrap();
        assert_eq!(by_caller.len(), 2);
        assert_eq!(by_caller[0].bucket, "curator");
        assert!((by_caller[0].cost_usd - 0.80).abs() < 1e-9);
        assert_eq!(by_caller[1].bucket, "signal_gate");
    }

    #[test]
    fn since_filter_excludes_old_rows() {
        let conn = db::open_memory().unwrap();
        // Directly insert with a stale ts
        conn.execute(
            "INSERT INTO agent_invocations (id, ts, caller, model, cost_usd, is_error)
             VALUES ('old', '1999-01-01T00:00:00Z', 'curator', 'sonnet', 0.10, 0)",
            [],
        )
        .unwrap();
        insert(&conn, &sample(Caller::Curator, "sonnet", 0.20)).unwrap();

        let recent = aggregate(&conn, Some("2020-01-01T00:00:00Z"), GroupBy::Total).unwrap();
        assert_eq!(recent[0].invocations, 1);
        assert!((recent[0].cost_usd - 0.20).abs() < 1e-9);
    }

    #[test]
    fn try_insert_does_not_panic_on_bad_schema() {
        // Open an in-memory DB then drop the table — try_insert should log and return.
        let conn = db::open_memory().unwrap();
        conn.execute("DROP TABLE agent_invocations", []).unwrap();
        try_insert(&conn, &sample(Caller::Curator, "sonnet", 0.1));
    }
}
