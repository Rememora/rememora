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
