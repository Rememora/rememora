mod common;

use rememora::hierarchy;
use rememora::models::context::{self, InsertContext};
use rememora::models::session;

#[test]
fn test_cross_agent_context_transfer() {
    let conn = common::create_test_db();

    // Agent A (Claude Code) registers project and saves memories
    rememora::models::project::add(&conn, "xfer", Some("/tmp/xfer"), "Transfer test project", &[]).unwrap();

    context::insert(
        &conn,
        &InsertContext {
            uri: "rememora://projects/xfer/memories/decisions/use-zustand".into(),
            parent_uri: Some("rememora://projects/xfer/memories/decisions".into()),
            context_type: "memory".into(),
            category: Some("decision".into()),
            name: "Use Zustand".into(),
            abstract_text: "Chose Zustand for state".into(),
            overview: "Full explanation of why Zustand...".into(),
            content: "...".into(),
            tags: "[]".into(),
            source_agent: Some("claude-code".into()),
            source_session: None,
            importance: 0.9,
        },
    )
    .unwrap();

    // Agent A ends session as transferred
    let session_a = session::start(&conn, "claude-code", Some("xfer"), None, "auth flow", None).unwrap();
    session::end(
        &conn,
        &session_a,
        "Login done, token refresh WIP",
        Some("Blocked on secure storage decision. Files: src/auth/login.rs, src/auth/token.rs"),
        Some("transferred"),
    )
    .unwrap();

    // Agent B (Codex) loads context
    let assembly = hierarchy::assemble(&conn, Some("xfer")).unwrap();
    let md = rememora::format::context_to_markdown(&assembly);

    // Agent B should see:
    // 1. The project memories
    assert!(md.contains("Zustand"));
    // 2. The last session's working state
    assert!(md.contains("Blocked on secure storage decision"));
    // 3. The session status
    assert!(md.contains("transferred"));
}

#[test]
fn test_multiple_transfer_chain() {
    let conn = common::create_test_db();
    rememora::models::project::add(&conn, "chain", None, "Chain test", &[]).unwrap();

    // Session 1 (Claude Code) → transferred
    let s1 = session::start(&conn, "claude-code", Some("chain"), None, "start work", None).unwrap();
    session::end(&conn, &s1, "Phase 1 done", Some("Need phase 2"), Some("transferred")).unwrap();

    // Session 2 (Codex) continues from s1 → transferred
    let s2 = session::start(&conn, "codex", Some("chain"), None, "continue work", Some(&s1)).unwrap();
    session::end(&conn, &s2, "Phase 2 done", Some("Need phase 3"), Some("transferred")).unwrap();

    // Session 3 (Claude Code) continues from s2
    let s3 = session::start(&conn, "claude-code", Some("chain"), None, "finish work", Some(&s2)).unwrap();

    // Verify chain
    let s3_record = session::get_by_id(&conn, &s3).unwrap().unwrap();
    assert_eq!(s3_record.parent_session.as_deref(), Some(s2.as_str()));

    let s2_record = session::get_by_id(&conn, &s2).unwrap().unwrap();
    assert_eq!(s2_record.parent_session.as_deref(), Some(s1.as_str()));

    // Latest session should be s3
    let latest = session::get_latest_for_project(&conn, "chain").unwrap().unwrap();
    assert_eq!(latest.id, s3);
}
