mod common;

use rememora::backup::{self, VerifyError};
use rememora::models::context::{self, InsertContext};
use rusqlite::Connection;
use std::path::Path;
use tempfile::TempDir;

const QUERIES: &[&str] = &["zustand state", "dark mode", "memory number", "decision", "rust sqlite"];

fn mem(conn: &Connection, i: usize) -> String {
    context::insert(
        conn,
        &InsertContext {
            uri: format!("rememora://projects/testproj/memories/decisions/memory-{i}"),
            parent_uri: Some("rememora://projects/testproj/memories/decisions".into()),
            context_type: "memory".into(),
            category: Some("decision".into()),
            name: format!("Memory number {i}"),
            abstract_text: format!("Decision {i} about rust sqlite"),
            overview: String::new(),
            content: format!("Memory number {i} content, decision about rust and sqlite"),
            tags: r#"["test"]"#.into(),
            source_agent: Some("claude-code".into()),
            source_session: None,
            importance: 0.25 + (i % 3) as f64 * 0.25,
            worktree: Some("wt".into()),
            branch: Some("br".into()),
        },
    )
    .unwrap()
}

/// File DB with seed data, 20 extra memories, a session, a relation and a supersede link.
fn seeded(path: &Path, key: Option<&str>) -> Connection {
    let conn = open(path, key);
    common::seed_test_data(&conn);
    let ids: Vec<String> = (0..20).map(|i| mem(&conn, i)).collect();
    context::supersede(&conn, &ids[0], &ids[1]).unwrap();
    rememora::models::relation::create(&conn, "rememora://a", "rememora://b", "related", "test").unwrap();
    rememora::models::session::start(
        &conn, "claude-code", Some("testproj"), None, "intent", None,
        &rememora::models::session::Provenance::default(),
    )
    .unwrap();
    conn
}

fn open(path: &Path, key: Option<&str>) -> Connection {
    match key {
        None => rememora::db::open_with_options(path, true).unwrap(),
        Some(k) => {
            // Key the connection the way db::open does, then run migrations via a reopen.
            let c = Connection::open(path).unwrap();
            c.pragma_update(None, "key", k).unwrap();
            c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;").unwrap();
            migrate_keyed(&c);
            c
        }
    }
}

/// db::open resolves keys from the environment, so for an explicit key build
/// the schema by importing an empty export into a keyed connection instead.
fn migrate_keyed(c: &Connection) {
    let plain = rememora::db::open_memory().unwrap();
    let schema: Vec<String> = plain
        .prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'contexts_fts_%' ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for sql in schema {
        c.execute_batch(&format!("{sql};")).unwrap();
    }
    c.execute_batch("INSERT INTO _migrations (name, applied_at) SELECT 'x', 'now' WHERE NOT EXISTS (SELECT 1 FROM _migrations);").unwrap();
}

fn search_ids(conn: &Connection) -> Vec<Vec<String>> {
    QUERIES
        .iter()
        .map(|q| {
            rememora::search::search_with_options(conn, q, None, None, 10, rememora::search::SearchOptions { bump: false })
                .unwrap()
                .into_iter()
                .map(|r| r.context.id)
                .collect()
        })
        .collect()
}

fn superseded(conn: &Connection) -> Vec<(String, Option<String>)> {
    conn.prepare("SELECT id, superseded_by FROM contexts ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn snapshot_while_another_connection_writes() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("live.db");
    let conn = seeded(&db, None);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let db = db.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let w = rememora::db::open_with_options(&db, true).unwrap();
            let mut i = 1000;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                mem(&w, i);
                i += 1;
            }
            i - 1000
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(50));
    let mut snaps = Vec::new();
    for _ in 0..3 {
        snaps.push(backup::create_snapshot(&conn, &dir.path().join("backups"), None).unwrap());
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let written = writer.join().unwrap();
    assert!(written > 0, "writer never ran");
    for (path, manifest) in &snaps {
        let r = backup::verify_snapshot(path, None).unwrap();
        assert_eq!(r.counts, manifest.counts);
        assert_eq!(r.checksum_ok, Some(true));
        // Consistent: memory rows and FTS rows agree.
        let s = Connection::open(path).unwrap();
        let fts: i64 = s.query_row("SELECT COUNT(*) FROM contexts_fts", [], |r| r.get(0)).unwrap();
        assert_eq!(fts, r.counts.contexts);
    }
    assert!(!dir.path().join("backups").join("key").exists());
    assert_eq!(backup::list_snapshots(&dir.path().join("backups")).unwrap().len(), 3);
}

#[test]
fn verify_detects_checksum_mismatch_and_flipped_byte() {
    let dir = TempDir::new().unwrap();
    let conn = seeded(&dir.path().join("live.db"), None);
    let (path, _) = backup::create_snapshot(&conn, dir.path(), None).unwrap();

    // Manifest checksum edited.
    let mpath = backup::manifest_path(&path);
    let raw = std::fs::read_to_string(&mpath).unwrap();
    let mut m: serde_json::Value = serde_json::from_str(&raw).unwrap();
    m["sha256"] = "00".repeat(32).into();
    std::fs::write(&mpath, m.to_string()).unwrap();
    assert!(matches!(backup::verify_snapshot(&path, None), Err(VerifyError::ChecksumMismatch { .. })));
    std::fs::write(&mpath, raw).unwrap();
    assert!(backup::verify_snapshot(&path, None).is_ok());

    // One flipped byte in the file.
    let mut bytes = std::fs::read(&path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(backup::verify_snapshot(&path, None), Err(VerifyError::ChecksumMismatch { .. })));
}

#[test]
fn restore_round_trip_keeps_pre_restore_file() {
    let dir = TempDir::new().unwrap();
    let live = dir.path().join("rememora.db");
    let conn = seeded(&live, None);
    let before = search_ids(&conn);
    let before_counts = backup::counts(&conn).unwrap();
    let (snap, _) = backup::create_snapshot(&conn, &dir.path().join("backups"), None).unwrap();
    for i in 100..110 {
        mem(&conn, i);
    }

    // Refused while our connection is open.
    assert!(backup::restore(&snap, &live, None, false).is_err());
    drop(conn);

    let dry = backup::restore(&snap, &live, None, true).unwrap();
    assert!(dry.dry_run && !dry.temp.exists());
    let plan = backup::restore(&snap, &live, None, false).unwrap();
    let aside = plan.pre_restore.unwrap();
    assert!(aside.exists(), "pre-restore file must be kept");
    let old = rememora::db::open_with_options(&aside, true).unwrap();
    assert_eq!(backup::counts(&old).unwrap().memories, before_counts.memories + 10);

    let restored = rememora::db::open_with_options(&live, true).unwrap();
    assert_eq!(backup::counts(&restored).unwrap(), before_counts);
    assert_eq!(search_ids(&restored), before);
}

#[test]
fn export_all_import_round_trip_and_idempotent() {
    let dir = TempDir::new().unwrap();
    let src = seeded(&dir.path().join("src.db"), None);
    let doc = rememora::dump::export_all(&src).unwrap();
    let dst = rememora::db::open_with_options(&dir.path().join("dst.db"), true).unwrap();

    let dry = rememora::dump::import(&dst, &doc, true).unwrap();
    assert!(dry.tables["contexts"].inserted > 0);
    assert_eq!(backup::counts(&dst).unwrap().contexts, 0);

    let first = rememora::dump::import(&dst, &doc, false).unwrap();
    assert_eq!(backup::counts(&dst).unwrap(), backup::counts(&src).unwrap());
    assert_eq!(superseded(&dst), superseded(&src));
    assert!(superseded(&dst).iter().any(|(_, s)| s.is_some()));
    assert_eq!(search_ids(&dst), search_ids(&src));
    let rel: i64 = dst.query_row("SELECT COUNT(*) FROM relations", [], |r| r.get(0)).unwrap();
    assert_eq!(rel, 1);
    let prov: String = dst.query_row("SELECT worktree FROM contexts WHERE worktree IS NOT NULL LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(prov, "wt");

    let second = rememora::dump::import(&dst, &doc, false).unwrap();
    assert_eq!(second.tables["contexts"].inserted, 0);
    assert_eq!(second.tables["contexts"].skipped, first.tables["contexts"].inserted);
    assert_eq!(backup::counts(&dst).unwrap(), backup::counts(&src).unwrap());
}

#[test]
fn import_rejects_per_project_export() {
    let conn = common::create_test_db();
    let err = rememora::dump::import(&conn, &serde_json::json!([]), false).unwrap_err();
    assert!(err.to_string().contains("Not a rememora export"));
}

#[test]
fn encrypted_snapshot_round_trip_with_explicit_key() {
    let dir = TempDir::new().unwrap();
    let live = dir.path().join("enc.db");
    let key = "explicit-test-key-0123";
    let conn = seeded(&live, Some(key));
    assert!(rememora::crypto::is_db_encrypted(&live));
    let before = search_ids(&conn);
    let (snap, m) = backup::create_snapshot(&conn, dir.path(), Some(key)).unwrap();
    assert!(m.encrypted && rememora::crypto::is_db_encrypted(&snap));
    // Plaintext never appears in the snapshot.
    let bytes = std::fs::read(&snap).unwrap();
    assert!(!bytes.windows(13).any(|w| w == b"Memory number"));

    assert!(matches!(backup::verify_snapshot(&snap, None), Err(VerifyError::KeyMissing)));
    assert!(matches!(backup::verify_snapshot(&snap, Some("wrong")), Err(VerifyError::WrongKey)));
    let r = backup::verify_snapshot(&snap, Some(key)).unwrap();
    assert_eq!(r.counts, m.counts);

    // Without a manifest, a flipped byte in a later page is caught by the HMAC.
    let copy = dir.path().join("copy.db");
    let mut b = bytes.clone();
    let off = b.len() - 2000;
    b[off] ^= 0x01;
    std::fs::write(&copy, &b).unwrap();
    assert!(backup::verify_snapshot(&copy, Some(key)).is_err());

    drop(conn);
    backup::restore(&snap, &live, Some(key), false).unwrap();
    let restored = Connection::open(&live).unwrap();
    restored.pragma_update(None, "key", key).unwrap();
    assert_eq!(search_ids(&restored), before);
}
