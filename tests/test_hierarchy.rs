mod common;

use rememora::hierarchy;
use rememora::models::session;

#[test]
fn test_l0_map_includes_global_preferences() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let l0 = hierarchy::get_l0_map(&conn, Some("testproj")).unwrap();
    let has_global_pref = l0.iter().any(|s| s.context.uri.contains("global"));
    assert!(has_global_pref);
}

#[test]
fn test_l0_map_includes_project_memories() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let l0 = hierarchy::get_l0_map(&conn, Some("testproj")).unwrap();
    let has_project_mem = l0.iter().any(|s| s.context.uri.contains("testproj"));
    assert!(has_project_mem);
}

#[test]
fn test_l0_sorted_by_score() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let l0 = hierarchy::get_l0_map(&conn, Some("testproj")).unwrap();
    // Verify sorted by score descending
    for i in 1..l0.len() {
        assert!(l0[i - 1].score >= l0[i].score);
    }
}

#[test]
fn test_l1_context_limits_results() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let l1 = hierarchy::get_l1_context(&conn, Some("testproj"), 2).unwrap();
    assert!(l1.len() <= 2);
}

#[test]
fn test_session_context() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let id = session::start(&conn, "claude-code", Some("testproj"), None, "auth flow", None).unwrap();
    session::end(&conn, &id, "Login done", Some("Token refresh WIP"), None).unwrap();

    let latest = hierarchy::get_session_context(&conn, "testproj").unwrap().unwrap();
    assert_eq!(latest.summary, "Login done");
    assert_eq!(latest.working_state, "Token refresh WIP");
}

#[test]
fn test_full_assembly_to_markdown() {
    let conn = common::create_test_db();
    common::seed_test_data(&conn);

    let id = session::start(&conn, "claude-code", Some("testproj"), None, "testing", None).unwrap();
    session::end(&conn, &id, "Test complete", Some("All good"), None).unwrap();

    let assembly = hierarchy::assemble(&conn, Some("testproj")).unwrap();
    let md = rememora::format::context_to_markdown(&assembly);

    assert!(md.contains("Rememora Context: testproj"));
    assert!(md.contains("Memory Map"));
    assert!(md.contains("Test complete"));
}

#[test]
fn test_empty_project_assembly() {
    let conn = common::create_test_db();

    let assembly = hierarchy::assemble(&conn, Some("nonexistent")).unwrap();
    let md = rememora::format::context_to_markdown(&assembly);

    assert!(md.contains("No memories or sessions found"));
}

// Issue #104: when no project is detected (cwd is not inside a registered
// project), Global mode used to filter to project=None and show an empty
// memory map. The fix: aggregate across every project in the DB.
#[test]
fn test_global_mode_aggregates_across_projects() {
    use rememora::models::context::{self, InsertContext};
    use rememora::models::project;

    let conn = common::create_test_db();

    // Two projects, two memories each.
    project::add(&conn, "alpha", Some("/tmp/alpha"), "Alpha project", &[]).unwrap();
    project::add(&conn, "beta", Some("/tmp/beta"), "Beta project", &[]).unwrap();

    for (proj, mems) in [
        ("alpha", &["alpha decision one", "alpha pattern two"][..]),
        ("beta", &["beta decision one", "beta pattern two"][..]),
    ] {
        for (i, name) in mems.iter().enumerate() {
            let category = if i % 2 == 0 { "decision" } else { "pattern" };
            context::insert(
                &conn,
                &InsertContext {
                    uri: format!(
                        "rememora://projects/{proj}/memories/{category}/m{i}-{}",
                        slug::slugify(name)
                    ),
                    parent_uri: Some(format!(
                        "rememora://projects/{proj}/memories/{category}"
                    )),
                    context_type: "memory".to_string(),
                    category: Some(category.to_string()),
                    name: name.to_string(),
                    abstract_text: format!("abs: {name}"),
                    overview: format!("ov: {name}"),
                    content: format!("body: {name}"),
                    tags: "[]".to_string(),
                    source_agent: Some("claude-code".to_string()),
                    source_session: None,
                    importance: 0.5,
                },
            )
            .unwrap();
        }
    }

    // Assemble in Global mode.
    let assembly = hierarchy::assemble(&conn, None).unwrap();

    // Then: at least 4 entries surface (all memory rows from both projects),
    // covering both project URIs.
    assert!(
        assembly.l0_abstracts.len() >= 4,
        "expected aggregated view across projects, got {} entries",
        assembly.l0_abstracts.len()
    );

    let uris: Vec<&str> = assembly
        .l0_abstracts
        .iter()
        .map(|s| s.context.uri.as_str())
        .collect();
    assert!(uris.iter().any(|u| u.contains("alpha")), "missing alpha");
    assert!(uris.iter().any(|u| u.contains("beta")), "missing beta");

    // The rendered markdown should tag each entry with its project so the
    // user can tell which workspace it came from.
    let md = rememora::format::context_to_markdown(&assembly);
    assert!(
        md.contains("[alpha]"),
        "Global L0 should prefix entries with project name; got:\n{md}"
    );
    assert!(md.contains("[beta]"));
    // And it must not claim the DB is empty.
    assert!(!md.contains("No memories"));
}

// Issue #104: when the DB really is empty, Global mode should still produce a
// distinct, accurate empty-state message.
#[test]
fn test_global_mode_empty_db_message() {
    let conn = common::create_test_db();
    let assembly = hierarchy::assemble(&conn, None).unwrap();
    let md = rememora::format::context_to_markdown(&assembly);
    assert!(
        md.contains("No memories found in the database"),
        "expected accurate Global empty-state, got:\n{md}"
    );
}
