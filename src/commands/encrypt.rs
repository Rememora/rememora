use anyhow::{bail, Context, Result};
use std::path::Path;

use rememora::crypto;

pub fn run_encrypt(db_path: &Path) -> Result<()> {
    if !db_path.exists() {
        bail!("Database not found at {}", db_path.display());
    }

    if crypto::is_db_encrypted(db_path) {
        bail!("Database is already encrypted");
    }

    cliclack::intro("Encrypt rememora database")?;

    // Resolve or generate a key
    let key = match crypto::resolve_key(false)? {
        Some(k) => {
            cliclack::log::info("Using existing key from environment/keychain")?;
            k
        }
        None => {
            let generated = crypto::generate_key();
            cliclack::log::info(format!("Generated new encryption key"))?;
            generated
        }
    };

    let spinner = cliclack::spinner();
    spinner.start("Encrypting database...");

    // Create encrypted copy using SQLCipher's ATTACH + sqlcipher_export
    let encrypted_path = db_path.with_extension("db.enc");
    let backup_path = db_path.with_extension("db.bak");

    {
        let conn = rusqlite::Connection::open(db_path)
            .context("Failed to open unencrypted database")?;

        // Attach a new encrypted database
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS encrypted KEY '{}';",
            encrypted_path.display(),
            key.replace('\'', "''")
        ))?;

        // Export all data to the encrypted database
        conn.execute_batch("SELECT sqlcipher_export('encrypted');")?;
        conn.execute_batch("DETACH DATABASE encrypted;")?;
    }

    // Verify the encrypted DB can be opened with the key
    {
        let verify_conn = rusqlite::Connection::open(&encrypted_path)
            .context("Failed to open encrypted database for verification")?;
        verify_conn.pragma_update(None, "key", &key)?;
        verify_conn
            .execute_batch("SELECT count(*) FROM sqlite_master;")
            .context("Encrypted database verification failed — key may be wrong")?;
    }

    // Swap: original → .bak, encrypted → original
    std::fs::rename(db_path, &backup_path)
        .context("Failed to create backup of original database")?;
    std::fs::rename(&encrypted_path, db_path)
        .context("Failed to move encrypted database into place")?;

    spinner.stop("Database encrypted successfully");

    // Store key in keychain
    match crypto::keychain_set(&key) {
        Ok(()) => {
            cliclack::log::success("Encryption key stored in OS keychain")?;
        }
        Err(e) => {
            cliclack::log::warning(format!("Could not store key in keychain: {e}"))?;
            cliclack::log::warning(format!(
                "Set REMEMORA_KEY environment variable to this key:\n  {key}"
            ))?;
        }
    }

    cliclack::log::info(format!("Backup saved to {}", backup_path.display()))?;
    cliclack::outro("Done")?;

    Ok(())
}

pub fn run_decrypt(db_path: &Path) -> Result<()> {
    if !db_path.exists() {
        bail!("Database not found at {}", db_path.display());
    }

    if !crypto::is_db_encrypted(db_path) {
        bail!("Database is not encrypted");
    }

    cliclack::intro("Decrypt rememora database")?;

    // Resolve key (required)
    let key = crypto::resolve_key(true)?
        .context("Encryption key required to decrypt")?;

    let spinner = cliclack::spinner();
    spinner.start("Decrypting database...");

    let decrypted_path = db_path.with_extension("db.dec");
    let backup_path = db_path.with_extension("db.bak");

    {
        let conn = rusqlite::Connection::open(db_path)
            .context("Failed to open encrypted database")?;
        conn.pragma_update(None, "key", &key)?;

        // Verify we can read with this key
        conn.execute_batch("SELECT count(*) FROM sqlite_master;")
            .context("Failed to read encrypted database — wrong key?")?;

        // Export to a plaintext database
        conn.execute_batch(&format!(
            "ATTACH DATABASE '{}' AS plaintext KEY '';",
            decrypted_path.display()
        ))?;
        conn.execute_batch("SELECT sqlcipher_export('plaintext');")?;
        conn.execute_batch("DETACH DATABASE plaintext;")?;
    }

    // Verify the decrypted DB
    {
        let verify_conn = rusqlite::Connection::open(&decrypted_path)
            .context("Failed to open decrypted database for verification")?;
        verify_conn
            .execute_batch("SELECT count(*) FROM sqlite_master;")
            .context("Decrypted database verification failed")?;
    }

    // Swap: original → .bak, decrypted → original
    std::fs::rename(db_path, &backup_path)
        .context("Failed to create backup of encrypted database")?;
    std::fs::rename(&decrypted_path, db_path)
        .context("Failed to move decrypted database into place")?;

    spinner.stop("Database decrypted successfully");

    // Offer to remove key from keychain
    if let Ok(Some(_)) = crypto::keychain_get() {
        let should_remove = cliclack::confirm("Remove encryption key from OS keychain?")
            .initial_value(true)
            .interact()?;
        if should_remove {
            crypto::keychain_delete()?;
            cliclack::log::success("Key removed from keychain")?;
        }
    }

    cliclack::log::info(format!("Backup saved to {}", backup_path.display()))?;
    cliclack::outro("Done")?;

    Ok(())
}
