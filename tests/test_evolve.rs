mod common;

use rememora::models::context::{self, InsertContext};

fn insert_memory(
    conn: &rusqlite::Connection,
    project: &str,
    category: &str,
    slug: &str,
    name: &str,
    content: &str,
    importance: f64,
) -> String {
    let uri = format!("rememora://projects/{project}/memories/{category}/{slug}");
    let parent = format!("rememora://projects/{project}/memories/{category}");
    context::insert(
        conn,
        &InsertContext {
            uri,
            parent_uri: Some(parent),
            context_type: "memory".into(),
            category: Some(category.into()),
            name: name.into(),
            abstract_text: name.into(),
            overview: content.into(),
            content: content.into(),
            tags: "[]".into(),
            source_agent: Some("claude-code".into()),
            source_session: None,
            importance,
        },
    )
    .unwrap()
}

#[test]
fn test_similar_memories_cluster_together() {
    let conn = common::create_test_db();

    // Register project
    rememora::models::project::add(
        &conn,
        "myapp",
        Some("/tmp/myapp"),
        "Test app",
        &["rust".into()],
    )
    .unwrap();

    // Insert two very similar memories about Zustand
    insert_memory(
        &conn,
        "myapp",
        "decision",
        "use-zustand",
        "Use Zustand for state management",
        "We chose Zustand for state management due to minimal boilerplate and simpler API.",
        0.8,
    );
    insert_memory(
        &conn,
        "myapp",
        "decision",
        "zustand-over-redux",
        "Zustand chosen over Redux for state",
        "After evaluating Redux and Zustand, we picked Zustand for state management. Less boilerplate.",
        0.7,
    );

    // Insert an unrelated memory in the same category
    insert_memory(
        &conn,
        "myapp",
        "decision",
        "use-postgres",
        "Use PostgreSQL for the database",
        "PostgreSQL was chosen as the primary database for its reliability and JSON support.",
        0.6,
    );

    // Load all memories and run cluster detection
    let memories = context::list_by_scope(&conn, Some("memory"), None, Some("myapp"), 100).unwrap();
    assert_eq!(memories.len(), 3);

    // Use a low threshold to catch BM25-based similarity
    let clusters =
        rememora::evolve::find_clusters(&conn, memories, 0.05).unwrap();

    // We should find at least one cluster containing the two Zustand memories
    assert!(
        !clusters.is_empty(),
        "Expected at least one cluster for similar Zustand memories"
    );

    // The Zustand memories should be in the same cluster
    let zustand_cluster = clusters.iter().find(|c| {
        c.memories
            .iter()
            .any(|m| m.name.contains("Zustand") || m.name.contains("zustand"))
    });
    assert!(
        zustand_cluster.is_some(),
        "Expected a cluster containing Zustand memories"
    );

    let zustand_cluster = zustand_cluster.unwrap();
    assert!(
        zustand_cluster.memories.len() >= 2,
        "Zustand cluster should have at least 2 memories, got {}",
        zustand_cluster.memories.len()
    );
}

#[test]
fn test_superseded_memories_excluded() {
    let conn = common::create_test_db();

    rememora::models::project::add(
        &conn,
        "myapp2",
        Some("/tmp/myapp2"),
        "Test app 2",
        &["rust".into()],
    )
    .unwrap();

    let id1 = insert_memory(
        &conn,
        "myapp2",
        "decision",
        "old-state",
        "Use Redux for state management",
        "Redux was chosen for state management.",
        0.7,
    );
    let id2 = insert_memory(
        &conn,
        "myapp2",
        "decision",
        "new-state",
        "Use Zustand for state management",
        "Switched from Redux to Zustand for state management.",
        0.8,
    );

    // Supersede old memory
    context::supersede(&conn, &id1, &id2).unwrap();

    // Load active (non-superseded) memories
    let memories =
        context::list_by_scope(&conn, Some("memory"), None, Some("myapp2"), 100).unwrap();
    assert_eq!(memories.len(), 1, "Only the non-superseded memory should load");
    assert_eq!(memories[0].id, id2);
}

#[test]
fn test_no_clusters_with_single_memory() {
    let conn = common::create_test_db();

    rememora::models::project::add(
        &conn,
        "solo",
        Some("/tmp/solo"),
        "Solo project",
        &["rust".into()],
    )
    .unwrap();

    insert_memory(
        &conn,
        "solo",
        "entity",
        "only-one",
        "The only entity memory",
        "This is the only entity memory in the project.",
        0.5,
    );

    let memories =
        context::list_by_scope(&conn, Some("memory"), None, Some("solo"), 100).unwrap();
    assert_eq!(memories.len(), 1);

    let clusters =
        rememora::evolve::find_clusters(&conn, memories, 0.05).unwrap();
    assert!(
        clusters.is_empty(),
        "Should not find clusters with only one memory"
    );
}

#[test]
fn test_dissimilar_memories_fewer_clusters_at_high_threshold() {
    let conn = common::create_test_db();

    rememora::models::project::add(
        &conn,
        "varied",
        Some("/tmp/varied"),
        "Varied project",
        &["rust".into()],
    )
    .unwrap();

    // Insert three memories: two similar (Stripe-related) and one dissimilar
    insert_memory(
        &conn,
        "varied",
        "entity",
        "stripe-api",
        "Stripe API integration for payments",
        "The project uses Stripe API for processing credit card payments with idempotency keys.",
        0.7,
    );
    insert_memory(
        &conn,
        "varied",
        "entity",
        "stripe-webhooks",
        "Stripe webhook handling for payment events",
        "Stripe webhooks are used to handle payment success and failure events asynchronously.",
        0.7,
    );
    insert_memory(
        &conn,
        "varied",
        "entity",
        "kubernetes-deploy",
        "Kubernetes deployment configuration",
        "Production deployments use Kubernetes with Helm charts and ArgoCD for GitOps.",
        0.6,
    );

    let memories =
        context::list_by_scope(&conn, Some("memory"), None, Some("varied"), 100).unwrap();
    assert_eq!(memories.len(), 3);

    // At a low threshold, we may get clusters containing all memories
    let low_clusters =
        rememora::evolve::find_clusters(&conn, memories.clone(), 0.01).unwrap();

    // At a high threshold, we should get fewer or no clusters
    let high_clusters =
        rememora::evolve::find_clusters(&conn, memories, 0.95).unwrap();

    // High threshold should produce fewer total clustered memories than low threshold
    let low_total: usize = low_clusters.iter().map(|c| c.memories.len()).sum();
    let high_total: usize = high_clusters.iter().map(|c| c.memories.len()).sum();
    assert!(
        high_total <= low_total,
        "Higher threshold should cluster fewer memories: high={high_total}, low={low_total}"
    );
}
