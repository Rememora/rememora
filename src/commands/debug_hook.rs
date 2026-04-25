//! `rememora debug record-hook-event` — record a single hook gate-outcome
//! event into `hook_invocations`.
//!
//! Called from `plugin/scripts/stop-curate.sh` at each gate-exit point.
//! The verb is intentionally private/experimental: the schema may churn
//! during the validation window for issue #82.
//!
//! Resilience contract: this command must never fail in a way that the
//! shell hook would surface to the user. We validate inputs against known
//! enums (so a typo produces a sensible CLI error during development) but
//! the actual insert uses `try_insert` — a missing/locked DB will not
//! propagate.

use anyhow::Result;
use rusqlite::Connection;

use rememora::models::hook_invocation::{self, HookEventRecord, HookKind, Outcome};

#[derive(Debug, Clone)]
pub struct RecordHookEventArgs {
    pub hook: String,
    pub outcome: String,
    pub session_id: Option<String>,
    pub parent_session: Option<String>,
    pub cooldown_state: Option<String>,
    pub extra: Option<String>,
}

pub fn record(conn: &Connection, args: &RecordHookEventArgs) -> Result<()> {
    let hook = HookKind::parse(&args.hook)?;
    let outcome = Outcome::parse(&args.outcome)?;

    let record = HookEventRecord {
        hook: hook.as_str(),
        outcome: outcome.as_str(),
        session_id: args.session_id.clone().filter(|s| !s.is_empty()),
        parent_session: args.parent_session.clone().filter(|s| !s.is_empty()),
        cooldown_state: args.cooldown_state.clone().filter(|s| !s.is_empty()),
        extra: args.extra.clone().filter(|s| !s.is_empty()),
    };

    hook_invocation::try_insert(conn, &record);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rememora::db;

    #[test]
    fn record_writes_a_row() {
        let conn = db::open_memory().unwrap();
        record(
            &conn,
            &RecordHookEventArgs {
                hook: "stop-curate".into(),
                outcome: "passed_through".into(),
                session_id: Some("abc".into()),
                parent_session: None,
                cooldown_state: Some("fresh".into()),
                extra: None,
            },
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM hook_invocations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn record_rejects_unknown_outcome() {
        let conn = db::open_memory().unwrap();
        let err = record(
            &conn,
            &RecordHookEventArgs {
                hook: "stop-curate".into(),
                outcome: "bogus".into(),
                session_id: None,
                parent_session: None,
                cooldown_state: None,
                extra: None,
            },
        )
        .unwrap_err();
        assert!(format!("{err}").contains("unknown outcome"));
    }

    #[test]
    fn record_rejects_unknown_hook() {
        let conn = db::open_memory().unwrap();
        let err = record(
            &conn,
            &RecordHookEventArgs {
                hook: "nope".into(),
                outcome: "passed_through".into(),
                session_id: None,
                parent_session: None,
                cooldown_state: None,
                extra: None,
            },
        )
        .unwrap_err();
        assert!(format!("{err}").contains("unknown hook"));
    }

    #[test]
    fn empty_strings_become_null() {
        let conn = db::open_memory().unwrap();
        record(
            &conn,
            &RecordHookEventArgs {
                hook: "stop-curate".into(),
                outcome: "env_var_short_circuit".into(),
                session_id: Some("".into()),
                parent_session: Some("".into()),
                cooldown_state: Some("".into()),
                extra: None,
            },
        )
        .unwrap();

        let nulls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM hook_invocations
                 WHERE session_id IS NULL
                   AND parent_session IS NULL
                   AND cooldown_state IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 1);
    }
}
