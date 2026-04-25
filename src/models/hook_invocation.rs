//! Per-invocation telemetry for plugin hooks.
//!
//! Used to validate that the existing curate-recursion gates (env-var +
//! pgrep + cooldown) are sufficient in production. Each call to
//! `plugin/scripts/stop-curate.sh` emits one row at the gate-exit point,
//! recording which gate (if any) short-circuited the curate spawn.
//!
//! Writers land here from a single site:
//! - `commands::debug_hook::record` (invoked by `stop-curate.sh`)
//!
//! Readers are `rememora usage --hooks` (aggregation).
//!
//! Schema is experimental — the `extra` column is a JSON blob so we can add
//! forward-compat fields without a migration churn during validation.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    StopCurate,
}

impl HookKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HookKind::StopCurate => "stop-curate",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "stop-curate" => Ok(HookKind::StopCurate),
            other => Err(anyhow!("unknown hook kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The `REMEMORA_CURATE_CHILD=1` env var was set — we are running inside
    /// a curate-spawned subagent and must not re-curate.
    EnvVarShortCircuit,
    /// The `pgrep` gate matched a concurrent curate for this session.
    PgrepShortCircuit,
    /// The cooldown stamp file is within `REMEMORA_CURATE_COOLDOWN_SECS`.
    CooldownShortCircuit,
    /// No gate fired; the curate spawn proceeded.
    PassedThrough,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::EnvVarShortCircuit => "env_var_short_circuit",
            Outcome::PgrepShortCircuit => "pgrep_short_circuit",
            Outcome::CooldownShortCircuit => "cooldown_short_circuit",
            Outcome::PassedThrough => "passed_through",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "env_var_short_circuit" => Ok(Outcome::EnvVarShortCircuit),
            "pgrep_short_circuit" => Ok(Outcome::PgrepShortCircuit),
            "cooldown_short_circuit" => Ok(Outcome::CooldownShortCircuit),
            "passed_through" => Ok(Outcome::PassedThrough),
            other => Err(anyhow!("unknown outcome: {other}")),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HookEventRecord {
    pub hook: &'static str,
    pub outcome: &'static str,
    pub session_id: Option<String>,
    pub parent_session: Option<String>,
    pub cooldown_state: Option<String>,
    pub extra: Option<String>,
}

pub fn insert(conn: &Connection, record: &HookEventRecord) -> Result<String> {
    let id = ulid::Ulid::new().to_string();
    let ts = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO hook_invocations (
            id, ts, hook, outcome, session_id, parent_session, cooldown_state, extra
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            ts,
            record.hook,
            record.outcome,
            record.session_id,
            record.parent_session,
            record.cooldown_state,
            record.extra,
        ],
    )?;

    Ok(id)
}

/// Best-effort insert that never raises.
///
/// Telemetry must not break the caller. If the DB is unreachable or the
/// schema drifted, swallow the error and return without propagating.
pub fn try_insert(conn: &Connection, record: &HookEventRecord) {
    if let Err(e) = insert(conn, record) {
        eprintln!("warn: hook_invocation insert failed: {e}");
    }
}

// ── Aggregation ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookAggregate {
    pub hook: String,
    pub outcome: String,
    pub count: i64,
}

/// Aggregate hook invocations since an ISO8601 cutoff (inclusive).
///
/// Returns one row per `(hook, outcome)` pair, ordered by count desc.
/// `since` is `NULL`-safe: pass `None` to aggregate all history.
/// `hook_filter` restricts to a single hook kind.
pub fn aggregate_by_outcome(
    conn: &Connection,
    since: Option<&str>,
    hook_filter: Option<&str>,
) -> Result<Vec<HookAggregate>> {
    let sql = "SELECT hook, outcome, COUNT(*) AS n
               FROM hook_invocations
               WHERE (?1 IS NULL OR ts >= ?1)
                 AND (?2 IS NULL OR hook = ?2)
               GROUP BY hook, outcome
               ORDER BY n DESC, hook ASC, outcome ASC";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![since, hook_filter], |row| {
            Ok(HookAggregate {
                hook: row.get(0)?,
                outcome: row.get(1)?,
                count: row.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn rec(outcome: Outcome, session: Option<&str>) -> HookEventRecord {
        HookEventRecord {
            hook: HookKind::StopCurate.as_str(),
            outcome: outcome.as_str(),
            session_id: session.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn insert_returns_ulid() {
        let conn = db::open_memory().unwrap();
        let id = insert(&conn, &rec(Outcome::PassedThrough, Some("s1"))).unwrap();
        // ULID is 26 chars
        assert_eq!(id.len(), 26);
    }

    #[test]
    fn aggregate_groups_by_outcome() {
        let conn = db::open_memory().unwrap();
        insert(&conn, &rec(Outcome::PassedThrough, Some("s1"))).unwrap();
        insert(&conn, &rec(Outcome::PassedThrough, Some("s2"))).unwrap();
        insert(&conn, &rec(Outcome::PgrepShortCircuit, Some("s1"))).unwrap();
        insert(&conn, &rec(Outcome::EnvVarShortCircuit, None)).unwrap();

        let agg = aggregate_by_outcome(&conn, None, None).unwrap();
        // 3 distinct outcomes
        assert_eq!(agg.len(), 3);
        // First (highest count) should be passed_through with 2
        assert_eq!(agg[0].outcome, "passed_through");
        assert_eq!(agg[0].count, 2);
    }

    #[test]
    fn aggregate_filters_by_hook() {
        let conn = db::open_memory().unwrap();
        insert(&conn, &rec(Outcome::PassedThrough, Some("s1"))).unwrap();

        let agg = aggregate_by_outcome(&conn, None, Some("stop-curate")).unwrap();
        assert_eq!(agg.len(), 1);

        let agg_none = aggregate_by_outcome(&conn, None, Some("nonexistent")).unwrap();
        assert!(agg_none.is_empty());
    }

    #[test]
    fn since_filter_excludes_old_rows() {
        let conn = db::open_memory().unwrap();
        // Direct insert with stale ts
        conn.execute(
            "INSERT INTO hook_invocations (id, ts, hook, outcome) VALUES
             ('old', '1999-01-01T00:00:00Z', 'stop-curate', 'passed_through')",
            [],
        )
        .unwrap();
        insert(&conn, &rec(Outcome::PassedThrough, Some("s1"))).unwrap();

        let recent = aggregate_by_outcome(&conn, Some("2020-01-01T00:00:00Z"), None).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].count, 1);
    }

    #[test]
    fn try_insert_does_not_panic_on_bad_schema() {
        let conn = db::open_memory().unwrap();
        conn.execute("DROP TABLE hook_invocations", []).unwrap();
        try_insert(&conn, &rec(Outcome::PassedThrough, Some("s1")));
    }

    #[test]
    fn outcome_enum_roundtrip() {
        for o in [
            Outcome::EnvVarShortCircuit,
            Outcome::PgrepShortCircuit,
            Outcome::CooldownShortCircuit,
            Outcome::PassedThrough,
        ] {
            assert_eq!(Outcome::parse(o.as_str()).unwrap(), o);
        }
        assert!(Outcome::parse("nope").is_err());
    }

    #[test]
    fn hook_enum_roundtrip() {
        assert_eq!(
            HookKind::parse(HookKind::StopCurate.as_str()).unwrap(),
            HookKind::StopCurate
        );
        assert!(HookKind::parse("nope").is_err());
    }
}
