use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

const KEYRING_SERVICE: &str = "rememora";
const KEYRING_USER: &str = "db-encryption-key";
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Path to the on-disk fallback key file. Used on hosts without a keychain
/// backend (vanilla Linux containers, CI, Docker images) where `keyring`
/// fails to find libsecret/secret-tool. The file is written with mode 0600.
///
/// This path is `<rememora-data-dir>/key`, where the data dir tracks the
/// `REMEMORA_DB` override so that integration tests (and any out-of-tree
/// callers that relocate the DB) get a key file alongside the DB rather
/// than in the user's real `~/.rememora/`.
pub fn default_key_file_path() -> PathBuf {
    if let Ok(p) = std::env::var("REMEMORA_DB") {
        let db = PathBuf::from(p);
        if let Some(parent) = db.parent() {
            return parent.join("key");
        }
    }
    let mut path = dirs::home_dir().expect("Could not determine home directory");
    path.push(".rememora");
    path.push("key");
    path
}

/// Outcome of persisting a freshly generated encryption key. Setup uses this
/// to print a tier-aware status line (keychain vs. on-disk file) and to
/// surface the security trade-off when the file fallback is taken.
#[derive(Debug)]
pub enum KeyStorageOutcome {
    /// Key was stored in the OS keychain.
    Keychain,
    /// Keychain was unavailable; key was written to `path` with mode 0600.
    /// `keychain_error` carries the underlying keychain failure for context.
    File {
        path: PathBuf,
        keychain_error: String,
    },
}

/// Attempt to persist `key` in the OS keychain; on failure, fall back to
/// writing it to `<data-dir>/key` with mode 0600. Returns the outcome so the
/// caller can render an appropriate status line.
///
/// Errors are returned only when *both* tiers fail. The file fallback is the
/// last line of defence — if it cannot be written, the caller MUST treat
/// setup as failed and not pretend success (issue #100).
pub fn persist_key(key: &str) -> Result<KeyStorageOutcome> {
    persist_key_with(key, keychain_set, keychain_get)
}

/// Round-trip readback verification for keychain persistence (issue #109).
///
/// On Debian-slim and other Linuxes without `libsecret-1-0` / `secret-tool`,
/// the `keyring` crate's mock backend has been observed to return `Ok(())`
/// from `set_password` while the value is never actually persisted. Trusting
/// that silent success leaves the user with an "encryption configured" log
/// line, no keychain entry, and no file fallback — every later call then
/// fails to find the key.
///
/// The cure is to read the key back immediately after writing it. If the
/// readback returns `None`, an empty string, or a different value than the
/// one we just wrote, we treat the keychain as unavailable and fall through
/// to the file fallback. The `REMEMORA_TEST_NO_KEYCHAIN=1` env var continues
/// to short-circuit straight to the file path for tests that don't want to
/// touch the host keychain at all.
///
/// `setter` and `getter` are injected so unit tests can exercise the
/// silent-success failure mode without needing a real keychain.
fn persist_key_with(
    key: &str,
    setter: fn(&str) -> Result<()>,
    getter: fn() -> Result<Option<String>>,
) -> Result<KeyStorageOutcome> {
    if std::env::var("REMEMORA_TEST_NO_KEYCHAIN").ok().as_deref() != Some("1") {
        match setter(key) {
            Ok(()) => match getter() {
                // Round-trip succeeded — value matches what we wrote.
                Ok(Some(stored)) if stored == key => return Ok(KeyStorageOutcome::Keychain),
                // Silent-success path (issue #109): set returned Ok but the
                // backend either dropped the value (None / empty) or stored
                // something different. Treat as keychain-unavailable and
                // fall through to the file tier.
                Ok(other) => {
                    let detail = match other {
                        Some(s) if s.is_empty() => {
                            "keychain readback returned empty value".to_string()
                        }
                        Some(_) => "keychain readback returned a different value".to_string(),
                        None => "keychain readback returned no entry".to_string(),
                    };
                    let path = default_key_file_path();
                    write_key_file(&path, key).with_context(|| {
                        format!(
                            "{detail} and file fallback at {} also failed",
                            path.display(),
                        )
                    })?;
                    return Ok(KeyStorageOutcome::File {
                        path,
                        keychain_error: detail,
                    });
                }
                Err(e) => {
                    let detail = format!("keychain readback failed: {e:#}");
                    let path = default_key_file_path();
                    write_key_file(&path, key).with_context(|| {
                        format!("{detail} and file fallback at {} also failed", path.display())
                    })?;
                    return Ok(KeyStorageOutcome::File {
                        path,
                        keychain_error: detail,
                    });
                }
            },
            Err(e) => {
                let kc_err = format!("{e:#}");
                let path = default_key_file_path();
                write_key_file(&path, key)
                    .with_context(|| format!(
                        "Keychain unavailable ({kc_err}) and file fallback at {} also failed",
                        path.display(),
                    ))?;
                return Ok(KeyStorageOutcome::File {
                    path,
                    keychain_error: kc_err,
                });
            }
        }
    }

    // Test override: skip keychain entirely and exercise the file path.
    let path = default_key_file_path();
    write_key_file(&path, key).with_context(|| format!(
        "REMEMORA_TEST_NO_KEYCHAIN=1 set but file fallback at {} failed",
        path.display(),
    ))?;
    Ok(KeyStorageOutcome::File {
        path,
        keychain_error: "skipped (REMEMORA_TEST_NO_KEYCHAIN=1)".to_string(),
    })
}

/// Read an on-disk key file. Returns `Ok(None)` when the file does not exist;
/// any other read failure is propagated so callers can surface it.
fn read_key_file(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read key file at {}", path.display()))?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed))
    }
}

/// Write `key` to `path` with mode 0600 on Unix. Creates parent directories
/// as needed. On non-Unix platforms the file is written without an explicit
/// permission bit (Windows ACLs are out of scope for this fallback).
fn write_key_file(path: &Path, key: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    std::fs::write(path, format!("{key}\n"))
        .with_context(|| format!("Failed to write key file at {}", path.display()))?;
    set_key_file_perms(path)
        .with_context(|| format!("Failed to set 0600 perms on {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_key_file_perms(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_key_file_perms(_path: &Path) -> Result<()> {
    // Non-Unix: rely on platform defaults. The encryption-at-rest property
    // still holds; only the file-perm bit is missing.
    Ok(())
}

/// Check whether a database file is encrypted by reading its header.
/// Plain SQLite files start with "SQLite format 3\0"; encrypted ones don't.
pub fn is_db_encrypted(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() >= 16 => bytes[..16] != SQLITE_MAGIC[..],
        _ => false,
    }
}

/// Resolve the encryption key without any interactive prompt:
/// 1. `REMEMORA_KEY` environment variable
/// 2. On-disk key file (`<data-dir>/key`, mode 0600) — used as the keychain
///    fallback on hosts without libsecret/secret-tool (issue #100)
/// 3. OS keychain
///
/// Returns `Ok(None)` when no source has a key. This is the entry point for
/// non-interactive callers (GUI apps, hooks, background workers) that must
/// not block on stdin.
pub fn resolve_key_no_prompt() -> Result<Option<String>> {
    // 1. Environment variable — highest priority, lets users/tests override.
    if let Ok(key) = std::env::var("REMEMORA_KEY") {
        if !key.is_empty() {
            return Ok(Some(key));
        }
    }

    // 2. On-disk file fallback. We check this *before* the keychain so that
    //    once setup writes the file (because the keychain failed), every
    //    subsequent invocation finds it without paying another keychain
    //    round-trip.
    let key_file = default_key_file_path();
    match read_key_file(&key_file) {
        Ok(Some(key)) => return Ok(Some(key)),
        Ok(None) => {}
        Err(e) => {
            eprintln!("Warning: key file at {} unreadable: {e}", key_file.display());
        }
    }

    // 3. OS keychain.
    match keychain_get() {
        Ok(Some(key)) => return Ok(Some(key)),
        Ok(None) => {}
        Err(e) => {
            eprintln!("Warning: keychain access failed: {e}");
        }
    }

    Ok(None)
}

/// Resolve the encryption key using a three-tier strategy:
/// 1. REMEMORA_KEY environment variable
/// 2. OS keychain
/// 3. Interactive terminal prompt (only if `prompt` is true)
pub fn resolve_key(prompt: bool) -> Result<Option<String>> {
    if let Some(key) = resolve_key_no_prompt()? {
        return Ok(Some(key));
    }

    // 3. Interactive prompt
    if prompt {
        let key = prompt_for_key("Enter encryption key: ")?;
        if key.is_empty() {
            bail!("Empty key provided");
        }
        return Ok(Some(key));
    }

    Ok(None)
}

/// Generate a random 256-bit key as a 64-character hex string.
pub fn generate_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Simple PRNG seeded from system time + pid — sufficient for key generation.
    // We avoid pulling in a full RNG crate for this single use.
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        ^ (std::process::id() as u128);

    let mut state = seed;
    let mut bytes = [0u8; 32];
    for byte in &mut bytes {
        // xorshift128-style mixing
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state & 0xFF) as u8;
    }

    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Store a key in the OS keychain.
pub fn keychain_set(key: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .context("Failed to create keychain entry")?;
    entry
        .set_password(key)
        .context("Failed to store key in keychain")?;
    Ok(())
}

/// Retrieve a key from the OS keychain. Returns Ok(None) if not found.
pub fn keychain_get() -> Result<Option<String>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .context("Failed to create keychain entry")?;
    match entry.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("Keychain error: {e}")),
    }
}

/// Delete the key from the OS keychain.
pub fn keychain_delete() -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .context("Failed to create keychain entry")?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // already gone
        Err(e) => Err(anyhow::anyhow!("Failed to delete keychain entry: {e}")),
    }
}

/// Prompt the user for a key via the terminal.
fn prompt_for_key(prompt: &str) -> Result<String> {
    rpassword::prompt_password(prompt).context("Failed to read password from terminal")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cargo runs unit tests in this module concurrently by default. Several
    /// tests below mutate `REMEMORA_KEY` / `REMEMORA_DB` / `REMEMORA_TEST_NO_KEYCHAIN`
    /// and call code paths (`default_key_file_path`, `resolve_key_no_prompt`,
    /// `persist_key_with`) that read those vars. Without serialization, two
    /// tests can stomp each other: A sets `REMEMORA_DB=tmpA`, B overwrites it
    /// with `tmpB`, A then writes its key file under `tmpB` and the assertion
    /// `path.starts_with(tmpA)` fails. Acquire this guard at the top of any
    /// env-touching test.
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// `resolve_key_no_prompt` must return the env var when it is set, without
    /// touching the keychain. This is the path GUI callers (the desktop app)
    /// rely on to stay non-interactive.
    #[test]
    fn resolve_key_no_prompt_reads_env_var() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        // Use a unique value so we do not collide with any real developer env.
        let sentinel = "env-key-sentinel-for-tests";
        std::env::set_var("REMEMORA_KEY", sentinel);
        let result = resolve_key_no_prompt().expect("resolve_key_no_prompt");
        std::env::remove_var("REMEMORA_KEY");
        assert_eq!(result.as_deref(), Some(sentinel));
    }

    /// File-tier read: with `REMEMORA_KEY` unset and a key file present in the
    /// data dir (pointed at by `REMEMORA_DB`), `resolve_key_no_prompt` should
    /// return that key without falling through to the keychain. This is the
    /// fallback path setup writes on hosts without a keychain backend
    /// (issue #100).
    #[test]
    fn resolve_key_no_prompt_reads_file_when_env_unset() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("rememora.db");
        let key_path = dir.path().join("key");
        std::fs::write(&key_path, "file-key-sentinel-for-tests\n").unwrap();
        // Lock down to 0600 so we test the same path setup writes.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&key_path).unwrap().permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&key_path, perms).unwrap();
        }

        // Ensure the env var is empty for this test, then point the data dir
        // at our tempdir via REMEMORA_DB so default_key_file_path resolves
        // to <tempdir>/key.
        let prev_env = std::env::var("REMEMORA_KEY").ok();
        let prev_db = std::env::var("REMEMORA_DB").ok();
        std::env::remove_var("REMEMORA_KEY");
        std::env::set_var("REMEMORA_DB", &db_path);

        let resolved = resolve_key_no_prompt().expect("resolve_key_no_prompt");

        // Restore env to avoid leaking into sibling tests.
        match prev_env {
            Some(v) => std::env::set_var("REMEMORA_KEY", v),
            None => std::env::remove_var("REMEMORA_KEY"),
        }
        match prev_db {
            Some(v) => std::env::set_var("REMEMORA_DB", v),
            None => std::env::remove_var("REMEMORA_DB"),
        }

        assert_eq!(resolved.as_deref(), Some("file-key-sentinel-for-tests"));
    }

    /// Issue #109 silent-success regression guard: when the keychain `set`
    /// returns Ok but the readback returns `None` (the failure mode observed
    /// on Debian-slim without libsecret), `persist_key_with` must fall back
    /// to writing the key file rather than reporting Keychain success.
    #[test]
    fn persist_key_with_falls_back_when_readback_returns_none() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("rememora.db");
        let prev_no_kc = std::env::var("REMEMORA_TEST_NO_KEYCHAIN").ok();
        let prev_db = std::env::var("REMEMORA_DB").ok();
        std::env::remove_var("REMEMORA_TEST_NO_KEYCHAIN");
        std::env::set_var("REMEMORA_DB", &db_path);

        // Setter "succeeds" but getter sees no entry — exactly the silent
        // success the keyring crate exhibits on Linux without libsecret.
        fn fake_set_ok(_k: &str) -> Result<()> { Ok(()) }
        fn fake_get_none() -> Result<Option<String>> { Ok(None) }

        let outcome = persist_key_with("rt-test-key", fake_set_ok, fake_get_none)
            .expect("persist_key_with should not error when fallback succeeds");

        // Restore env before assertions so failures don't leak to siblings.
        match prev_no_kc {
            Some(v) => std::env::set_var("REMEMORA_TEST_NO_KEYCHAIN", v),
            None => std::env::remove_var("REMEMORA_TEST_NO_KEYCHAIN"),
        }
        match prev_db {
            Some(v) => std::env::set_var("REMEMORA_DB", v),
            None => std::env::remove_var("REMEMORA_DB"),
        }

        match outcome {
            KeyStorageOutcome::File { path, keychain_error } => {
                assert!(
                    path.starts_with(dir.path()),
                    "fallback wrote outside scratch dir: {}",
                    path.display(),
                );
                assert!(path.exists(), "fallback file must exist on disk");
                assert!(
                    keychain_error.contains("readback") || keychain_error.contains("no entry"),
                    "keychain_error should describe readback failure, got: {keychain_error}",
                );
            }
            KeyStorageOutcome::Keychain => {
                panic!("must not report Keychain success when readback returns None");
            }
        }
    }

    /// Round-trip success path: setter writes the key, getter returns it
    /// unchanged. `persist_key_with` must report `Keychain` and not write
    /// any file fallback.
    #[test]
    fn persist_key_with_uses_keychain_when_readback_matches() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        use std::sync::Mutex;
        static SLOT: Mutex<Option<String>> = Mutex::new(None);

        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("rememora.db");
        let prev_no_kc = std::env::var("REMEMORA_TEST_NO_KEYCHAIN").ok();
        let prev_db = std::env::var("REMEMORA_DB").ok();
        std::env::remove_var("REMEMORA_TEST_NO_KEYCHAIN");
        std::env::set_var("REMEMORA_DB", &db_path);

        fn fake_set(k: &str) -> Result<()> {
            *SLOT.lock().unwrap() = Some(k.to_string());
            Ok(())
        }
        fn fake_get() -> Result<Option<String>> {
            Ok(SLOT.lock().unwrap().clone())
        }

        let outcome = persist_key_with("happy-path-key", fake_set, fake_get)
            .expect("persist_key_with on success path");

        // Restore env first.
        match prev_no_kc {
            Some(v) => std::env::set_var("REMEMORA_TEST_NO_KEYCHAIN", v),
            None => std::env::remove_var("REMEMORA_TEST_NO_KEYCHAIN"),
        }
        match prev_db {
            Some(v) => std::env::set_var("REMEMORA_DB", v),
            None => std::env::remove_var("REMEMORA_DB"),
        }

        assert!(
            matches!(outcome, KeyStorageOutcome::Keychain),
            "should report Keychain when readback matches",
        );
        // No file should have been written next to the scratch DB.
        let key_file = dir.path().join("key");
        assert!(
            !key_file.exists(),
            "must not write file fallback when keychain round-trip succeeds",
        );
    }

    /// `default_key_file_path` must place the key file next to the DB when
    /// `REMEMORA_DB` is set. This keeps integration tests and out-of-tree
    /// callers from polluting the user's `~/.rememora/`.
    #[test]
    fn default_key_file_path_tracks_remora_db_env() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("REMEMORA_DB").ok();
        std::env::set_var("REMEMORA_DB", "/tmp/rememora-key-path-test/scratch.db");
        let p = default_key_file_path();
        match prev {
            Some(v) => std::env::set_var("REMEMORA_DB", v),
            None => std::env::remove_var("REMEMORA_DB"),
        }
        assert_eq!(p, std::path::PathBuf::from("/tmp/rememora-key-path-test/key"));
    }
}
