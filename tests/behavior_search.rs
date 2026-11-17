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

// ---------------------------------------------------------------------------
// Ranking: plain-word queries
// ---------------------------------------------------------------------------

fn top_name(conn: &rusqlite::Connection, query: &str) -> String {
    rememora::search::search(conn, query, None, None, 10)
        .unwrap()
        .first()
        .map(|r| r.context.name.clone())
        .unwrap_or_default()
}

fn names(conn: &rusqlite::Connection, query: &str) -> Vec<String> {
    rememora::search::search(conn, query, None, None, 10)
        .unwrap()
        .into_iter()
        .map(|r| r.context.name)
        .collect()
}

#[test]
fn stopwords_do_not_outrank_the_topic_word() {
    // Given: one memory about postgres, and one that is full of "we" and "did"
    let conn = db_with_memories(&[
        memory("Database choice")
            .abstract_text("Chose Postgres for the ledger")
            .content("Postgres gives us transactional DDL."),
        memory("Team habits")
            .abstract_text("We did standups, we did retros, we did demos")
            .content("We did a lot of meetings and we did not like them."),
    ]);

    // When: asking a natural-language question
    // Then: the postgres memory wins, not the one that says "we did" a lot
    assert_eq!(top_name(&conn, "why did we pick postgres"), "Database choice");
}

#[test]
fn query_of_only_stopwords_still_searches_them() {
    // Given: a memory containing "the"
    let conn = db_with_memories(&[memory("Note").content("the only note")]);

    // When / Then: an all-stopword query falls back to its raw terms
    assert!(!names(&conn, "the").is_empty());
}

#[test]
fn synonyms_bridge_abbreviations_both_ways() {
    // Given: memories written with the short and the long spelling
    let conn = db_with_memories(&[
        memory("Session tokens").content("auth tokens expire after an hour"),
        memory("Primary store").content("We run PostgreSQL 16 in production"),
    ]);

    // When / Then: the other spelling finds each one
    assert_eq!(top_name(&conn, "authentication"), "Session tokens");
    assert_eq!(top_name(&conn, "postgres"), "Primary store");
    assert_eq!(top_name(&conn, "db"), "", "no memory says database or db");
}

#[test]
fn longer_words_also_match_as_prefixes() {
    // Given: a memory using the full word
    let conn = db_with_memories(&[memory("Push").content("notifications are batched hourly")]);

    // When / Then: a five-letter prefix finds it
    assert_eq!(top_name(&conn, "notif"), "Push");
}

#[test]
fn misspelled_word_with_no_hits_is_corrected() {
    // Given: a memory spelled correctly
    let conn = db_with_memories(&[
        memory("Gradle fix").content("android build needs the jdk8 stdlib forced"),
        memory("Other").content("ios build is fine"),
    ]);

    // When / Then: a transposed spelling still finds it
    assert_eq!(top_name(&conn, "andriod build"), "Gradle fix");
}

#[test]
fn correctly_spelled_words_are_not_rewritten() {
    // Given: a word that exists in the index and a near neighbour
    let conn = db_with_memories(&[
        memory("Window").content("the window manager"),
        memory("Widow").content("a widow line in typesetting"),
    ]);

    // When / Then: an exact hit is not widened to its neighbour
    assert_eq!(names(&conn, "widow"), vec!["Widow".to_string()]);
}

#[test]
fn camel_case_query_matches_split_words() {
    // Given: a memory that spells the identifier as words
    let conn = db_with_memories(&[memory("Retries").content("the retry policy caps at five")]);

    // When / Then: the camelCase identifier finds it
    assert_eq!(top_name(&conn, "RetryPolicy"), "Retries");
}

#[test]
fn snake_case_and_flags_match_as_phrases() {
    // Given: one memory with the flag, one with the words apart
    let conn = db_with_memories(&[
        memory("Hooks").content("never commit with --no-verify"),
        memory("Scattered").content("no tests verify this, there is no plan"),
    ]);

    // When / Then: the flag matches the adjacent words first
    assert_eq!(top_name(&conn, "--no-verify"), "Hooks");
}

#[test]
fn memory_covering_every_concept_beats_one_repeating_a_single_term() {
    // Given: one memory mentioning both concepts, one hammering one of them
    let conn = db_with_memories(&[
        memory("Repeats")
            .abstract_text("redis redis redis")
            .content("redis redis redis redis redis"),
        memory("Covers").content("redis eviction policy for the session cache"),
    ]);

    // When / Then: coverage wins
    assert_eq!(top_name(&conn, "redis eviction"), "Covers");
}

#[test]
fn importance_prior_breaks_ties_but_cannot_beat_a_better_match() {
    // Given: two equally matching memories, one far more important and used
    let conn = db_with_memories(&[
        memory("Low").importance(0.1).content("kafka consumer lag"),
        memory("High")
            .importance(1.0)
            .access_count(500)
            .content("kafka consumer lag"),
        memory("Hot but partial")
            .importance(1.0)
            .access_count(10_000)
            .content("kafka topic naming"),
    ]);

    // When / Then: the prior settles the tie, and the partial match stays below
    let got = names(&conn, "kafka consumer lag");
    assert_eq!(got[0], "High");
    assert_eq!(got[1], "Low");
    assert_eq!(got[2], "Hot but partial");
}

#[test]
fn search_bump_false_leaves_counts_alone() {
    let conn = db_with_memories(&[memory("Quiet").content("unbumped words")]);
    let opts = rememora::search::SearchOptions { bump: false };
    let hits = rememora::search::search_with_options(&conn, "unbumped", None, None, 10, opts).unwrap();
    assert_eq!(hits.len(), 1);
    let count: i64 = conn
        .query_row("SELECT active_count FROM contexts WHERE id = ?1", [&hits[0].context.id], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn project_filter_treats_underscore_and_percent_literally() {
    // Given: projects whose names differ only where a LIKE wildcard would sit
    let conn = db_with_memories(&[
        memory("Underscore").project("my_app").content("shared token"),
        memory("Lookalike").project("myxapp").content("shared token"),
        memory("Percent").project("a%b").content("shared token"),
        memory("Lookalike two").project("axxb").content("shared token"),
    ]);

    // When / Then: each filter returns only its own project
    let one = |p: &str| -> Vec<String> {
        rememora::search::search(&conn, "shared token", Some(p), None, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.context.name)
            .collect()
    };
    assert_eq!(one("my_app"), vec!["Underscore".to_string()]);
    assert_eq!(one("a%b"), vec!["Percent".to_string()]);
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[test]
fn compact_format_keeps_about_120_chars_of_abstract() {
    let text = format!("{}END", "word ".repeat(23)); // 118 chars
    let conn = db_with_memories(&[memory("long").project("a_rather_long_project_name").abstract_text(&text)]);
    let results = rememora::search::search(&conn, "word", None, None, 10).unwrap();
    let rendered = rememora::format::search_results_to_compact(&results);
    assert!(rendered.contains("END"), "abstract was cut: {rendered}");
}

#[test]
fn json_hits_carry_fields_needed_without_a_get() {
    let conn = db_with_memories(&[memory("Fielded")
        .project("proj")
        .category("decision")
        .importance(0.8)
        .overview("the overview text")
        .content("the content body")]);
    let results = rememora::search::search(&conn, "fielded", None, None, 10).unwrap();

    let brief: serde_json::Value =
        serde_json::from_str(&rememora::format::search_results_to_json(&results, false)).unwrap();
    let hit = &brief[0];
    assert_eq!(hit["project"], "proj");
    assert_eq!(hit["category"], "decision");
    assert_eq!(hit["overview"], "the overview text");
    assert_eq!(hit["importance"], 0.8);
    assert!(hit["created_at"].is_string());
    assert!(hit["updated_at"].is_string());
    assert!(hit.get("content").is_none(), "content only with --full");

    let full: serde_json::Value =
        serde_json::from_str(&rememora::format::search_results_to_json(&results, true)).unwrap();
    assert_eq!(full[0]["content"], "the content body");
}
