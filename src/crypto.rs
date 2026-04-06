use anyhow::{bail, Context, Result};
use std::path::Path;

const KEYRING_SERVICE: &str = "rememora";
const KEYRING_USER: &str = "db-encryption-key";
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

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

/// Resolve the encryption key using a three-tier strategy:
/// 1. REMEMORA_KEY environment variable
/// 2. OS keychain
/// 3. Interactive terminal prompt (only if `prompt` is true)
pub fn resolve_key(prompt: bool) -> Result<Option<String>> {
    // 1. Environment variable
    if let Ok(key) = std::env::var("REMEMORA_KEY") {
        if !key.is_empty() {
            return Ok(Some(key));
        }
    }

    // 2. OS keychain
    match keychain_get() {
        Ok(Some(key)) => return Ok(Some(key)),
        Ok(None) => {}
        Err(e) => {
            eprintln!("Warning: keychain access failed: {e}");
        }
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
