//! Behavior tests: Search (BM25 + filters + active_count bumping).
//!
//! Migrated from test_search.rs to BDD-style Given-When-Then structure.

mod scenarios;

use scenarios::{db_with_memories, memory};

// ---------------------------------------------------------------------------
// BM25 matching across fields
// ---------------------------------------------------------------------------

#[test]
fn search_matches_memory_by_content() {
    // Given: a memory with "Zustand" in its content
    let conn = db_with_memories(&[
        memory("Use Zustand for state management")
            .project("testproj")
            .content("After evaluating Redux, MobX, and Zustand, we chose Zustand for state management."),
    ]);

    // When: searching for "Zustand"
    let results = rememora::search::search(&conn, "Zustand", None, None, 10).unwrap();

    // Then: it should find the memory
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.context.name.contains("Zustand")));
}

#[test]
fn search_matches_memory_by_abstract() {
    // Given: a memory with "dark mode" in its abstract
    let conn = db_with_memories(&[
        memory("Prefers dark mode")
            .abstract_text("User prefers dark mode in all editors")
            .content("Dark mode preference applies everywhere."),
    ]);

    // When: searching for "dark mode"
    let results = rememora::search::search(&conn, "dark mode", None, None, 10).unwrap();

    // Then: it should find the memory
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.context.name.contains("dark mode")));
}

#[test]
fn search_matches_memory_by_tags() {
    // Given: a memory tagged with "payments"
    let conn = db_with_memories(&[
        memory("Stripe API integration")
            .project("testproj")
            .category("entity")
            .tags(&["payments", "api"])
            .content("Stripe integration details"),
    ]);

    // When: searching for "payments"
    let results = rememora::search::search(&conn, "payments", None, None, 10).unwrap();

    // Then: it should find the memory via tag match
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.context.name.contains("Stripe")));
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

#[test]
fn project_filter_scopes_results_to_that_project() {
    // Given: memories in two different projects
    let conn = db_with_memories(&[
        memory("Alpha decision")
            .project("alpha")
            .content("Decision for project alpha"),
        memory("Beta decision")
            .project("beta")
            .content("Decision for project beta"),
    ]);

    // When: searching within "alpha" only
    let results = rememora::search::search(&conn, "decision", Some("alpha"), None, 10).unwrap();

    // Then: only alpha results should appear
    assert!(!results.is_empty());
    assert!(
        results.iter().all(|r| r.context.uri.contains("alpha") || r.context.uri.contains("global")),
        "All results should be from alpha or global scope"
    );
}

#[test]
fn category_filter_returns_only_matching_category() {
    // Given: memories in different categories matching the same query
    let conn = db_with_memories(&[
        memory("Zustand state decision")
            .project("testproj")
            .category("decision")
            .content("Chose Zustand for state"),
        memory("Zustand API entity")
            .project("testproj")
            .category("entity")
            .content("Zustand store API reference"),
    ]);

    // When: searching with category filter
    let results =
        rememora::search::search(&conn, "Zustand", None, Some("decision"), 10).unwrap();

    // Then: only decisions should appear
    assert!(!results.is_empty());
    assert!(results
        .iter()
        .all(|r| r.context.category.as_deref() == Some("decision")));
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn search_for_nonexistent_term_returns_empty() {
    // Given: a database with some memories
    let conn = db_with_memories(&[
        memory("Some memory")
            .project("testproj")
            .content("Normal content here"),
    ]);

    // When: searching for a term that doesn't exist anywhere
    let results = rememora::search::search(&conn, "nonexistentxyz123", None, None, 10).unwrap();

    // Then: no results, no error
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// Active count bumping
// ---------------------------------------------------------------------------

#[test]
fn search_bumps_active_count_for_returned_results() {
    // Given: a memory with zero initial accesses
    let conn = db_with_memories(&[
        memory("Bump target")
            .project("testproj")
            .content("This memory should get its access count bumped by search"),
    ]);

    // When: searching and finding it
    let results = rememora::search::search(&conn, "bumped", None, None, 10).unwrap();
    assert!(!results.is_empty());
    let id = results[0].context.id.clone();

    // Then: active_count should have been incremented
    let ctx = rememora::models::context::get_by_id(&conn, &id)
        .unwrap()
        .unwrap();
    assert!(
        ctx.active_count >= 1,
        "active_count should be >= 1 after search, got {}",
        ctx.active_count
    );
}
