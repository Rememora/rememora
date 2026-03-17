mod common;

use rememora::search;

#[test]
fn test_search_by_content_keyword() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results = search::search(&conn, "Zustand", None, None, 10).unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.context.name.contains("Zustand")));
}

#[test]
fn test_search_by_abstract_keyword() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results = search::search(&conn, "dark mode", None, None, 10).unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.context.name.contains("dark mode")));
}

#[test]
fn test_search_with_project_filter() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    // Search within testproj only — should find Zustand (search adds OR between words)
    let results = search::search(&conn, "Zustand", Some("testproj"), None, 10).unwrap();
    let has_zustand = results.iter().any(|r| r.context.name.contains("Zustand"));
    assert!(has_zustand);
}

#[test]
fn test_search_with_category_filter() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results = search::search(&conn, "Zustand Stripe", None, Some("decision"), 10).unwrap();
    // Should only find the decision (Zustand), not the entity (Stripe)
    assert!(!results.is_empty());
    assert!(results.iter().all(|r| r.context.category.as_deref() == Some("decision")));
}

#[test]
fn test_search_no_results() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results = search::search(&conn, "nonexistentxyz123", None, None, 10).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_search_bumps_active_count() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    // Get initial active_count
    let results = search::search(&conn, "Zustand", None, None, 10).unwrap();
    let id = results[0].context.id.clone();

    let ctx = rememora::models::context::get_by_id(&conn, &id).unwrap().unwrap();
    assert!(ctx.active_count >= 1); // bumped by search
}
