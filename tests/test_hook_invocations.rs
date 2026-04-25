//! Integration tests for the `hook_invocations` table and its public API.
//!
//! Mirrors the unit tests but exercises the full migration path against a
//! freshly-opened in-memory DB.

mod common;

use rememora::models::hook_invocation::{
    self, aggregate_by_outcome, HookEventRecord, HookKind, Outcome,
};

#[test]
fn migration_creates_hook_invocations_table() {
    let conn = common::create_test_db();
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='hook_invocations')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(exists, "hook_invocations table should exist after migrate");
}

#[test]
fn insert_and_aggregate_by_outcome() {
    let conn = common::create_test_db();

    for o in [
        Outcome::PassedThrough,
        Outcome::PassedThrough,
        Outcome::PgrepShortCircuit,
        Outcome::EnvVarShortCircuit,
        Outcome::CooldownShortCircuit,
    ] {
        hook_invocation::insert(
            &conn,
            &HookEventRecord {
                hook: HookKind::StopCurate.as_str(),
                outcome: o.as_str(),
                session_id: Some("sess-x".into()),
                cooldown_state: Some("fresh".into()),
                ..Default::default()
            },
        )
        .unwrap();
    }

    let agg = aggregate_by_outcome(&conn, None, Some("stop-curate")).unwrap();
    assert_eq!(agg.len(), 4); // 4 distinct outcomes
    assert_eq!(agg[0].outcome, "passed_through"); // highest count
    assert_eq!(agg[0].count, 2);

    // Total invocations across outcomes
    let total: i64 = agg.iter().map(|r| r.count).sum();
    assert_eq!(total, 5);
}

#[test]
fn since_filter_excludes_old_rows() {
    let conn = common::create_test_db();

    // Stale row inserted directly with explicit ts
    conn.execute(
        "INSERT INTO hook_invocations (id, ts, hook, outcome) VALUES
         ('old1', '1990-01-01T00:00:00Z', 'stop-curate', 'passed_through')",
        [],
    )
    .unwrap();

    hook_invocation::insert(
        &conn,
        &HookEventRecord {
            hook: HookKind::StopCurate.as_str(),
            outcome: Outcome::PassedThrough.as_str(),
            ..Default::default()
        },
    )
    .unwrap();

    let recent = aggregate_by_outcome(&conn, Some("2020-01-01T00:00:00Z"), None).unwrap();
    let total: i64 = recent.iter().map(|r| r.count).sum();
    assert_eq!(total, 1);

    let all = aggregate_by_outcome(&conn, None, None).unwrap();
    let total_all: i64 = all.iter().map(|r| r.count).sum();
    assert_eq!(total_all, 2);
}
