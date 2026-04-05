use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::io::Read;
use std::path::{Path, PathBuf};

use rememora::curator::{self, Backend, Signal};
use rememora::jsonl;
use rememora::models::watermark;

pub struct CurateArgs {
    pub file: Option<String>,
    pub from_stdin: bool,
    pub auto: bool,
    pub dry_run: bool,
    pub backend: Backend,
    pub reset_watermark: bool,
    pub project: Option<String>,
}

pub fn run(conn: &Connection, args: &CurateArgs, json_output: bool) -> Result<()> {
    if args.reset_watermark {
        return reset_watermarks(conn, args, json_output);
    }

    if args.from_stdin {
        return curate_stdin(conn, args, json_output);
    }

    let files = resolve_files(args)?;

    if files.is_empty() {
        if json_output {
            println!("{{\"status\":\"no_files\",\"message\":\"No JSONL files found\"}}");
        } else {
            println!("No JSONL files found to curate.");
        }
        return Ok(());
    }

    let mut total_curated = 0;
    let mut total_skipped = 0;
    let mut total_no_signal = 0;

    for file in &files {
        match curate_file(conn, file, args, json_output)? {
            FileResult::Curated => total_curated += 1,
            FileResult::NoSignal => total_no_signal += 1,
            FileResult::NoNewContent => total_skipped += 1,
        }
    }

    if !json_output {
        println!(
            "\nDone: {} curated, {} no signal, {} skipped",
            total_curated, total_no_signal, total_skipped
        );
    }

    Ok(())
}

enum FileResult {
    Curated,
    NoSignal,
    NoNewContent,
}

fn curate_file(
    conn: &Connection,
    path: &Path,
    args: &CurateArgs,
    json_output: bool,
) -> Result<FileResult> {
    let path_str = path.to_string_lossy();

    // Get watermark
    let offset = watermark::get(conn, &path_str)?
        .map(|w| w.byte_offset)
        .unwrap_or(0);

    // Parse JSONL from offset
    let parse_result = jsonl::parse_file(path, offset)?;

    if parse_result.entries.is_empty() {
        if !json_output {
            eprintln!("  {} — no new content", path_str);
        }
        return Ok(FileResult::NoNewContent);
    }

    let transcript = jsonl::render_transcript(&parse_result.entries);

    if !json_output {
        eprintln!(
            "  {} — {} entries, {} lines from offset {}{}",
            path_str,
            parse_result.entries.len(),
            parse_result.lines_processed,
            offset,
            if parse_result.truncated {
                " (truncated)"
            } else {
                ""
            }
        );
    }

    // Signal gate
    let signal = curator::signal_gate(&transcript, args.backend)?;

    if signal == Signal::No {
        // Update watermark even if no signal — don't re-process
        watermark::set(conn, &path_str, parse_result.new_offset, parse_result.lines_processed as u64)?;
        watermark::log_action(conn, &path_str, "noop", None, "No signal detected", "")?;

        if !json_output {
            eprintln!("    → no signal, skipping");
        }
        return Ok(FileResult::NoSignal);
    }

    // Detect project
    let project = args
        .project
        .as_deref()
        .or_else(|| detect_project_from_path(path))
        .unwrap_or("unknown");

    if !json_output {
        eprintln!("    → signal detected, curating for project '{project}'...");
    }

    // Run curator
    let result = curator::curate(&transcript, project, args.backend, args.dry_run)?;

    // Update watermark
    watermark::set(conn, &path_str, parse_result.new_offset, parse_result.lines_processed as u64)?;

    // Log the curation
    let action = if args.dry_run { "noop" } else { "add" };
    let reason = result.curator_output.as_deref().unwrap_or("");
    // Truncate reason for the log — full output can be very long
    let log_reason = if reason.len() > 500 {
        &reason[..500]
    } else {
        reason
    };
    watermark::log_action(conn, &path_str, action, None, log_reason, "sonnet")?;

    if json_output {
        let output = serde_json::json!({
            "file": path_str,
            "signal": "yes",
            "dry_run": args.dry_run,
            "entries_processed": parse_result.entries.len(),
            "curator_output": result.curator_output,
        });
        println!("{}", serde_json::to_string(&output)?);
    } else if let Some(output) = &result.curator_output {
        println!("{output}");
    }

    Ok(FileResult::Curated)
}

fn curate_stdin(_conn: &Connection, args: &CurateArgs, json_output: bool) -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read from stdin")?;

    if input.trim().is_empty() {
        bail!("No input on stdin");
    }

    // Parse JSONL from stdin
    let cursor = std::io::Cursor::new(input.into_bytes());
    let parse_result = jsonl::parse_reader(cursor)?;

    if parse_result.entries.is_empty() {
        if json_output {
            println!("{{\"status\":\"no_entries\",\"message\":\"No conversation entries found\"}}");
        } else {
            println!("No conversation entries found in input.");
        }
        return Ok(());
    }

    let transcript = jsonl::render_transcript(&parse_result.entries);

    let signal = curator::signal_gate(&transcript, args.backend)?;
    if signal == Signal::No {
        if json_output {
            println!("{{\"status\":\"no_signal\",\"message\":\"No signal detected\"}}");
        } else {
            println!("No signal detected in transcript.");
        }
        return Ok(());
    }

    let project = args.project.as_deref().unwrap_or("unknown");
    let result = curator::curate(&transcript, project, args.backend, args.dry_run)?;

    if json_output {
        let output = serde_json::json!({
            "source": "stdin",
            "signal": "yes",
            "dry_run": args.dry_run,
            "entries_processed": parse_result.entries.len(),
            "curator_output": result.curator_output,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if let Some(output) = &result.curator_output {
        println!("{output}");
    }

    Ok(())
}

fn reset_watermarks(conn: &Connection, args: &CurateArgs, json_output: bool) -> Result<()> {
    if let Some(file) = &args.file {
        watermark::reset(conn, file)?;
        if json_output {
            println!("{{\"reset\":\"{file}\"}}");
        } else {
            println!("Reset watermark for: {file}");
        }
    } else {
        let wms = watermark::list(conn)?;
        for wm in &wms {
            watermark::reset(conn, &wm.file_path)?;
        }
        if json_output {
            println!("{{\"reset_count\":{}}}", wms.len());
        } else {
            println!("Reset {} watermarks", wms.len());
        }
    }
    Ok(())
}

/// Resolve which JSONL files to process.
fn resolve_files(args: &CurateArgs) -> Result<Vec<PathBuf>> {
    if let Some(file) = &args.file {
        let path = PathBuf::from(file);
        if !path.exists() {
            bail!("File not found: {file}");
        }
        return Ok(vec![path]);
    }

    if args.auto {
        return find_claude_session_files();
    }

    bail!("Specify --file <path>, --from-stdin, or --auto to find Claude Code session files");
}

/// Find Claude Code session JSONL files in the standard location.
fn find_claude_session_files() -> Result<Vec<PathBuf>> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let claude_dir = home.join(".claude").join("projects");

    if !claude_dir.exists() {
        return Ok(vec![]);
    }

    let mut files = Vec::new();

    for project_entry in std::fs::read_dir(&claude_dir)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        for file_entry in std::fs::read_dir(&project_path)? {
            let file_entry = file_entry?;
            let file_path = file_entry.path();
            if file_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(file_path);
            }
        }
    }

    // Sort by modification time, most recent first
    files.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|m| m.modified()).ok();
        let b_time = b.metadata().and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });

    Ok(files)
}

/// Try to detect the project name from the JSONL file path.
///
/// Claude Code stores sessions in `~/.claude/projects/<encoded-path>/`.
/// The directory name is the project path with `/` replaced by `-`.
fn detect_project_from_path(path: &Path) -> Option<&str> {
    // Path looks like: ~/.claude/projects/-Users-user-Projects-myproject/session.jsonl
    // The parent directory name encodes the project path
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
}
