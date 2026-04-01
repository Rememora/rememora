mod common;

use rememora::search;

#[test]
fn test_store_embedding_roundtrip() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    // Get a context ID from seeded data
    let results = search::search(&conn, "Zustand", None, None, 1).unwrap();
    let ctx_id = &results[0].context.id;

    // Store a fake embedding
    let embedding: Vec<f32> = (0..384).map(|i| (i as f32) / 384.0).collect();
    search::store_embedding(&conn, ctx_id, &embedding, "test-model").unwrap();

    // Verify it was stored in context_embeddings
    let (dims, model): (i64, String) = conn
        .query_row(
            "SELECT dimensions, model_name FROM context_embeddings WHERE context_id = ?1",
            [ctx_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(dims, 384);
    assert_eq!(model, "test-model");
}

#[test]
fn test_store_embedding_blob_size() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results = search::search(&conn, "Zustand", None, None, 1).unwrap();
    let ctx_id = &results[0].context.id;

    let embedding: Vec<f32> = vec![1.0; 384];
    search::store_embedding(&conn, ctx_id, &embedding, "test-model").unwrap();

    // BLOB should be 384 * 4 bytes (f32 = 4 bytes each)
    let blob_len: i64 = conn
        .query_row(
            "SELECT length(embedding) FROM context_embeddings WHERE context_id = ?1",
            [ctx_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(blob_len, 384 * 4);
}

#[test]
fn test_store_embedding_upsert() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results = search::search(&conn, "Zustand", None, None, 1).unwrap();
    let ctx_id = &results[0].context.id;

    // Store twice — should upsert, not duplicate
    let emb1: Vec<f32> = vec![1.0; 384];
    let emb2: Vec<f32> = vec![2.0; 384];
    search::store_embedding(&conn, ctx_id, &emb1, "model-v1").unwrap();
    search::store_embedding(&conn, ctx_id, &emb2, "model-v2").unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM context_embeddings WHERE context_id = ?1",
            [ctx_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Should have the latest model name
    let model: String = conn
        .query_row(
            "SELECT model_name FROM context_embeddings WHERE context_id = ?1",
            [ctx_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(model, "model-v2");
}

#[test]
fn test_hybrid_search_falls_back_to_bm25() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    // Without embeddings, hybrid_search should behave like BM25 search
    let results = search::hybrid_search(&conn, "Zustand", None, None, None, 10).unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.context.name.contains("Zustand")));
}

#[test]
fn test_hybrid_search_with_project_filter() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results =
        search::hybrid_search(&conn, "Zustand", None, Some("testproj"), None, 10).unwrap();
    assert!(!results.is_empty());
    assert!(results
        .iter()
        .all(|r| r.context.uri.contains("testproj") || r.context.uri.contains("global")));
}

#[test]
fn test_hybrid_search_with_category_filter() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results =
        search::hybrid_search(&conn, "Zustand Stripe", None, None, Some("decision"), 10).unwrap();
    assert!(!results.is_empty());
    assert!(results
        .iter()
        .all(|r| r.context.category.as_deref() == Some("decision")));
}

#[test]
fn test_hybrid_search_no_results() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let results =
        search::hybrid_search(&conn, "nonexistentxyz123", None, None, None, 10).unwrap();
    assert!(results.is_empty());
}

// --- Tests below require embed-candle feature for full vector search ---

#[cfg(feature = "embed-candle")]
mod vector {
    use super::*;

    fn seed_with_embeddings(conn: &rusqlite::Connection) {
        common::seed_test_data(conn);

        let results = search::search(conn, "Zustand", None, None, 1).unwrap();
        let zustand_id = &results[0].context.id;

        let results = search::search(conn, "dark mode", None, None, 1).unwrap();
        let darkmode_id = &results[0].context.id;

        let results = search::search(conn, "Stripe", None, None, 1).unwrap();
        let stripe_id = &results[0].context.id;

        // Create distinct embeddings for each — normalized unit vectors along different axes
        let mut emb_zustand = vec![0.0f32; 384];
        emb_zustand[0] = 1.0; // points along dim 0

        let mut emb_dark = vec![0.0f32; 384];
        emb_dark[1] = 1.0; // points along dim 1

        let mut emb_stripe = vec![0.0f32; 384];
        emb_stripe[2] = 1.0; // points along dim 2

        search::store_embedding(conn, zustand_id, &emb_zustand, "test").unwrap();
        search::store_embedding(conn, &darkmode_id, &emb_dark, "test").unwrap();
        search::store_embedding(conn, &stripe_id, &emb_stripe, "test").unwrap();
    }

    #[test]
    fn test_vec_contexts_table_exists() {
        let conn = common::create_test_db();
        // vec_contexts should be created when embed-candle is enabled
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vec_contexts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_store_embedding_populates_vec_contexts() {
        let conn = common::create_test_db();
        seed_with_embeddings(&conn);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_contexts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_vector_search_finds_nearest() {
        let conn = common::create_test_db();
        seed_with_embeddings(&conn);

        // Query with embedding similar to Zustand (along dim 0)
        let mut query_emb = vec![0.0f32; 384];
        query_emb[0] = 0.9;
        query_emb[1] = 0.1; // slight noise

        // Normalize
        let norm: f32 = query_emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        let query_emb: Vec<f32> = query_emb.iter().map(|x| x / norm).collect();

        let results = search::hybrid_search(
            &conn,
            "state management", // BM25 query
            Some(&query_emb),
            None,
            None,
            10,
        )
        .unwrap();

        assert!(!results.is_empty());
        // Zustand should appear (matched by both BM25 "state management" and vector proximity)
        assert!(results.iter().any(|r| r.context.name.contains("Zustand")));
    }

    #[test]
    fn test_vector_search_with_project_filter() {
        let conn = common::create_test_db();
        seed_with_embeddings(&conn);

        let mut query_emb = vec![0.0f32; 384];
        query_emb[0] = 1.0;

        let results = search::hybrid_search(
            &conn,
            "Zustand",
            Some(&query_emb),
            Some("testproj"),
            None,
            10,
        )
        .unwrap();

        // All results should be from testproj or global
        for r in &results {
            assert!(
                r.context.uri.contains("testproj") || r.context.uri.contains("global"),
                "Unexpected URI: {}",
                r.context.uri
            );
        }
    }

    #[test]
    fn test_vector_search_with_category_filter() {
        let conn = common::create_test_db();
        seed_with_embeddings(&conn);

        let mut query_emb = vec![0.0f32; 384];
        query_emb[0] = 1.0;

        let results = search::hybrid_search(
            &conn,
            "Zustand Stripe",
            Some(&query_emb),
            None,
            Some("decision"),
            10,
        )
        .unwrap();

        for r in &results {
            assert_eq!(
                r.context.category.as_deref(),
                Some("decision"),
                "Expected decision category, got: {:?}",
                r.context.category
            );
        }
    }

    #[test]
    fn test_hybrid_search_bumps_active_count_for_vector_results() {
        let conn = common::create_test_db();
        seed_with_embeddings(&conn);

        // Get initial active_count for Stripe (entity)
        let results = search::search(&conn, "Stripe", None, None, 1).unwrap();
        let stripe_id = results[0].context.id.clone();
        let initial_count = rememora::models::context::get_by_id(&conn, &stripe_id)
            .unwrap()
            .unwrap()
            .active_count;

        // Now do a hybrid search that should find Stripe via vector (dim 2)
        let mut query_emb = vec![0.0f32; 384];
        query_emb[2] = 1.0;

        let _ = search::hybrid_search(
            &conn,
            "payments api", // BM25 might also find Stripe
            Some(&query_emb),
            None,
            None,
            10,
        )
        .unwrap();

        let updated_count = rememora::models::context::get_by_id(&conn, &stripe_id)
            .unwrap()
            .unwrap()
            .active_count;

        assert!(
            updated_count > initial_count,
            "active_count should have been bumped: initial={initial_count}, updated={updated_count}"
        );
    }
}
