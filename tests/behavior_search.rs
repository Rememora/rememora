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
// Output formats: compact + context
// ---------------------------------------------------------------------------

#[test]
fn compact_format_is_one_line_per_hit_with_category_uri_and_rank() {
    // Given: two memories that should match a query
    let conn = db_with_memories(&[
        memory("Redis caching")
            .project("testproj")
            .category("decision")
            .content("Picked Redis over Memcached for caching"),
        memory("Redis connection pool")
            .project("testproj")
            .category("entity")
            .content("Shared Redis pool lives in src/cache"),
    ]);

    // When: searching and rendering the compact format
    let results = rememora::search::search(&conn, "redis", None, None, 10).unwrap();
    assert!(!results.is_empty(), "expected search hits");

    let rendered = rememora::format::search_results_to_compact(&results);
    let lines: Vec<&str> = rendered.lines().collect();

    // Then: one line per hit, each line carries category tag + URI + rank token
    assert_eq!(lines.len(), results.len());
    for (line, result) in lines.iter().zip(results.iter()) {
        let cat = result.context.category.as_deref().unwrap_or("");
        assert!(
            line.starts_with(&format!("[{}]", cat)),
            "line should start with category tag, got: {line}"
        );
        assert!(line.contains(&result.context.uri), "line should include URI: {line}");
        assert!(line.contains("rank="), "line should include rank token: {line}");
    }
}

#[test]
fn compact_format_trims_overly_long_abstracts() {
    // Given: a memory with an intentionally very long abstract
    let huge = "x".repeat(2000);
    let conn = db_with_memories(&[
        memory("bloated abstract")
            .project("testproj")
            .abstract_text(&huge)
            .content("anything"),
    ]);

    // When: searching and rendering compact
    let results = rememora::search::search(&conn, "bloated", None, None, 10).unwrap();
    let rendered = rememora::format::search_results_to_compact(&results);

    // Then: no line keeps the full 2000-char abstract
    for line in rendered.lines() {
        assert!(
            line.chars().count() < 400,
            "compact line should be trimmed, got {} chars",
            line.chars().count()
        );
    }
}

#[test]
fn context_format_is_length_capped_for_prompt_injection() {
    // Given: many memories so that an uncapped render would blow the budget
    let builders: Vec<_> = (0..50)
        .map(|i| {
            memory(&format!("match-{i}"))
                .project("testproj")
                .content("prompt-injection budget test")
        })
        .collect();
    let conn = db_with_memories(&builders);

    // When: searching and rendering context-mode
    let results =
        rememora::search::search(&conn, "prompt injection budget", None, None, 50).unwrap();
    assert!(!results.is_empty());
    let rendered = rememora::format::search_results_to_context(&results);

    // Then: overall byte count stays well under the inline-prompt cap
    assert!(
        rendered.len() <= 1200,
        "context format should be capped under 1200 bytes, got {}",
        rendered.len()
    );
    // And every line carries a bracketed category prefix
    for line in rendered.lines() {
        assert!(line.starts_with('['), "context line should start with [cat], got: {line}");
    }
}

#[test]
fn context_format_empty_when_no_results() {
    // Given: an empty DB
    let conn = db_with_memories(&[]);

    // When: searching for something that won't match, then rendering context-mode
    let results = rememora::search::search(&conn, "anything", None, None, 10).unwrap();
    let rendered = rememora::format::search_results_to_context(&results);

    // Then: empty string (no spurious header)
    assert!(rendered.is_empty());
}

// ---------------------------------------------------------------------------
// FTS5 query syntax (Issue #103)
// ---------------------------------------------------------------------------

#[test]
fn search_supports_explicit_or_operator() {
    // Given: two memories that share no common terms
    let conn = db_with_memories(&[
        memory("Redis caching")
            .project("testproj")
            .content("Picked Redis over memcached for caching"),
        memory("Stampede prevention")
            .project("testproj")
            .content("Thundering herd workaround using locks"),
    ]);

    // When: using FTS5's explicit OR operator
    let results = rememora::search::search(&conn, "redis OR stampede", None, None, 10).unwrap();

    // Then: both memories appear (no FTS5 syntax error)
    assert_eq!(results.len(), 2, "expected both branches of OR to match");
}

#[test]
fn search_supports_grouped_or_and() {
    // Given: memories with overlapping terms
    let conn = db_with_memories(&[
        memory("Redis caching layer")
            .project("testproj")
            .content("redis caching"),
        memory("Memcached cache layer")
            .project("testproj")
            .content("memcached caching"),
        memory("Unrelated note")
            .project("testproj")
            .content("nothing relevant"),
    ]);

    // When: using a grouped expression
    let results =
        rememora::search::search(&conn, "(redis OR memcached) AND caching", None, None, 10)
            .unwrap();

    // Then: both cache memories match, the unrelated one does not
    assert!(results.iter().any(|r| r.context.name.contains("Redis")));
    assert!(results.iter().any(|r| r.context.name.contains("Memcached")));
    assert!(!results.iter().any(|r| r.context.name.contains("Unrelated")));
}

#[test]
fn search_supports_phrase_query() {
    // Given: a memory containing the exact phrase
    let conn = db_with_memories(&[
        memory("Locks and herds")
            .project("testproj")
            .content("Use a thundering herd lock"),
        memory("Order matters")
            .project("testproj")
            // Same words, different order — must NOT match the phrase query.
            .content("a herd thundering through"),
    ]);

    // When: using FTS5 phrase syntax
    let results =
        rememora::search::search(&conn, "\"thundering herd\"", None, None, 10).unwrap();

    // Then: only the exact-phrase memory matches
    assert!(results.iter().any(|r| r.context.name.contains("Locks")));
    assert!(!results.iter().any(|r| r.context.name.contains("Order")));
}

#[test]
fn search_supports_prefix_query() {
    // Given: a memory containing "redis"
    let conn = db_with_memories(&[memory("Redis pool")
        .project("testproj")
        .content("redis connection pool")]);

    // When: using a prefix query
    let results = rememora::search::search(&conn, "redi*", None, None, 10).unwrap();

    // Then: it matches via the prefix
    assert!(!results.is_empty(), "redi* should match redis");
}

#[test]
fn search_empty_query_returns_empty_no_error() {
    // Given: a populated DB
    let conn = db_with_memories(&[memory("anything").project("testproj").content("x")]);

    // When: passing empty / whitespace-only queries
    let empty = rememora::search::search(&conn, "", None, None, 10).unwrap();
    let blank = rememora::search::search(&conn, "   \t  ", None, None, 10).unwrap();

    // Then: empty results, never an error
    assert!(empty.is_empty());
    assert!(blank.is_empty());
}

#[test]
fn search_with_unbalanced_quote_falls_back_safely() {
    // Given: a memory matching "redis"
    let conn = db_with_memories(&[memory("Redis cache")
        .project("testproj")
        .content("redis cache")]);

    // When: passing a query with a stray double-quote that would make FTS5
    // unhappy (the primary query path passes quotes through)
    let results = rememora::search::search(&conn, "redis \"", None, None, 10).unwrap();

    // Then: the safe-fallback OR-of-tokens path runs and finds the memory
    assert!(
        !results.is_empty(),
        "fallback should still surface the redis memory"
    );
}

#[test]
fn search_strips_control_characters() {
    // Given: a populated DB
    let conn = db_with_memories(&[memory("Redis cache")
        .project("testproj")
        .content("redis cache")]);

    // When: query embeds NUL and other control bytes
    let results = rememora::search::search(&conn, "redis\x00\x01\x02", None, None, 10).unwrap();

    // Then: the control bytes are stripped and the search still works
    assert!(
        !results.is_empty(),
        "control chars should be stripped, search should proceed"
    );
}

#[test]
fn search_with_or_in_lowercase_is_treated_as_term() {
    // FTS5 only treats UPPERCASE OR as the operator. Lowercase "or" should
    // be a regular search term — and the bag-of-words OR fallback we build
    // for plain queries must not crash on it.
    let conn = db_with_memories(&[memory("redis cache decision")
        .project("testproj")
        .content("we picked redis over memcache or anything else")]);

    let results = rememora::search::search(&conn, "redis or memcache", None, None, 10).unwrap();
    assert!(!results.is_empty(), "lowercase or is a literal token, not the operator");
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
