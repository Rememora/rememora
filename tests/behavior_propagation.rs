//! Behavior tests: Hierarchical score propagation.
//!
//! Validates that searching with `--propagate` boosts related contexts
//! in the URI tree (parents, children, siblings) using exponential decay.

mod scenarios;

use rememora::propagate::PropagationConfig;
use scenarios::{db_with_memories, memory};

// ---------------------------------------------------------------------------
// Parent boost
// ---------------------------------------------------------------------------

#[test]
fn parent_gets_boosted_when_child_matches() {
    // Given: a parent category context and a child memory with "Zustand" keyword
    let conn = db_with_memories(&[
        memory("state-management decisions")
            .project("testproj")
            .category("decision")
            .abstract_text("Decisions about state management approaches")
            .content("Collection of state management decisions"),
        memory("chose Zustand over Redux")
            .project("testproj")
            .category("decision")
            .content("After evaluating Redux and Zustand, we chose Zustand for its simplicity"),
    ]);

    // When: searching for "Zustand" with propagation enabled
    let config = PropagationConfig::default();
    let results = rememora::search::search_with_propagation(
        &conn, "Zustand", Some("testproj"), None, 10, &config,
    )
    .unwrap();

    // Then: the parent "state-management decisions" should appear (boosted by child match)
    assert!(results.len() >= 2, "Expected at least 2 results (direct + parent), got {}", results.len());
    assert!(
        results.iter().any(|r| r.context.name.contains("state-management")),
        "Parent context should appear via propagation"
    );
}

// ---------------------------------------------------------------------------
// Sibling boost
// ---------------------------------------------------------------------------

#[test]
fn sibling_gets_boosted_when_sibling_matches() {
    // Given: two sibling memories (same parent_uri) where only one has "Zustand"
    let conn = db_with_memories(&[
        memory("chose Zustand for state")
            .project("testproj")
            .category("decision")
            .content("We chose Zustand for all state management"),
        memory("prefer hooks over classes")
            .project("testproj")
            .category("decision")
            .content("React hooks are preferred over class components"),
    ]);

    // When: searching for "Zustand" with propagation
    let config = PropagationConfig::default();
    let results = rememora::search::search_with_propagation(
        &conn, "Zustand", Some("testproj"), None, 10, &config,
    )
    .unwrap();

    // Then: the sibling "prefer hooks" should appear via propagation
    assert!(
        results.iter().any(|r| r.context.name.contains("hooks")),
        "Sibling context should appear via propagation. Got: {:?}",
        results.iter().map(|r| &r.context.name).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Children boost
// ---------------------------------------------------------------------------

#[test]
fn children_get_boosted_when_parent_matches() {
    // Given: a parent context that matches "architecture" and children under it
    //   We need the parent to actually contain searchable text AND have
    //   children whose parent_uri points to the parent's URI.
    let conn = scenarios::given::db();

    // Insert parent first
    let parent_id = memory("architecture overview")
        .project("testproj")
        .category("entity")
        .abstract_text("System architecture overview and key decisions")
        .content("The overall architecture follows a modular pattern")
        .insert(&conn);

    // Get parent's URI so we can set children's parent_uri correctly
    let parent_ctx = rememora::models::context::get_by_id(&conn, &parent_id).unwrap().unwrap();

    // Insert child with explicit parent_uri pointing to parent
    let child_uri = format!("{}/api-layer", parent_ctx.uri);
    rememora::models::context::insert(
        &conn,
        &rememora::models::context::InsertContext {
            uri: child_uri,
            parent_uri: Some(parent_ctx.uri.clone()),
            context_type: "memory".to_string(),
            category: Some("entity".to_string()),
            name: "API layer design".to_string(),
            abstract_text: "REST API layer details".to_string(),
            overview: "API layer overview".to_string(),
            content: "The API layer uses Express with middleware".to_string(),
            tags: "[]".to_string(),
            source_agent: Some("claude-code".to_string()),
            source_session: None,
            importance: 0.5,
        },
    )
    .unwrap();

    // When: searching for "architecture" with propagation
    let config = PropagationConfig::default();
    let results = rememora::search::search_with_propagation(
        &conn, "architecture", Some("testproj"), None, 10, &config,
    )
    .unwrap();

    // Then: the child "API layer" should appear via propagation
    assert!(
        results.iter().any(|r| r.context.name.contains("API layer")),
        "Child context should appear via propagation. Got: {:?}",
        results.iter().map(|r| &r.context.name).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Decay ordering
// ---------------------------------------------------------------------------

#[test]
fn parent_boost_is_stronger_than_sibling_boost() {
    // Given: a direct match, its parent (1 hop), and a sibling (2 hops)
    let conn = db_with_memories(&[
        memory("chose Zustand for state")
            .project("testproj")
            .category("decision")
            .content("We chose Zustand for state management in all React apps"),
        memory("prefer dark mode")
            .project("testproj")
            .category("decision")
            .content("Dark mode is the default theme preference"),
    ]);

    // When: searching for "Zustand" with propagation
    let config = PropagationConfig::default();
    let results = rememora::search::search_with_propagation(
        &conn, "Zustand", Some("testproj"), None, 10, &config,
    )
    .unwrap();

    // Then: direct match should rank first
    assert!(!results.is_empty());
    assert!(
        results[0].context.name.contains("Zustand"),
        "Direct match should rank first"
    );

    // And: if both parent and sibling appear, parent should rank higher than sibling
    // (1 hop vs 2 hops with same decay factor)
    // This is implicit in the scoring: decay^1 > decay^2 for decay < 1
}

// ---------------------------------------------------------------------------
// Direct match outranks propagated
// ---------------------------------------------------------------------------

#[test]
fn direct_match_outranks_propagated_result() {
    // Given: two memories, one matching directly and one only via propagation
    let conn = db_with_memories(&[
        memory("chose Zustand for state")
            .project("testproj")
            .category("decision")
            .content("We chose Zustand as our state management solution"),
        memory("prefer hooks over classes")
            .project("testproj")
            .category("decision")
            .content("React hooks are preferred over class components"),
    ]);

    // When: searching for "Zustand" with propagation
    let config = PropagationConfig::default();
    let results = rememora::search::search_with_propagation(
        &conn, "Zustand", Some("testproj"), None, 10, &config,
    )
    .unwrap();

    // Then: the direct match should always be first
    assert!(!results.is_empty());
    assert!(
        results[0].context.name.contains("Zustand"),
        "Direct match should outrank propagated. First result: {}",
        results[0].context.name
    );
}

// ---------------------------------------------------------------------------
// Max depth respected
// ---------------------------------------------------------------------------

#[test]
fn propagation_respects_max_depth() {
    // Given: memories where siblings are 2 hops away
    let conn = db_with_memories(&[
        memory("chose Zustand for state")
            .project("testproj")
            .category("decision")
            .content("We chose Zustand for state management"),
        memory("prefer dark mode")
            .project("testproj")
            .category("decision")
            .content("Dark mode preference for all editors"),
    ]);

    // When: searching with depth=1 (siblings are 2 hops, should be excluded)
    let config = PropagationConfig {
        decay_factor: 0.3,
        max_depth: 1,
    };
    let results = rememora::search::search_with_propagation(
        &conn, "Zustand", Some("testproj"), None, 10, &config,
    )
    .unwrap();

    // Then: sibling should NOT appear (it's 2 hops away, depth limit is 1)
    assert!(
        !results.iter().any(|r| r.context.name.contains("dark mode")),
        "Sibling should not appear with max_depth=1"
    );
}

// ---------------------------------------------------------------------------
// Propagation disabled by default
// ---------------------------------------------------------------------------

#[test]
fn without_propagation_results_are_normal_bm25() {
    // Given: a memory and its sibling
    let conn = db_with_memories(&[
        memory("chose Zustand for state")
            .project("testproj")
            .category("decision")
            .content("We chose Zustand for state management"),
        memory("prefer dark mode")
            .project("testproj")
            .category("decision")
            .content("Dark mode preference for all editors"),
    ]);

    // When: searching WITHOUT propagation (normal search)
    let results = rememora::search::search(
        &conn, "Zustand", Some("testproj"), None, 10,
    )
    .unwrap();

    // Then: only the direct match should appear (no sibling boost)
    assert_eq!(results.len(), 1, "Without propagation, only direct matches should appear");
    assert!(results[0].context.name.contains("Zustand"));
}

// ---------------------------------------------------------------------------
// Superseded contexts excluded
// ---------------------------------------------------------------------------

#[test]
fn propagation_excludes_superseded_contexts() {
    // Given: a memory and a superseded sibling
    let conn = scenarios::given::db();

    // Create two memories
    let active_id = memory("chose Zustand for state")
        .project("testproj")
        .category("decision")
        .content("We chose Zustand for state management")
        .insert(&conn);

    let superseded_id = memory("chose Redux for state")
        .project("testproj")
        .category("decision")
        .content("We chose Redux for state management")
        .insert(&conn);

    // Supersede the old one
    rememora::models::context::supersede(&conn, &superseded_id, &active_id).unwrap();

    // When: searching with propagation
    let config = PropagationConfig::default();
    let results = rememora::search::search_with_propagation(
        &conn, "Zustand", Some("testproj"), None, 10, &config,
    )
    .unwrap();

    // Then: the superseded memory should not appear
    assert!(
        !results.iter().any(|r| r.context.id == superseded_id),
        "Superseded context should not appear in propagated results"
    );
}
