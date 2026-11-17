mod common;

use rememora::models::session;

#[test]
fn test_start_session() {
    let conn = common::create_test_db();
    let id = session::start(&conn, "claude-code", Some("myapp"), Some("/tmp/test"), "auth flow", None, &session::Provenance::default()).unwrap();

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
    let id = session::start(&conn, "claude-code", Some("myapp"), None, "test", None, &session::Provenance::default()).unwrap();

    session::end(&conn, &id, "Completed the work", None, None).unwrap();

    let s = session::get_by_id(&conn, &id).unwrap().unwrap();
    assert_eq!(s.status, "ended");
    assert_eq!(s.summary, "Completed the work");
    assert!(s.ended_at.is_some());
}

#[test]
fn test_end_session_transferred() {
    let conn = common::create_test_db();
    let id = session::start(&conn, "claude-code", Some("myapp"), None, "auth flow", None, &session::Provenance::default()).unwrap();

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

    session::start(&conn, "claude-code", Some("myapp"), None, "first", None, &session::Provenance::default()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10)); // ensure different timestamps
    let id2 = session::start(&conn, "codex", Some("myapp"), None, "second", None, &session::Provenance::default()).unwrap();

    let latest = session::get_latest_for_project(&conn, "myapp").unwrap().unwrap();
    assert_eq!(latest.id, id2);
    assert_eq!(latest.intent, "second");
}

#[test]
fn test_list_sessions() {
    let conn = common::create_test_db();

    session::start(&conn, "claude-code", Some("myapp"), None, "one", None, &session::Provenance::default()).unwrap();
    session::start(&conn, "codex", Some("myapp"), None, "two", None, &session::Provenance::default()).unwrap();
    session::start(&conn, "gemini", Some("other"), None, "three", None, &session::Provenance::default()).unwrap();

    let myapp_sessions = session::list(&conn, Some("myapp"), 10).unwrap();
    assert_eq!(myapp_sessions.len(), 2);

    let all_sessions = session::list(&conn, None, 10).unwrap();
    assert_eq!(all_sessions.len(), 3);

    // Verify ordered by started_at DESC
    assert!(all_sessions[0].started_at >= all_sessions[1].started_at);
}

#[test]
fn test_get_active_for_project() {
    let conn = common::create_test_db();

    // Create an active session and an ended session
    let id1 = session::start(&conn, "claude-code", Some("myapp"), None, "first", None, &session::Provenance::default()).unwrap();
    session::end(&conn, &id1, "Done", None, None).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(10));
    let id2 = session::start(&conn, "codex", Some("myapp"), None, "second", None, &session::Provenance::default()).unwrap();

    // Should find the active session (id2), not the ended one (id1)
    let active = session::get_active_for_project(&conn, "myapp").unwrap().unwrap();
    assert_eq!(active.id, id2);
    assert_eq!(active.status, "active");
    assert_eq!(active.intent, "second");
}

#[test]
fn test_get_active_for_project_none_when_all_ended() {
    let conn = common::create_test_db();

    let id = session::start(&conn, "claude-code", Some("myapp"), None, "work", None, &session::Provenance::default()).unwrap();
    session::end(&conn, &id, "Done", None, None).unwrap();

    let active = session::get_active_for_project(&conn, "myapp").unwrap();
    assert!(active.is_none());
}

#[test]
fn test_get_active_for_project_none_when_no_sessions() {
    let conn = common::create_test_db();

    let active = session::get_active_for_project(&conn, "nonexistent").unwrap();
    assert!(active.is_none());
}

#[test]
fn test_get_active_for_project_returns_most_recent() {
    let conn = common::create_test_db();

    // Two active sessions for the same project — should return most recent
    session::start(&conn, "claude-code", Some("myapp"), None, "first active", None, &session::Provenance::default()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let id2 = session::start(&conn, "codex", Some("myapp"), None, "second active", None, &session::Provenance::default()).unwrap();

    let active = session::get_active_for_project(&conn, "myapp").unwrap().unwrap();
    assert_eq!(active.id, id2);
}

#[test]
fn test_parent_session_chain() {
    let conn = common::create_test_db();

    let id1 = session::start(&conn, "claude-code", Some("myapp"), None, "start work", None, &session::Provenance::default()).unwrap();
    session::end(&conn, &id1, "Partial progress", None, Some("transferred")).unwrap();

    let id2 = session::start(&conn, "codex", Some("myapp"), None, "continue work", Some(&id1), &session::Provenance::default()).unwrap();

    let s2 = session::get_by_id(&conn, &id2).unwrap().unwrap();
    assert_eq!(s2.parent_session.as_deref(), Some(id1.as_str()));
}

/// `session end-active` with two agents active on one project: without
/// `--agent` it ends nothing and lists both; with `--agent` it ends only that
/// agent's session.
#[test]
fn end_active_is_scoped_by_agent_when_several_agents_are_active() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rememora.db");
    drop(rememora::db::open_with_options(&db, true).unwrap());
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_rememora"))
            .args(args)
            .arg("--no-encryption")
            .current_dir(dir.path())
            .env("HOME", dir.path())
            .env("REMEMORA_DB", &db)
            .env("REMEMORA_NO_UPDATE_CHECK", "1")
            .env_remove("REMEMORA_READONLY")
            .output()
            .unwrap()
    };
    let start = |agent: &str| {
        let out = run(&["session", "start", "--agent", agent, "--project", "duo", "--intent", agent]);
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let claude = start("claude-code");
    let codex = start("codex");

    let out = run(&["session", "end-active", "--project", "duo", "--summary", "s"]);
    assert!(!out.status.success(), "ambiguous end-active must fail");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains(&claude) && stdout.contains(&codex), "lists candidates: {stdout}");
    assert!(stderr.contains("--agent"), "{stderr}");

    let conn = rememora::db::open_with_options(&db, true).unwrap();
    assert_eq!(session::list_active_for_project(&conn, "duo", 10).unwrap().len(), 2, "nothing ended");
    drop(conn);

    let out = run(&["session", "end-active", "--project", "duo", "--agent", "claude-code", "--summary", "s", "--json"]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains(&claude));

    let conn = rememora::db::open_with_options(&db, true).unwrap();
    let left = session::list_active_for_project(&conn, "duo", 10).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, codex);
    drop(conn);

    // One agent left: the bare form ends it, as before.
    let out = run(&["session", "end-active", "--project", "duo", "--auto-summary"]);
    assert!(out.status.success(), "{out:?}");
    let conn = rememora::db::open_with_options(&db, true).unwrap();
    assert!(session::list_active_for_project(&conn, "duo", 10).unwrap().is_empty());
}
