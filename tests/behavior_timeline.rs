//! Behavior tests: `rememora timeline` — chronological slice around an anchor.
//!
//! Exercises `rememora::timeline::build_timeline`, the library-side primitive
//! that the `timeline` CLI verb wraps.

mod scenarios;

use rememora::timeline::{self, TimelineArgs, TimelineOrder};
use rusqlite::Connection;
use scenarios::{db_with_memories, memory};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Backdate `contexts.created_at` so `slice_by_ts` has a deterministic order
/// regardless of insert timing. `accessed_days_ago` only touches `updated_at`;
/// the timeline slices on `created_at`, so we have to rewrite that too.
fn backdate_created(conn: &Connection, uri: &str, days_ago: i64) {
    let past = chrono::Utc::now() - chrono::Duration::days(days_ago);
    conn.execute(
        "UPDATE contexts SET created_at = ?1, updated_at = ?1 WHERE uri = ?2",
        rusqlite::params![past.to_rfc3339(), uri],
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Core slicing
// ---------------------------------------------------------------------------

#[test]
fn timeline_returns_before_and_after_slice_around_anchor() {
    // Given: five memories in one project, created at deterministic times
    let conn = db_with_memories(&[
        memory("Oldest decision").project("testproj").category("decision"),
        memory("Older decision").project("testproj").category("decision"),
        memory("Anchor decision").project("testproj").category("decision"),
        memory("Newer decision").project("testproj").category("decision"),
        memory("Newest decision").project("testproj").category("decision"),
    ]);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/oldest-decision", 40);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/older-decision", 20);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/anchor-decision", 10);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/newer-decision", 5);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/newest-decision", 1);

    // When: building a timeline with before=2, after=2
    let t = timeline::build_timeline(
        &conn,
        &TimelineArgs {
            anchor: "rememora://projects/testproj/memories/decision/anchor-decision".into(),
            before: 2,
            after: 2,
            project: None,
            by: TimelineOrder::Ts,
        },
    )
    .unwrap();

    // Then: anchor is the middle memory, before holds the two older items
    // (oldest-first), after holds the two newer items (oldest-first)
    assert_eq!(t.anchor.name, "Anchor decision");
    assert_eq!(
        t.before.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec!["Oldest decision", "Older decision"]
    );
    assert_eq!(
        t.after.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec!["Newer decision", "Newest decision"]
    );
}

#[test]
fn timeline_respects_before_after_limits() {
    // Given: several memories, but only 1 wanted on each side
    let conn = db_with_memories(&[
        memory("A").project("testproj"),
        memory("B").project("testproj"),
        memory("Anchor").project("testproj"),
        memory("C").project("testproj"),
        memory("D").project("testproj"),
    ]);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/a", 40);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/b", 20);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/anchor", 10);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/c", 5);
    backdate_created(&conn, "rememora://projects/testproj/memories/decision/d", 1);

    // When: before=1, after=1
    let t = timeline::build_timeline(
        &conn,
        &TimelineArgs {
            anchor: "rememora://projects/testproj/memories/decision/anchor".into(),
            before: 1,
            after: 1,
            project: None,
            by: TimelineOrder::Ts,
        },
    )
    .unwrap();

    // Then: only the closest neighbour on each side is returned
    assert_eq!(t.before.len(), 1);
    assert_eq!(t.before[0].name, "B"); // newest-older
    assert_eq!(t.after.len(), 1);
    assert_eq!(t.after[0].name, "C"); // oldest-newer
}

#[test]
fn timeline_by_hotness_prefers_hot_neighbours() {
    // Given: peers with different hotness; anchor in the middle chronologically
    // but we explicitly request hotness ordering
    let conn = db_with_memories(&[
        memory("cold-a").project("testproj").importance(0.1).access_count(0),
        memory("hot-b").project("testproj").importance(0.9).access_count(100),
        memory("anchor").project("testproj"),
        memory("cold-c").project("testproj").importance(0.1).access_count(0),
        memory("hot-d").project("testproj").importance(0.9).access_count(100),
    ]);

    // When: building a hotness-ordered timeline of size 2
    let t = timeline::build_timeline(
        &conn,
        &TimelineArgs {
            anchor: "rememora://projects/testproj/memories/decision/anchor".into(),
            before: 1,
            after: 1,
            project: None,
            by: TimelineOrder::Hotness,
        },
    )
    .unwrap();

    // Then: both slots go to the hot memories, cold ones are excluded
    let all_names: Vec<&str> = t
        .before
        .iter()
        .chain(t.after.iter())
        .map(|c| c.name.as_str())
        .collect();
    assert!(all_names.contains(&"hot-b"), "hot-b should appear in timeline, got {all_names:?}");
    assert!(all_names.contains(&"hot-d"), "hot-d should appear in timeline, got {all_names:?}");
    assert!(!all_names.contains(&"cold-a"), "cold-a should not beat hot peers");
    assert!(!all_names.contains(&"cold-c"), "cold-c should not beat hot peers");
}

#[test]
fn timeline_missing_anchor_errors() {
    // Given: an empty DB
    let conn = db_with_memories(&[]);

    // When: asking for a timeline around a URI that isn't there
    let err = timeline::build_timeline(
        &conn,
        &TimelineArgs {
            anchor: "rememora://projects/nope/memories/decision/ghost".into(),
            before: 3,
            after: 3,
            project: None,
            by: TimelineOrder::Ts,
        },
    )
    .unwrap_err();

    // Then: error mentions the missing URI
    let msg = err.to_string();
    assert!(msg.contains("Context not found"), "unexpected error: {msg}");
}

#[test]
fn timeline_scopes_peers_to_anchor_project() {
    // Given: one memory in project-alpha (the anchor) and one in project-beta
    let conn = db_with_memories(&[
        memory("anchor").project("alpha"),
        memory("stranger").project("beta"),
    ]);

    // When: building a timeline around the alpha anchor
    let t = timeline::build_timeline(
        &conn,
        &TimelineArgs {
            anchor: "rememora://projects/alpha/memories/decision/anchor".into(),
            before: 3,
            after: 3,
            project: None,
            by: TimelineOrder::Ts,
        },
    )
    .unwrap();

    // Then: the beta memory is not pulled into the timeline
    let all_names: Vec<&str> = t
        .before
        .iter()
        .chain(t.after.iter())
        .map(|c| c.name.as_str())
        .collect();
    assert!(!all_names.contains(&"stranger"), "peer leaked across project boundary: {all_names:?}");
}
