mod common;

use rememora::models::context::{self, InsertContext};

fn make_test_insert(uri: &str, name: &str) -> InsertContext {
    InsertContext {
        uri: uri.to_string(),
        parent_uri: Some("rememora://projects/test/memories/decisions".to_string()),
        context_type: "memory".to_string(),
        category: Some("decision".to_string()),
        name: name.to_string(),
        abstract_text: format!("Abstract for {name}"),
        overview: format!("Overview for {name}"),
        content: format!("Full content for {name}"),
        tags: "[]".to_string(),
        source_agent: Some("claude-code".to_string()),
        source_session: None,
        importance: 0.5,
    }
}

#[test]
fn test_insert_and_get_by_id() {
    let conn = common::create_test_db();
    let input = make_test_insert("rememora://projects/test/memories/decisions/foo", "Foo decision");
    let id = context::insert(&conn, &input).unwrap();

    let ctx = context::get_by_id(&conn, &id).unwrap().unwrap();
    assert_eq!(ctx.uri, "rememora://projects/test/memories/decisions/foo");
    assert_eq!(ctx.name, "Foo decision");
    assert_eq!(ctx.context_type, "memory");
    assert_eq!(ctx.category.as_deref(), Some("decision"));
    assert_eq!(ctx.importance, 0.5);
    assert_eq!(ctx.active_count, 0);
    assert!(ctx.superseded_by.is_none());
}

#[test]
fn test_insert_and_get_by_uri() {
    let conn = common::create_test_db();
    let input = make_test_insert("rememora://projects/test/memories/decisions/bar", "Bar decision");
    context::insert(&conn, &input).unwrap();

    let ctx = context::get_by_uri(&conn, "rememora://projects/test/memories/decisions/bar")
        .unwrap()
        .unwrap();
    assert_eq!(ctx.name, "Bar decision");
}

#[test]
fn test_list_by_parent() {
    let conn = common::create_test_db();
    let parent = "rememora://projects/test/memories/decisions";

    let input1 = InsertContext {
        uri: "rememora://projects/test/memories/decisions/one".into(),
        parent_uri: Some(parent.into()),
        ..make_test_insert("", "One")
    };
    let input2 = InsertContext {
        uri: "rememora://projects/test/memories/decisions/two".into(),
        parent_uri: Some(parent.into()),
        ..make_test_insert("", "Two")
    };

    context::insert(&conn, &input1).unwrap();
    context::insert(&conn, &input2).unwrap();

    let children = context::list_by_parent(&conn, parent).unwrap();
    assert_eq!(children.len(), 2);
}

#[test]
fn test_update_context() {
    let conn = common::create_test_db();
    let input = make_test_insert("rememora://projects/test/memories/decisions/upd", "Update me");
    let id = context::insert(&conn, &input).unwrap();

    context::update(&conn, &id, Some("New abstract"), None, None, Some(0.9), None).unwrap();

    let ctx = context::get_by_id(&conn, &id).unwrap().unwrap();
    assert_eq!(ctx.abstract_text, "New abstract");
    assert_eq!(ctx.importance, 0.9);
    assert_eq!(ctx.overview, "Overview for Update me"); // unchanged
}

#[test]
fn test_insert_seeds_last_accessed_at() {
    let conn = common::create_test_db();
    let input = make_test_insert("rememora://projects/test/memories/decisions/seed", "Seed");
    let id = context::insert(&conn, &input).unwrap();

    let ctx = context::get_by_id(&conn, &id).unwrap().unwrap();
    let accessed = context::last_accessed_at(&conn, &id).unwrap();
    assert_eq!(accessed.as_deref(), Some(ctx.created_at.as_str()));
}

#[test]
fn test_bump_does_not_touch_updated_at() {
    let conn = common::create_test_db();
    let input = make_test_insert("rememora://projects/test/memories/decisions/bump", "Bump me");
    let id = context::insert(&conn, &input).unwrap();

    // Backdate both timestamps so any write is unambiguous.
    let long_ago = "2020-01-01T00:00:00+00:00";
    conn.execute(
        "UPDATE contexts SET updated_at = ?1, last_accessed_at = ?1 WHERE id = ?2",
        rusqlite::params![long_ago, id],
    )
    .unwrap();

    context::bump_active_count(&conn, &id).unwrap();

    let ctx = context::get_by_id(&conn, &id).unwrap().unwrap();
    // active_count rises, content-recency stays put.
    assert_eq!(ctx.active_count, 1);
    assert_eq!(
        ctx.updated_at, long_ago,
        "retrieval must not restamp updated_at — that is what destroyed the recency signal"
    );
    // Access recency moves instead.
    let accessed = context::last_accessed_at(&conn, &id).unwrap().unwrap();
    assert!(
        accessed.as_str() > long_ago,
        "last_accessed_at should advance on bump, got {accessed}"
    );
}

#[test]
fn test_update_still_touches_updated_at() {
    let conn = common::create_test_db();
    let input = make_test_insert("rememora://projects/test/memories/decisions/edit", "Edit me");
    let id = context::insert(&conn, &input).unwrap();

    let long_ago = "2020-01-01T00:00:00+00:00";
    conn.execute(
        "UPDATE contexts SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![long_ago, id],
    )
    .unwrap();

    context::update(&conn, &id, Some("Rewritten abstract"), None, None, None, None).unwrap();

    let ctx = context::get_by_id(&conn, &id).unwrap().unwrap();
    assert!(
        ctx.updated_at.as_str() > long_ago,
        "a real content change must advance updated_at, got {}",
        ctx.updated_at
    );
}

#[test]
fn test_migration_006_backfills_last_accessed_from_updated_at() {
    // Upgrade path: a DB written before migration 006, whose `updated_at` was
    // restamped on every read by the old `bump_active_count`. That polluted
    // value *is* the last-access time, so the backfill recovers the access
    // signal — and must leave the (unrecoverable) `updated_at` alone.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("legacy.db");
    let polluted = "2026-01-01T00:00:00+00:00";

    let id = {
        let conn = rememora::db::open_with_options(&path, true).unwrap();
        let id = context::insert(
            &conn,
            &make_test_insert("rememora://projects/test/memories/decisions/legacy", "Legacy"),
        )
        .unwrap();

        // Wind the schema back to its pre-006 shape.
        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_ctx_last_accessed;
             ALTER TABLE contexts DROP COLUMN last_accessed_at;
             DELETE FROM _migrations WHERE name = '006_access_recency';",
        )
        .unwrap();
        conn.execute(
            "UPDATE contexts SET updated_at = ?1, active_count = 12 WHERE id = ?2",
            rusqlite::params![polluted, id],
        )
        .unwrap();
        id
    };

    // Reopening runs migration 006.
    let conn = rememora::db::open_with_options(&path, true).unwrap();

    let ctx = context::get_by_id(&conn, &id).unwrap().unwrap();
    assert_eq!(
        ctx.updated_at, polluted,
        "the migration must not rewrite updated_at — the true value is unrecoverable"
    );
    assert_eq!(ctx.active_count, 12);
    assert_eq!(
        context::last_accessed_at(&conn, &id).unwrap().as_deref(),
        Some(polluted),
        "last_accessed_at should be backfilled from the polluted updated_at"
    );
}

#[test]
fn test_supersede() {
    let conn = common::create_test_db();
    let old_input = make_test_insert("rememora://projects/test/memories/decisions/old", "Old");
    let new_input = make_test_insert("rememora://projects/test/memories/decisions/new", "New");

    let old_id = context::insert(&conn, &old_input).unwrap();
    let new_id = context::insert(&conn, &new_input).unwrap();

    context::supersede(&conn, &old_id, &new_id).unwrap();

    let old = context::get_by_id(&conn, &old_id).unwrap().unwrap();
    assert_eq!(old.superseded_by.as_deref(), Some(new_id.as_str()));
}

#[test]
fn test_fts_finds_by_content() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    // Search for "Zustand" which appears in the content
    let results: Vec<(String,)> = conn
        .prepare("SELECT c.name FROM contexts_fts fts JOIN contexts c ON c.rowid = fts.rowid WHERE contexts_fts MATCH 'zustand'")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?,)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(!results.is_empty());
    assert!(results.iter().any(|(name,)| name.contains("Zustand")));
}

#[test]
fn test_fts_finds_by_tag() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    // Search for "payments" which appears in tags
    let results: Vec<(String,)> = conn
        .prepare("SELECT c.name FROM contexts_fts fts JOIN contexts c ON c.rowid = fts.rowid WHERE contexts_fts MATCH 'payments'")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?,)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(!results.is_empty());
    assert!(results.iter().any(|(name,)| name.contains("Stripe")));
}

#[test]
fn test_duplicate_uri_fails() {
    let conn = common::create_test_db();
    let input = make_test_insert("rememora://projects/test/memories/decisions/dup", "Dup");
    context::insert(&conn, &input).unwrap();
    assert!(context::insert(&conn, &input).is_err());
}
