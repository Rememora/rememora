//! Local snapshots of the live database (issue #143).
//!
//! A snapshot is made with the SQLite online backup API. Under SQLCipher the
//! destination connection is keyed with the same key as the source before the
//! copy starts: SQLCipher then copies pages as-is, so the snapshot stays
//! encrypted with that key. An unkeyed destination is refused by SQLCipher
//! ("backup is not supported with encrypted databases"), so a plaintext copy
//! of an encrypted database cannot be produced by accident.
//!
//! The whole copy runs as one `step(-1)`, inside a single read transaction on
//! the source, so writers on other connections (WAL mode) keep going and the
//! snapshot is a consistent point-in-time image.

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Marks snapshots made by `rememora backup`. `--keep` prunes only these.
pub const CREATED_BY: &str = "rememora backup";
const FILE_PREFIX: &str = "rememora-";
const FILE_SUFFIX: &str = ".db";
const MANIFEST_SUFFIX: &str = ".manifest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Counts {
    pub contexts: i64,
    pub memories: i64,
    pub sessions: i64,
    pub projects: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub created_by: String,
    pub created_at: String,
    pub rememora_version: String,
    /// Number of rows in `_migrations`.
    pub migrations: i64,
    pub last_migration: Option<String>,
    pub encrypted: bool,
    pub counts: Counts,
    pub sha256: String,
    pub size_bytes: u64,
    /// Set when a verify of this snapshot last passed.
    #[serde(default)]
    pub verified_at: Option<String>,
}

pub fn counts(conn: &Connection) -> Result<Counts> {
    let q = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    Ok(Counts {
        contexts: q("SELECT COUNT(*) FROM contexts")?,
        memories: q("SELECT COUNT(*) FROM contexts WHERE context_type = 'memory'")?,
        sessions: q("SELECT COUNT(*) FROM sessions")?,
        projects: q("SELECT COUNT(*) FROM contexts WHERE context_type = 'project'")?,
    })
}

fn migrations(conn: &Connection) -> Result<(i64, Option<String>)> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))?;
    let last: Option<String> = conn
        .query_row("SELECT name FROM _migrations ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .ok();
    Ok((n, last))
}

/// `<data dir>/backups`, where the data dir is the live database's directory.
pub fn default_backup_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("backups")
}

pub fn manifest_path(snapshot: &Path) -> PathBuf {
    let mut s = snapshot.as_os_str().to_owned();
    s.push(MANIFEST_SUFFIX);
    PathBuf::from(s)
}

pub fn read_manifest(snapshot: &Path) -> Option<Manifest> {
    let raw = std::fs::read_to_string(manifest_path(snapshot)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_manifest(snapshot: &Path, m: &Manifest) -> Result<()> {
    let path = manifest_path(snapshot);
    std::fs::write(&path, serde_json::to_string_pretty(m)? + "\n")
        .with_context(|| format!("Failed to write manifest {}", path.display()))
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Reserve a fresh, timestamped file name in `dir` without ever overwriting.
fn reserve_snapshot_path(dir: &Path) -> Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    for n in 0..1000 {
        let name = if n == 0 {
            format!("{FILE_PREFIX}{stamp}{FILE_SUFFIX}")
        } else {
            format!("{FILE_PREFIX}{stamp}-{n}{FILE_SUFFIX}")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("Failed to create {}", path.display())),
        }
    }
    bail!("Could not find a free snapshot name in {}", dir.display())
}

/// Snapshot `src` into a new file in `dir`, write its manifest, and return
/// the snapshot path. `key` must be the source's key when it is encrypted.
pub fn create_snapshot(src: &Connection, dir: &Path, key: Option<&str>) -> Result<(PathBuf, Manifest)> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create backup directory {}", dir.display()))?;
    let path = reserve_snapshot_path(dir)?;
    let result = copy_into(src, &path, key);
    if let Err(e) = result {
        // Only our own, just-reserved file is removed.
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }

    let report = verify_snapshot(&path, key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let (migrations, last_migration) = {
        let conn = open_snapshot_readonly(&path, key)?;
        migrations(&conn)?
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let manifest = Manifest {
        created_by: CREATED_BY.to_string(),
        created_at: now.clone(),
        rememora_version: env!("CARGO_PKG_VERSION").to_string(),
        migrations,
        last_migration,
        encrypted: crate::crypto::is_db_encrypted(&path),
        counts: report.counts,
        sha256: sha256_file(&path)?,
        size_bytes: std::fs::metadata(&path)?.len(),
        verified_at: Some(now),
    };
    write_manifest(&path, &manifest)?;
    Ok((path, manifest))
}

fn copy_into(src: &Connection, path: &Path, key: Option<&str>) -> Result<()> {
    let mut dst = Connection::open(path)?;
    if let Some(k) = key {
        dst.pragma_update(None, "key", k)?;
    }
    {
        let backup = rusqlite::backup::Backup::new(src, &mut dst)?;
        let mut attempts = 0;
        loop {
            match backup.step(-1)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => continue,
                rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                    attempts += 1;
                    if attempts > 200 {
                        bail!("Database stayed busy; snapshot not taken");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                _ => continue,
            }
        }
    }
    // The copied header says WAL; switch the snapshot to a single
    // self-contained file with no -wal/-shm sidecars.
    let mode: String = dst.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        bail!("Could not finalise snapshot (journal_mode={mode})");
    }
    Ok(())
}

/// Open a snapshot read-only. No migrations, no runtime PRAGMAs.
pub fn open_snapshot_readonly(path: &Path, key: Option<&str>) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("Failed to open {}", path.display()))?;
    if crate::crypto::is_db_encrypted(path) {
        if let Some(k) = key {
            conn.pragma_update(None, "key", k)?;
        }
    }
    Ok(conn)
}

#[derive(Debug)]
pub struct VerifyReport {
    pub counts: Counts,
    pub encrypted: bool,
    /// `Some(true)` when the manifest checksum matched, `None` without manifest.
    pub checksum_ok: Option<bool>,
}

#[derive(Debug)]
pub enum VerifyError {
    Missing(PathBuf),
    ChecksumMismatch { expected: String, actual: String },
    KeyMissing,
    /// The file is unchanged since it was written, so the key must be wrong.
    WrongKey,
    /// Encrypted and unreadable, with no manifest to say which.
    WrongKeyOrCorrupt(String),
    Corrupt(String),
    Other(anyhow::Error),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::Missing(p) => write!(f, "Snapshot not found: {}", p.display()),
            VerifyError::ChecksumMismatch { expected, actual } => write!(
                f,
                "Checksum mismatch: the manifest says sha256 {expected}, the file is {actual}. \
                 The snapshot changed or is corrupt since it was taken."
            ),
            VerifyError::KeyMissing => write!(
                f,
                "Snapshot is encrypted and no key is available. Set REMEMORA_KEY to the key the \
                 database had when the snapshot was taken."
            ),
            VerifyError::WrongKey => write!(
                f,
                "Wrong key: the file matches its manifest checksum but cannot be decrypted with \
                 the current key. Set REMEMORA_KEY to the key used when the snapshot was taken."
            ),
            VerifyError::WrongKeyOrCorrupt(e) => write!(
                f,
                "Cannot read the snapshot: wrong key or corrupt file (no manifest to tell them \
                 apart): {e}"
            ),
            VerifyError::Corrupt(e) => write!(f, "Snapshot is corrupt: {e}"),
            VerifyError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Check a snapshot: manifest checksum (when present), decryptability,
/// `PRAGMA integrity_check`, and row counts.
pub fn verify_snapshot(path: &Path, key: Option<&str>) -> Result<VerifyReport, VerifyError> {
    if !path.is_file() {
        return Err(VerifyError::Missing(path.to_path_buf()));
    }
    let checksum_ok = match read_manifest(path) {
        Some(m) => {
            let actual = sha256_file(path).map_err(VerifyError::Other)?;
            if actual != m.sha256 {
                return Err(VerifyError::ChecksumMismatch { expected: m.sha256, actual });
            }
            Some(true)
        }
        None => None,
    };
    let encrypted = crate::crypto::is_db_encrypted(path);
    if encrypted && key.is_none() {
        return Err(VerifyError::KeyMissing);
    }
    let conn = open_snapshot_readonly(path, key).map_err(VerifyError::Other)?;

    if let Err(e) = conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)) {
        let msg = e.to_string();
        return Err(match (encrypted, checksum_ok) {
            (true, Some(true)) => VerifyError::WrongKey,
            (true, None) => VerifyError::WrongKeyOrCorrupt(msg),
            _ => VerifyError::Corrupt(msg),
        });
    }

    let integrity: Result<Vec<String>, rusqlite::Error> = conn
        .prepare("PRAGMA integrity_check")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0))?.collect());
    match integrity {
        Ok(lines) if lines.len() == 1 && lines[0] == "ok" => {}
        Ok(lines) => return Err(VerifyError::Corrupt(lines.join("; "))),
        Err(e) => return Err(VerifyError::Corrupt(e.to_string())),
    }

    let counts = counts(&conn).map_err(|e| VerifyError::Corrupt(format!("{e:#}")))?;
    Ok(VerifyReport { counts, encrypted, checksum_ok })
}

/// Record a passing verify in the snapshot's manifest, if it has one.
pub fn mark_verified(snapshot: &Path) -> Result<()> {
    if let Some(mut m) = read_manifest(snapshot) {
        m.verified_at = Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        write_manifest(snapshot, &m)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SnapshotEntry {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub manifest: Option<Manifest>,
}

impl SnapshotEntry {
    /// Manifest `created_at`, falling back to the file's mtime.
    pub fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        if let Some(m) = &self.manifest {
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&m.created_at) {
                return Some(t.with_timezone(&chrono::Utc));
            }
        }
        std::fs::metadata(&self.path).and_then(|m| m.modified()).ok().map(Into::into)
    }
}

/// Snapshots in `dir`, oldest first. Missing dir is an empty list.
pub fn list_snapshots(dir: &Path) -> Result<Vec<SnapshotEntry>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !(name.starts_with(FILE_PREFIX) && name.ends_with(FILE_SUFFIX)) || !path.is_file() {
            continue;
        }
        out.push(SnapshotEntry {
            size_bytes: std::fs::metadata(&path)?.len(),
            manifest: read_manifest(&path),
            path,
        });
    }
    out.sort_by_key(|e| (e.created_at(), e.path.clone()));
    Ok(out)
}

pub fn newest_snapshot(dir: &Path) -> Result<Option<SnapshotEntry>> {
    Ok(list_snapshots(dir)?.pop())
}

/// Remove all but the newest `keep` snapshots made by `rememora backup`
/// (manifest present with `created_by` set by this module). Returns what was
/// removed. Files without such a manifest are never touched.
pub fn prune(dir: &Path, keep: usize) -> Result<Vec<PathBuf>> {
    let ours: Vec<SnapshotEntry> = list_snapshots(dir)?
        .into_iter()
        .filter(|e| e.manifest.as_ref().is_some_and(|m| m.created_by == CREATED_BY))
        .collect();
    let excess = ours.len().saturating_sub(keep);
    let mut removed = Vec::new();
    for e in ours.into_iter().take(excess) {
        std::fs::remove_file(&e.path)
            .with_context(|| format!("Failed to remove {}", e.path.display()))?;
        let _ = std::fs::remove_file(manifest_path(&e.path));
        removed.push(e.path);
    }
    Ok(removed)
}

/// What `restore` did, or would do under `dry_run`.
#[derive(Debug, Clone, Serialize)]
pub struct RestorePlan {
    pub snapshot: PathBuf,
    pub live: PathBuf,
    pub temp: PathBuf,
    /// Where the current live database (and its -wal/-shm) is moved. `None`
    /// when there is no live database yet.
    pub pre_restore: Option<PathBuf>,
    pub moved_aside: Vec<PathBuf>,
    pub counts: Counts,
    pub dry_run: bool,
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Fail unless this process can get exclusive access to the live database.
///
/// Leaving WAL mode needs every other connection closed, so a refused
/// `journal_mode = DELETE` (with no busy wait) means another process has the
/// database open. On success the WAL has been checkpointed into the main
/// file, so the file moved aside is complete on its own. The next normal open
/// switches the database back to WAL.
pub fn ensure_exclusive(live: &Path, key: Option<&str>) -> Result<()> {
    let conn = Connection::open(live)?;
    if crate::crypto::is_db_encrypted(live) {
        match key {
            Some(k) => conn.pragma_update(None, "key", k)?,
            None => bail!("The live database is encrypted and no key is available"),
        }
    }
    conn.busy_timeout(std::time::Duration::from_millis(0))?;
    let mode: Result<String, _> = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0));
    match mode {
        Ok(m) if m.eq_ignore_ascii_case("delete") => Ok(()),
        Ok(m) => bail!(
            "Another process has the database open (journal_mode stayed {m}). \
             Close other rememora processes (MCP servers, hooks, the desktop app) and retry."
        ),
        Err(e) => bail!(
            "Another process has the database open ({e}). Close other rememora processes \
             (MCP servers, hooks, the desktop app) and retry."
        ),
    }
}

/// Restore `snapshot` over `live`. Verifies the snapshot, copies it to a
/// temporary file next to `live`, verifies that copy, then moves the current
/// database (and -wal/-shm) aside to `<live>.pre-restore-<UTC>` and moves the
/// copy into place. Nothing is deleted.
pub fn restore(snapshot: &Path, live: &Path, key: Option<&str>, dry_run: bool) -> Result<RestorePlan> {
    let report = verify_snapshot(snapshot, key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let temp = sibling(live, &format!(".restore-tmp-{stamp}"));
    let live_exists = live.exists();
    let pre_restore = live_exists.then(|| sibling(live, &format!(".pre-restore-{stamp}")));
    let mut plan = RestorePlan {
        snapshot: snapshot.to_path_buf(),
        live: live.to_path_buf(),
        temp: temp.clone(),
        pre_restore: pre_restore.clone(),
        moved_aside: Vec::new(),
        counts: report.counts,
        dry_run,
    };
    if let Some(aside) = &pre_restore {
        for suffix in ["", "-wal", "-shm"] {
            if sibling(live, suffix).exists() {
                plan.moved_aside.push(sibling(aside, suffix));
            }
        }
        if plan.moved_aside.iter().any(|p| p.exists()) || temp.exists() {
            bail!("Refusing to overwrite an existing file next to {}", live.display());
        }
    }
    if dry_run {
        return Ok(plan);
    }

    if live_exists {
        ensure_exclusive(live, key)?;
    }
    if let Some(parent) = live.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(snapshot, &temp)
        .with_context(|| format!("Failed to copy snapshot to {}", temp.display()))?;
    if let Err(e) = verify_snapshot(&temp, key) {
        bail!("Restored copy failed verification, live database untouched (copy left at {}): {e}", temp.display());
    }

    plan.moved_aside.clear();
    if let Some(aside) = &pre_restore {
        for suffix in ["", "-wal", "-shm"] {
            let from = sibling(live, suffix);
            if from.exists() {
                let to = sibling(aside, suffix);
                std::fs::rename(&from, &to).with_context(|| {
                    format!("Failed to move {} aside to {}", from.display(), to.display())
                })?;
                plan.moved_aside.push(to);
            }
        }
    }
    std::fs::rename(&temp, live)
        .with_context(|| format!("Failed to move {} into place", temp.display()))?;
    Ok(plan)
}
