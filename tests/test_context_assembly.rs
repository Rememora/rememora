//! `rememora context`: handoff, sections, token budget, query, JSON.

mod common;

use rememora::format::{self, estimate_tokens};
use rememora::hierarchy::{self, ContextOptions};
use rememora::models::context::{self, InsertContext};
use rememora::models::{project, session};
use rusqlite::{params, Connection};

fn mem(conn: &Connection, proj: &str, category: &str, name: &str, importance: f64) -> String {
    context::insert(
        conn,
        &InsertContext {
            uri: format!("rememora://projects/{proj}/memories/{category}/{}", slug::slugify(name)),
            parent_uri: Some(format!("rememora://projects/{proj}/memories/{category}")),
            context_type: "memory".into(),
            category: Some(category.into()),
            name: name.into(),
            abstract_text: format!("{name} abstract"),
            overview: format!("{name} overview with more detail"),
            content: format!("{name} content"),
            tags: "[]".into(),
            importance,
            ..Default::default()
        },
    )
    .unwrap()
}

fn age(conn: &Connection, id: &str, days: i64) {
    let ts = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    conn.execute(
        "UPDATE contexts SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
        params![ts, id],
    )
    .unwrap();
}

fn age_session(conn: &Connection, id: &str, days: i64) {
    let ts = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    conn.execute(
        "UPDATE sessions SET started_at = ?1, ended_at = ?1 WHERE id = ?2",
        params![ts, id],
    )
    .unwrap();
}

fn all_ids(a: &hierarchy::ContextAssembly) -> Vec<String> {
    a.relevant
        .iter()
        .chain(&a.recent)
        .chain(&a.ranked)
        .map(|s| s.context.id.clone())
        .collect()
}

fn alpha_with_memories(n: usize) -> Connection {
    let conn = common::create_test_db();
    project::add(&conn, "alpha", None, "Alpha billing", &["rust".into()]).unwrap();
    for i in 0..n {
        let id = mem(&conn, "alpha", "decision", &format!("alpha fact {i}"), 0.5 + (i % 5) as f64 / 10.0);
        age(&conn, &id, 30);
    }
    conn
}

// Item 1: B's fresh active session must not shadow A's handoff.
#[test]
fn handoff_survives_a_new_active_session() {
    let conn = alpha_with_memories(2);
    let a = session::start(&conn, "claude-code", Some("alpha"), None, "migrate ledger", None, &Default::default()).unwrap();
    session::end(&conn, &a, "tables created", Some("next: backfill; blocked on review"), None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let b = session::start(&conn, "codex", Some("alpha"), None, "invoice retries", None, &Default::default()).unwrap();

    let asm = hierarchy::assemble(&conn, Some("alpha")).unwrap();
    assert_eq!(asm.handoff.as_ref().unwrap().id, a);
    assert_eq!(asm.active_sessions.len(), 1);
    assert_eq!(asm.active_sessions[0].id, b);

    let md = format::context_to_markdown(&asm);
    assert!(md.contains("## Handoff"), "{md}");
    assert!(md.contains("Working state: next: backfill; blocked on review"), "{md}");
    assert!(md.contains("From: claude-code, ended"), "{md}");
    assert!(md.contains("## Active sessions"), "{md}");
    assert!(md.contains("- codex: invoice retries (started"), "{md}");
    // The active session's intent becomes the implicit query.
    assert_eq!(asm.query.as_deref(), Some("invoice retries"));
    assert!(asm.query_from_session);
}

#[test]
fn auto_summary_without_working_state_is_not_a_handoff() {
    let conn = alpha_with_memories(0);
    let a = session::start(&conn, "claude-code", Some("alpha"), None, "", None, &Default::default()).unwrap();
    session::end(&conn, &a, "real summary", Some("real state"), None).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let b = session::start(&conn, "codex", Some("alpha"), None, "", None, &Default::default()).unwrap();
    let auto = format!("{} Duration: 3m", session::AUTO_SUMMARY_PREFIX);
    session::end(&conn, &b, &auto, None, None).unwrap();

    assert_eq!(session::get_latest_handoff(&conn, "alpha").unwrap().unwrap().id, a);
}

#[test]
fn old_handoff_says_how_old_it_is() {
    let conn = alpha_with_memories(0);
    let a = session::start(&conn, "codex", Some("alpha"), None, "x", None, &Default::default()).unwrap();
    session::end(&conn, &a, "done", Some("state"), Some("transferred")).unwrap();
    age_session(&conn, &a, 20);
    let md = format::context_to_markdown(&hierarchy::assemble(&conn, Some("alpha")).unwrap());
    assert!(md.contains("transferred 20d ago"), "{md}");
    assert!(md.contains("this handoff is 20 days old"), "{md}");
}

// Item 2: no registry pseudo-memory, no duplicates, header carries the project.
#[test]
fn project_row_is_header_not_memory_and_nothing_repeats() {
    let conn = alpha_with_memories(30);
    let asm = hierarchy::assemble(&conn, Some("alpha")).unwrap();
    assert!(all_ids(&asm).iter().all(|id| {
        context::get_by_id(&conn, id).unwrap().unwrap().context_type == "memory"
    }));
    let md = format::render_context(&asm, None).markdown;
    assert!(md.contains("Alpha billing. Stack: rust."), "{md}");
    assert!(!md.contains("[project]"), "{md}");
    assert!(!md.contains("Project: alpha. Stack"), "{md}");
    for i in 0..30 {
        let uri = format!("rememora://projects/alpha/memories/decision/alpha-fact-{i}`");
        assert_eq!(md.matches(&uri).count(), 1, "{uri} should print once:\n{md}");
    }
    let ids = all_ids(&asm);
    let mut dedup = ids.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(ids.len(), dedup.len());
}

#[test]
fn fresh_low_importance_memory_is_not_starved() {
    // More high-importance memories than the old 100-row SQL cap.
    let conn = alpha_with_memories(150);
    let fresh = mem(&conn, "alpha", "decision", "just saved", 0.1);
    let asm = hierarchy::assemble(&conn, Some("alpha")).unwrap();
    assert_eq!(asm.recent.first().map(|s| s.context.id.as_str()), Some(fresh.as_str()));
    let md = format::render_context(&asm, Some(format::DEFAULT_CONTEXT_MAX_TOKENS)).markdown;
    assert!(md.contains("## Recent (last 7 days)\n\n- [decision] just saved abstract"), "{md}");
}

#[test]
fn recent_is_newest_first_and_capped() {
    let conn = alpha_with_memories(0);
    let mut ids = Vec::new();
    for i in 0..8 {
        let id = mem(&conn, "alpha", "event", &format!("recent {i}"), 0.5);
        age(&conn, &id, 6 - (i as i64).min(6));
        ids.push(id);
    }
    let asm = hierarchy::assemble(&conn, Some("alpha")).unwrap();
    assert_eq!(asm.recent.len(), hierarchy::RECENT_LIMIT);
    assert_eq!(asm.recent[0].context.name, "recent 7");
    assert_eq!(asm.recent[1].context.name, "recent 6");
}

#[test]
fn ties_order_deterministically() {
    let conn = alpha_with_memories(0);
    for i in 0..6 {
        let id = mem(&conn, "alpha", "case", &format!("tie {i}"), 0.5);
        conn.execute(
            "UPDATE contexts SET created_at = '2020-01-01T00:00:00+00:00', updated_at = '2020-01-01T00:00:00+00:00' WHERE id = ?1",
            params![id],
        )
        .unwrap();
    }
    let a: Vec<_> = all_ids(&hierarchy::assemble(&conn, Some("alpha")).unwrap());
    let b: Vec<_> = all_ids(&hierarchy::assemble(&conn, Some("alpha")).unwrap());
    assert_eq!(a, b);
    let mut sorted = a.clone();
    sorted.sort();
    assert_eq!(a, sorted, "equal scores fall back to id order");
}

// Item 3: budget.
#[test]
fn estimator_is_chars_over_four_rounded_up() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
}

#[test]
fn output_never_exceeds_budget_and_reports_usage() {
    let conn = alpha_with_memories(60);
    let a = session::start(&conn, "claude-code", Some("alpha"), None, "fact", None, &Default::default()).unwrap();
    session::end(&conn, &a, &"long summary ".repeat(50), Some(&"state ".repeat(80)), None).unwrap();
    let asm = hierarchy::assemble_with(&conn, Some("alpha"), &ContextOptions { query: Some("fact".into()) }).unwrap();
    for n in [5, 40, 120, 300, 600, 1000, 2500] {
        let r = format::render_context(&asm, Some(n));
        assert!(r.tokens_used <= n, "budget {n} exceeded: {}", r.tokens_used);
        assert_eq!(r.tokens_used, estimate_tokens(&r.markdown));
        if n >= 120 {
            assert!(r.markdown.contains(&format!("_~{} tokens (budget {n})", r.tokens_used)), "{}", r.markdown);
            assert_eq!(r.omitted > 0, r.markdown.contains("rememora search"), "{}", r.markdown);
        }
    }
    let r = format::render_context(&asm, Some(600));
    let shown = r.shown.relevant.len() + r.shown.recent.len() + r.shown.top.len() + r.shown.abstracts.len();
    assert_eq!(r.omitted + shown, asm.memory_count());
    assert!(r.omitted > 0);
    // Handoff comes first.
    assert!(r.markdown.contains("## Handoff"));
    let unlimited = format::render_context(&asm, None);
    assert_eq!(unlimited.omitted, 0);
    assert!(unlimited.markdown.contains("nothing omitted"));
}

#[test]
fn top_in_full_then_abstracts() {
    let conn = alpha_with_memories(15);
    let r = format::render_context(&hierarchy::assemble(&conn, Some("alpha")).unwrap(), None);
    assert_eq!(r.shown.top.len(), format::CONTEXT_TOP_FULL);
    assert_eq!(r.shown.abstracts.len(), 5);
    assert!(r.markdown.contains("overview with more detail"));
    assert!(r.markdown.contains("## More memories"));
}

// Item 4: query.
#[test]
fn query_adds_relevant_section_deduplicated() {
    let conn = alpha_with_memories(10);
    let hit = mem(&conn, "alpha", "pattern", "stripe webhook retries", 0.2);
    age(&conn, &hit, 30);
    let asm = hierarchy::assemble_with(&conn, Some("alpha"), &ContextOptions { query: Some("stripe webhook".into()) }).unwrap();
    assert_eq!(asm.relevant.first().map(|s| s.context.id.as_str()), Some(hit.as_str()));
    assert!(!asm.ranked.iter().any(|s| s.context.id == hit));
    let md = format::context_to_markdown(&asm);
    assert!(md.contains("## Relevant to: stripe webhook\n"), "{md}");
    assert_eq!(md.matches("stripe-webhook-retries`").count(), 1);
    // bump disabled: loading context is not a retrieval.
    let rec = context::get_by_id(&conn, &hit).unwrap().unwrap();
    assert_eq!(rec.active_count, 0);
}

// Item 5: JSON.
#[test]
fn json_has_all_sections() {
    let conn = alpha_with_memories(20);
    let a = session::start(&conn, "claude-code", Some("alpha"), None, "", None, &Default::default()).unwrap();
    session::end(&conn, &a, "s", Some("w"), None).unwrap();
    mem(&conn, "alpha", "event", "fresh one", 0.4);
    let asm = hierarchy::assemble_with(&conn, Some("alpha"), &ContextOptions { query: Some("fact".into()) }).unwrap();
    let v: serde_json::Value = serde_json::from_str(&format::context_to_json(&asm, Some(400))).unwrap();
    for k in ["project", "handoff", "active_sessions", "relevant", "recent", "top", "abstracts", "tokens_used", "omitted"] {
        assert!(v.get(k).is_some(), "missing {k}: {v}");
    }
    assert_eq!(v["project"]["name"], "alpha");
    assert_eq!(v["handoff"]["working_state"], "w");
    assert!(v["tokens_used"].as_u64().unwrap() <= 400);
    assert!(v["omitted"].as_u64().unwrap() > 0);
    assert_eq!(v["recent"][0]["name"], "fresh one");
}

// Item 6: `_` in a project name is literal.
#[test]
fn project_filter_treats_underscore_literally() {
    let conn = common::create_test_db();
    mem(&conn, "my_app", "decision", "mine", 0.5);
    mem(&conn, "myXapp", "decision", "not mine", 0.5);
    mem(&conn, "my%app", "decision", "percent", 0.5);
    let names: Vec<String> = context::list_memories_in_scope(&conn, Some("my_app"))
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, vec!["mine".to_string()]);
    let names: Vec<String> = context::list_memories_in_scope(&conn, Some("my%app"))
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, vec!["percent".to_string()]);
}

// `session resume` shows the handoff, not the new empty session.
#[test]
fn resume_cli_shows_handoff_and_active() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("r.db");
    {
        let conn = rememora::db::open_with_options(&db, true).unwrap();
        let a = session::start(&conn, "claude-code", Some("alpha"), None, "ledger", None, &Default::default()).unwrap();
        session::end(&conn, &a, "done", Some("next: backfill"), None).unwrap();
        session::start(&conn, "codex", Some("alpha"), None, "retries", None, &Default::default()).unwrap();
    }
    let out = assert_cmd::Command::cargo_bin("rememora")
        .unwrap()
        .env("REMEMORA_DB", &db)
        .current_dir(dir.path())
        .args(["--no-encryption", "session", "resume", "--project", "alpha"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("next: backfill"), "{text}");
    assert!(text.contains("## Active sessions"), "{text}");
    assert!(text.contains("- codex: retries"), "{text}");

    let out = assert_cmd::Command::cargo_bin("rememora")
        .unwrap()
        .env("REMEMORA_DB", &db)
        .current_dir(dir.path())
        .args(["--no-encryption", "context", "--project", "alpha", "--cheatsheet"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("## Working State\n\nnext: backfill"), "{text}");
    assert!(text.contains("## Active Sessions"), "{text}");
}
