use rusqlite::Connection;
use std::collections::HashSet;

use super::builders::MemoryBuilder;

/// Empty test database (in-memory, all migrations applied).
pub fn db() -> Connection {
    rememora::db::open_memory().expect("Failed to create test DB")
}

/// Test database with a single project registered.
pub fn db_with_project(name: &str) -> Connection {
    let conn = db();
    rememora::models::project::add(
        &conn,
        name,
        Some(&format!("/tmp/{name}")),
        &format!("Test project: {name}"),
        &[],
    )
    .unwrap();
    conn
}

/// Test database pre-loaded with memories.
/// Projects referenced by any memory are auto-created.
pub fn db_with_memories(builders: &[MemoryBuilder]) -> Connection {
    let conn = db();

    // Auto-create any referenced projects
    let mut seen = HashSet::new();
    for b in builders {
        if let Some(proj) = b.project_name() {
            if seen.insert(proj.to_string()) {
                rememora::models::project::add(
                    &conn,
                    proj,
                    Some(&format!("/tmp/{proj}")),
                    &format!("Test project: {proj}"),
                    &[],
                )
                .unwrap();
            }
        }
    }

    for b in builders {
        b.insert(&conn);
    }
    conn
}

/// Test database with a project and pre-loaded sessions.
pub fn db_with_sessions(
    project: &str,
    sessions: &[super::builders::SessionBuilder],
) -> Connection {
    let conn = db_with_project(project);
    for s in sessions {
        s.insert(&conn);
    }
    conn
}
