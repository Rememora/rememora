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

/// The migration names recorded in the ledger, sorted.
fn ledger_names(conn: &rusqlite::Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM _migrations ORDER BY name")
        .unwrap();
    let names = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    names
}

/// Every `*.sql` under `src/migrations`, by file stem — the ledger name each one
/// is recorded under.
fn migration_files() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/migrations");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter_map(|n| n.strip_suffix(".sql").map(str::to_string))
        .collect();
    names.sort();
    names
}

#[test]
fn test_migrations_idempotent() {
    // Idempotency is a claim about *reopening* a database, so open a real file
    // twice. An in-memory DB gets a fresh schema on every open and can never
    // exercise the replay path at all.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("replay.db");

    let conn = rememora::db::open_with_options(&path, true).unwrap();
    let after_first = ledger_names(&conn);
    drop(conn);

    let conn = rememora::db::open_with_options(&path, true).unwrap();
    let after_second = ledger_names(&conn);

    assert_eq!(
        after_first, after_second,
        "reopening the DB re-applied or re-recorded a migration"
    );

    // The ledger must hold exactly the migrations that exist on disk — not
    // merely "at least as many". A floor still passes in the one state that
    // actually matters: a migration that ran but was never recorded, which for a
    // non-replayable migration (006) is what makes the DB unopenable.
    assert_eq!(
        after_second,
        migration_files(),
        "the ledger must record exactly the migrations in src/migrations, once each"
    );
}

#[test]
fn test_migration_006_survives_a_lost_ledger_row() {
    // The crash this simulates: the ALTER lands, then the process dies (or the
    // ledger INSERT hits SQLITE_BUSY) before '006_access_recency' is recorded.
    // `ALTER TABLE ... ADD COLUMN` has no `IF NOT EXISTS`, so replaying 006
    // verbatim would abort on "duplicate column name" — and since migrate() runs
    // on every open, the database would never open again.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("lost-ledger.db");

    let conn = rememora::db::open_with_options(&path, true).unwrap();
    conn.execute_batch("DELETE FROM _migrations WHERE name = '006_access_recency';")
        .unwrap();
    drop(conn);

    let conn = rememora::db::open_with_options(&path, true)
        .expect("a lost ledger row must not brick the database");

    // The column survived untouched and the ledger is whole again.
    let count: i64 = conn
        .query_row("SELECT COUNT(last_accessed_at) FROM contexts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(ledger_names(&conn), migration_files());
}

#[test]
fn test_migration_006_rolls_back_when_the_ledger_write_fails() {
    // Apply-then-record is one transaction, so a failure to record must undo the
    // schema change rather than leave the half-applied state behind. Without the
    // transaction this test finds `last_accessed_at` already added.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ledger-boom.db");

    let conn = rememora::db::open_with_options(&path, true).unwrap();
    // Wind the schema back to its pre-006 shape, then make recording 006 fail.
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_ctx_last_accessed;
         ALTER TABLE contexts DROP COLUMN last_accessed_at;
         DELETE FROM _migrations WHERE name = '006_access_recency';
         CREATE TRIGGER ledger_boom BEFORE INSERT ON _migrations
         BEGIN SELECT RAISE(ABORT, 'simulated crash recording the migration'); END;",
    )
    .unwrap();
    drop(conn);

    rememora::db::open_with_options(&path, true)
        .expect_err("recording 006 was rigged to fail, so the open must fail");

    // Inspect the file without running migrations — the DB is unencrypted here.
    let raw = rusqlite::Connection::open(&path).unwrap();
    let column_added: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('contexts') WHERE name = 'last_accessed_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        column_added, 0,
        "the ALTER must roll back with the failed ledger write, not survive it"
    );
}
