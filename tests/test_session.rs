mod common;

use rememora::models::session;

#[test]
fn test_start_session() {
    let conn = common::create_test_db();
    let id = session::start(&conn, "claude-code", Some("myapp"), Some("/tmp/test"), "auth flow", None).unwrap();

    let s = session::get_by_id(&conn, &id).unwrap().unwrap();
    assert_eq!(s.status, "active");
    assert_eq!(s.agent, "claude-code");
    assert_eq!(s.project.as_deref(), Some("myapp"));
    assert_eq!(s.intent, "auth flow");
    assert!(!s.started_at.is_empty());
    assert!(s.ended_at.is_none());
}

#[test]
fn test_end_session() {
    let conn = common::create_test_db();
    let id = session::start(&conn, "claude-code", Some("myapp"), None, "test", None).unwrap();

    session::end(&conn, &id, "Completed the work", None, None).unwrap();

    let s = session::get_by_id(&conn, &id).unwrap().unwrap();
    assert_eq!(s.status, "ended");
    assert_eq!(s.summary, "Completed the work");
    assert!(s.ended_at.is_some());
}

#[test]
fn test_end_session_transferred() {
    let conn = common::create_test_db();
    let id = session::start(&conn, "claude-code", Some("myapp"), None, "auth flow", None).unwrap();

    session::end(
        &conn,
        &id,
        "Login done, token refresh WIP",
        Some("Blocked on secure storage decision"),
        Some("transferred"),
    )
    .unwrap();

    let s = session::get_by_id(&conn, &id).unwrap().unwrap();
    assert_eq!(s.status, "transferred");
    assert_eq!(s.working_state, "Blocked on secure storage decision");
}

#[test]
fn test_get_latest_for_project() {
    let conn = common::create_test_db();

    session::start(&conn, "claude-code", Some("myapp"), None, "first", None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10)); // ensure different timestamps
    let id2 = session::start(&conn, "codex", Some("myapp"), None, "second", None).unwrap();

    let latest = session::get_latest_for_project(&conn, "myapp").unwrap().unwrap();
    assert_eq!(latest.id, id2);
    assert_eq!(latest.intent, "second");
}

#[test]
fn test_list_sessions() {
    let conn = common::create_test_db();

    session::start(&conn, "claude-code", Some("myapp"), None, "one", None).unwrap();
    session::start(&conn, "codex", Some("myapp"), None, "two", None).unwrap();
    session::start(&conn, "gemini", Some("other"), None, "three", None).unwrap();

    let myapp_sessions = session::list(&conn, Some("myapp"), 10).unwrap();
    assert_eq!(myapp_sessions.len(), 2);

    let all_sessions = session::list(&conn, None, 10).unwrap();
    assert_eq!(all_sessions.len(), 3);

    // Verify ordered by started_at DESC
    assert!(all_sessions[0].started_at >= all_sessions[1].started_at);
}

#[test]
fn test_parent_session_chain() {
    let conn = common::create_test_db();

    let id1 = session::start(&conn, "claude-code", Some("myapp"), None, "start work", None).unwrap();
    session::end(&conn, &id1, "Partial progress", None, Some("transferred")).unwrap();

    let id2 = session::start(&conn, "codex", Some("myapp"), None, "continue work", Some(&id1)).unwrap();

    let s2 = session::get_by_id(&conn, &id2).unwrap().unwrap();
    assert_eq!(s2.parent_session.as_deref(), Some(id1.as_str()));
}
