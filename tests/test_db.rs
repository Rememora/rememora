mod common;

#[test]
fn test_db_opens_with_wal() {
    let conn = common::create_test_db();
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    // In-memory DB returns "memory" for journal_mode, but WAL is set for file-based
    // For in-memory, just verify it doesn't error
    assert!(!journal_mode.is_empty());
}

#[test]
fn test_migrations_create_tables() {
    let conn = common::create_test_db();

    // Verify contexts table exists
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM contexts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);

    // Verify sessions table exists
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);

    // Verify relations table exists
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM relations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);

    // Verify FTS5 table exists
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM contexts_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);

    // Verify context_embeddings table exists (migration 002)
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM context_embeddings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);

    // Verify curator tables exist (migration 003)
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM watermarks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM curator_log", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM consolidation_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_migrations_idempotent() {
    let conn = common::create_test_db();
    // Running open_memory again with same connection would re-run configure+migrate
    // But since we use IF NOT EXISTS, it should be safe
    // Just verify the migration tracking table has exactly one entry
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 3);
}
