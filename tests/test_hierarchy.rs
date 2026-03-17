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
