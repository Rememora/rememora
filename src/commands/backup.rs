use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::io::IsTerminal;
use std::path::Path;

use rememora::backup::{self, VerifyError};
use rememora::crypto;

/// The key for a database file: required when the file is encrypted.
fn key_for(path: &Path) -> Result<Option<String>> {
    if crypto::is_db_encrypted(path) {
        Ok(Some(crypto::resolve_key(true)?.context("Encryption key required but not available")?))
    } else {
        Ok(None)
    }
}

fn counts_line(c: &backup::Counts) -> String {
    format!(
        "{} contexts, {} memories, {} sessions, {} projects",
        c.contexts, c.memories, c.sessions, c.projects
    )
}

pub fn run_backup(conn: &Connection, db_path: &Path, to: Option<&Path>, keep: Option<usize>, json: bool) -> Result<()> {
    let dir = to.map(Path::to_path_buf).unwrap_or_else(|| backup::default_backup_dir(db_path));
    let key = key_for(db_path)?;
    let (path, manifest) = backup::create_snapshot(conn, &dir, key.as_deref())?;
    let pruned = match keep {
        Some(n) => backup::prune(&dir, n)?,
        None => Vec::new(),
    };
    if json {
        println!(
            "{}",
            serde_json::json!({
                "snapshot": path,
                "manifest": backup::manifest_path(&path),
                "pruned": pruned,
                "details": manifest,
            })
        );
    } else {
        println!("Snapshot written: {}", path.display());
        println!("  {}", counts_line(&manifest.counts));
        println!("  sha256 {}", manifest.sha256);
        for p in &pruned {
            println!("  pruned {}", p.display());
        }
    }
    if manifest.encrypted {
        eprintln!(
            "Note: this snapshot is encrypted with the current database key. The key is not stored \
             in the backup directory; keep it safe (keychain, key file or REMEMORA_KEY), you need it to restore."
        );
    }
    Ok(())
}

pub fn run_verify(snapshot: &Path, json: bool) -> Result<()> {
    let key = if crypto::is_db_encrypted(snapshot) {
        crypto::resolve_key(true)?
    } else {
        None
    };
    match backup::verify_snapshot(snapshot, key.as_deref()) {
        Ok(r) => {
            backup::mark_verified(snapshot)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true, "snapshot": snapshot, "encrypted": r.encrypted,
                        "checksum": match r.checksum_ok { Some(_) => "match", None => "no manifest" },
                        "counts": r.counts,
                    })
                );
            } else {
                println!("OK: {}", snapshot.display());
                println!(
                    "  checksum: {}",
                    if r.checksum_ok.is_some() { "matches manifest" } else { "no manifest, not checked" }
                );
                println!("  integrity_check: ok");
                println!("  encrypted: {}", if r.encrypted { "yes" } else { "no" });
                println!("  {}", counts_line(&r.counts));
            }
            Ok(())
        }
        Err(e) => {
            let kind = match &e {
                VerifyError::Missing(_) => "missing",
                VerifyError::ChecksumMismatch { .. } => "checksum_mismatch",
                VerifyError::KeyMissing => "key_missing",
                VerifyError::WrongKey => "wrong_key",
                VerifyError::WrongKeyOrCorrupt(_) => "wrong_key_or_corrupt",
                VerifyError::Corrupt(_) => "corrupt",
                VerifyError::Other(_) => "error",
            };
            if json {
                println!("{}", serde_json::json!({ "ok": false, "error": kind, "message": e.to_string() }));
            }
            bail!("Verify failed for {}: {e}", snapshot.display())
        }
    }
}

pub fn run_list(db_path: &Path, dir: Option<&Path>, json: bool) -> Result<()> {
    let dir = dir.map(Path::to_path_buf).unwrap_or_else(|| backup::default_backup_dir(db_path));
    let entries = backup::list_snapshots(&dir)?;
    if json {
        let items: Vec<_> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "path": e.path,
                    "created_at": e.created_at().map(|t| t.to_rfc3339()),
                    "size_bytes": e.size_bytes,
                    "counts": e.manifest.as_ref().map(|m| m.counts),
                    "encrypted": e.manifest.as_ref().map(|m| m.encrypted),
                    "verified_at": e.manifest.as_ref().and_then(|m| m.verified_at.clone()),
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "dir": dir, "snapshots": items }));
        return Ok(());
    }
    if entries.is_empty() {
        println!("No snapshots in {}. Run `rememora backup`.", dir.display());
        return Ok(());
    }
    println!("Snapshots in {}:", dir.display());
    for e in &entries {
        let when = e.created_at().map(|t| t.format("%Y-%m-%d %H:%M:%SZ").to_string()).unwrap_or_default();
        let (counts, verified) = match &e.manifest {
            Some(m) => (
                counts_line(&m.counts),
                m.verified_at.as_deref().map(|v| format!("verified {v}")).unwrap_or_else(|| "not verified".into()),
            ),
            None => ("no manifest".to_string(), "not verified".to_string()),
        };
        let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        println!("  {name}  {when}  {} KiB  {counts}  {verified}", e.size_bytes.div_ceil(1024));
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    use std::io::Write;
    eprint!("{prompt} [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

pub fn run_restore(db_path: &Path, snapshot: &Path, dry_run: bool, yes: bool, json: bool) -> Result<()> {
    let key = key_for(snapshot)?;
    // Check first, then ask, then act.
    let plan = backup::restore(snapshot, db_path, key.as_deref(), true)?;
    let describe = |p: &backup::RestorePlan, verb: &str| {
        println!("{verb} {} -> {}", p.snapshot.display(), p.live.display());
        println!("  snapshot verified: {}", counts_line(&p.counts));
        match &p.pre_restore {
            Some(a) => println!("  current database moved aside to {}", a.display()),
            None => println!("  no current database at that path"),
        }
    };
    if dry_run {
        if json {
            println!("{}", serde_json::to_string(&plan)?);
        } else {
            describe(&plan, "Would restore");
            println!("  (dry run, nothing changed)");
        }
        return Ok(());
    }
    if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("Refusing to restore without --yes when not running in a terminal");
        }
        describe(&plan, "About to restore");
        if !confirm("Replace the live database with this snapshot?")? {
            bail!("Restore cancelled");
        }
    }
    let done = backup::restore(snapshot, db_path, key.as_deref(), false)?;
    if json {
        println!("{}", serde_json::to_string(&done)?);
    } else {
        describe(&done, "Restored");
        for p in &done.moved_aside {
            println!("  kept {}", p.display());
        }
    }
    Ok(())
}

pub fn run_export_all(conn: &Connection, out: Option<&Path>) -> Result<()> {
    let doc = rememora::dump::export_all(conn)?;
    let text = serde_json::to_string_pretty(&doc)? + "\n";
    match out {
        Some(p) => {
            std::fs::write(p, text).with_context(|| format!("Failed to write {}", p.display()))?;
            eprintln!("Export written to {}", p.display());
        }
        None => print!("{text}"),
    }
    Ok(())
}

pub fn run_import(conn: &Connection, file: &Path, dry_run: bool, json: bool) -> Result<()> {
    let raw = std::fs::read_to_string(file).with_context(|| format!("Failed to read {}", file.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&raw).context("Import file is not valid JSON")?;
    let report = rememora::dump::import(conn, &doc, dry_run)?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    println!("{} {}:", if dry_run { "Dry run, would import" } else { "Imported" }, file.display());
    for (table, r) in &report.tables {
        if r.inserted + r.skipped > 0 {
            println!("  {table}: {} inserted, {} skipped (already present)", r.inserted, r.skipped);
        }
    }
    Ok(())
}

/// Newest backup summary for `rememora status`.
pub fn status_info(db_path: &Path) -> serde_json::Value {
    let dir = backup::default_backup_dir(db_path);
    match backup::newest_snapshot(&dir).ok().flatten() {
        Some(e) => {
            let created = e.created_at();
            let age = created.map(|t| (chrono::Utc::now() - t).num_seconds().max(0));
            serde_json::json!({
                "dir": dir, "newest": e.path,
                "created_at": created.map(|t| t.to_rfc3339()), "age_seconds": age,
            })
        }
        None => serde_json::json!({ "dir": dir, "newest": null }),
    }
}

pub fn human_age(secs: i64) -> String {
    match secs {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}
