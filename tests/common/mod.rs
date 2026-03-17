use rusqlite::Connection;

pub fn create_test_db() -> Connection {
    rememora::db::open_memory().expect("Failed to create test DB")
}

pub fn seed_test_data(conn: &Connection) {
    // Add a test project
    rememora::models::project::add(conn, "testproj", Some("/tmp/testproj"), "Test project", &["rust".into(), "sqlite".into()])
        .expect("Failed to add test project");

    // Add some memories
    rememora::models::context::insert(conn, &rememora::models::context::InsertContext {
        uri: "rememora://projects/testproj/memories/decisions/use-zustand".into(),
        parent_uri: Some("rememora://projects/testproj/memories/decisions".into()),
        context_type: "memory".into(),
        category: Some("decision".into()),
        name: "Use Zustand for state management".into(),
        abstract_text: "Chose Zustand over Redux for simplicity".into(),
        overview: "Zustand was chosen for state management due to its minimal boilerplate and simpler mental model compared to Redux.".into(),
        content: "After evaluating Redux, MobX, and Zustand, we chose Zustand for state management. Key reasons: 1) Minimal boilerplate 2) No providers needed 3) Simple devtools integration.".into(),
        tags: r#"["state","architecture"]"#.into(),
        source_agent: Some("claude-code".into()),
        source_session: None,
        importance: 0.9,
    }).expect("Failed to insert test memory");

    rememora::models::context::insert(conn, &rememora::models::context::InsertContext {
        uri: "rememora://global/memories/preferences/dark-mode".into(),
        parent_uri: Some("rememora://global/memories/preferences".into()),
        context_type: "memory".into(),
        category: Some("preference".into()),
        name: "Prefers dark mode".into(),
        abstract_text: "User prefers dark mode in all editors".into(),
        overview: "Dark mode preference applies to VS Code, terminal, and web apps.".into(),
        content: "User prefers dark mode in all editors and terminals.".into(),
        tags: "[]".into(),
        source_agent: Some("claude-code".into()),
        source_session: None,
        importance: 0.8,
    }).expect("Failed to insert test preference");

    rememora::models::context::insert(conn, &rememora::models::context::InsertContext {
        uri: "rememora://projects/testproj/memories/entities/stripe-api".into(),
        parent_uri: Some("rememora://projects/testproj/memories/entities".into()),
        context_type: "memory".into(),
        category: Some("entity".into()),
        name: "Stripe API integration".into(),
        abstract_text: "Uses Stripe for payments with idempotency keys".into(),
        overview: "The project integrates with Stripe API for payment processing using idempotency keys for safe retries.".into(),
        content: "Stripe integration details...".into(),
        tags: r#"["payments","api"]"#.into(),
        source_agent: Some("codex".into()),
        source_session: None,
        importance: 0.7,
    }).expect("Failed to insert test entity");
}
