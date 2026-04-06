mod common;

use std::path::Path;
use tempfile::TempDir;

/// Helper: create a file-backed unencrypted DB with test data
fn create_file_db(dir: &Path) -> std::path::PathBuf {
    let db_path = dir.join("test.db");
    let conn = rememora::db::open(&db_path).expect("Failed to create file DB");
    common::seed_test_data(&conn);
    drop(conn);
    db_path
}

#[test]
fn test_is_db_encrypted_plain() {
    let dir = TempDir::new().unwrap();
    let db_path = create_file_db(dir.path());
    assert!(!rememora::crypto::is_db_encrypted(&db_path));
}

#[test]
fn test_is_db_encrypted_nonexistent() {
    assert!(!rememora::crypto::is_db_encrypted(Path::new("/tmp/does_not_exist.db")));
}

#[test]
fn test_encrypt_and_detect() {
    let dir = TempDir::new().unwrap();
    let db_path = create_file_db(dir.path());

    let key = rememora::crypto::generate_key();
    let encrypted_path = dir.path().join("encrypted.db");

    // Encrypt using sqlcipher_export
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS encrypted KEY '{}';",
            encrypted_path.display(),
            key
        ))
        .unwrap();
        conn.execute_batch("SELECT sqlcipher_export('encrypted');")
            .unwrap();
        conn.execute_batch("DETACH DATABASE encrypted;").unwrap();
    }

    // The encrypted file should be detected as encrypted
    assert!(rememora::crypto::is_db_encrypted(&encrypted_path));
    // The original should still be plain
    assert!(!rememora::crypto::is_db_encrypted(&db_path));
}

#[test]
fn test_encrypted_db_roundtrip() {
    let dir = TempDir::new().unwrap();
    let db_path = create_file_db(dir.path());

    let key = rememora::crypto::generate_key();
    let encrypted_path = dir.path().join("encrypted.db");

    // Encrypt
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS encrypted KEY '{}';",
            encrypted_path.display(),
            key
        ))
        .unwrap();
        conn.execute_batch("SELECT sqlcipher_export('encrypted');")
            .unwrap();
        conn.execute_batch("DETACH DATABASE encrypted;").unwrap();
    }

    // Open the encrypted DB with the key and verify data survived
    {
        let conn = rusqlite::Connection::open(&encrypted_path).unwrap();
        conn.pragma_update(None, "key", &key).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM contexts", [], |r| r.get(0))
            .unwrap();
        // seed_test_data creates 1 project + 3 memories = 4 contexts
        assert!(count >= 4, "Expected at least 4 contexts, got {count}");
    }
}

#[test]
fn test_encrypted_db_wrong_key_fails() {
    let dir = TempDir::new().unwrap();
    let db_path = create_file_db(dir.path());

    let key = rememora::crypto::generate_key();
    let wrong_key = rememora::crypto::generate_key();
    let encrypted_path = dir.path().join("encrypted.db");

    // Encrypt
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS encrypted KEY '{}';",
            encrypted_path.display(),
            key
        ))
        .unwrap();
        conn.execute_batch("SELECT sqlcipher_export('encrypted');")
            .unwrap();
        conn.execute_batch("DETACH DATABASE encrypted;").unwrap();
    }

    // Open with wrong key should fail
    let conn = rusqlite::Connection::open(&encrypted_path).unwrap();
    conn.pragma_update(None, "key", &wrong_key).unwrap();
    let result = conn.execute_batch("SELECT count(*) FROM sqlite_master;");
    assert!(result.is_err(), "Should fail with wrong key");
}

#[test]
fn test_decrypt_roundtrip() {
    let dir = TempDir::new().unwrap();
    let db_path = create_file_db(dir.path());

    let key = rememora::crypto::generate_key();
    let encrypted_path = dir.path().join("encrypted.db");
    let decrypted_path = dir.path().join("decrypted.db");

    // Encrypt
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS encrypted KEY '{}';",
            encrypted_path.display(),
            key
        ))
        .unwrap();
        conn.execute_batch("SELECT sqlcipher_export('encrypted');")
            .unwrap();
        conn.execute_batch("DETACH DATABASE encrypted;").unwrap();
    }

    // Decrypt back
    {
        let conn = rusqlite::Connection::open(&encrypted_path).unwrap();
        conn.pragma_update(None, "key", &key).unwrap();
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS plaintext KEY '';",
            decrypted_path.display()
        ))
        .unwrap();
        conn.execute_batch("SELECT sqlcipher_export('plaintext');")
            .unwrap();
        conn.execute_batch("DETACH DATABASE plaintext;").unwrap();
    }

    // Decrypted file should be plain SQLite
    assert!(!rememora::crypto::is_db_encrypted(&decrypted_path));

    // And should have all the data
    let conn = rusqlite::Connection::open(&decrypted_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM contexts", [], |r| r.get(0))
        .unwrap();
    assert!(count >= 4, "Expected at least 4 contexts, got {count}");
}

#[test]
fn test_fts5_works_with_encryption() {
    let dir = TempDir::new().unwrap();
    let db_path = create_file_db(dir.path());

    let key = rememora::crypto::generate_key();
    let encrypted_path = dir.path().join("encrypted.db");

    // Encrypt
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS encrypted KEY '{}';",
            encrypted_path.display(),
            key
        ))
        .unwrap();
        conn.execute_batch("SELECT sqlcipher_export('encrypted');")
            .unwrap();
        conn.execute_batch("DETACH DATABASE encrypted;").unwrap();
    }

    // Open encrypted and search via FTS5
    let conn = rusqlite::Connection::open(&encrypted_path).unwrap();
    conn.pragma_update(None, "key", &key).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contexts_fts WHERE contexts_fts MATCH 'zustand'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(count >= 1, "FTS5 search should find 'zustand' in encrypted DB");
}

#[test]
fn test_wal_mode_with_encryption() {
    let dir = TempDir::new().unwrap();
    let encrypted_path = dir.path().join("wal_test.db");
    let key = rememora::crypto::generate_key();

    // Create an encrypted DB directly
    let conn = rusqlite::Connection::open(&encrypted_path).unwrap();
    conn.pragma_update(None, "key", &key).unwrap();
    conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();

    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal_mode, "wal");
}

#[test]
fn test_generate_key_uniqueness() {
    let key1 = rememora::crypto::generate_key();
    let key2 = rememora::crypto::generate_key();
    assert_ne!(key1, key2, "Generated keys should be unique");
    assert_eq!(key1.len(), 64, "Key should be 64 hex chars (256 bits)");
    assert_eq!(key2.len(), 64);
}

#[test]
fn test_key_resolution_env_var() {
    // Set env var and verify it's picked up
    std::env::set_var("REMEMORA_KEY", "test-key-from-env");
    let key = rememora::crypto::resolve_key(false).unwrap();
    assert_eq!(key, Some("test-key-from-env".to_string()));
    std::env::remove_var("REMEMORA_KEY");
}
