//! `rememora watch-transcript` — curate non-Claude agent session JSONL.
//!
//! Phase 1: Codex only, one-shot by default, optional `--follow` mode that
//! polls the file every 500 ms for new content. **Not a daemon.** The user
//! runs this per session; there is no launchd/systemd integration, no plugin
//! hook, no HTTP server, no persistent background worker.

use anyhow::{bail, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rememora::curator::{self, Signal};
use rememora::jsonl_codex;
use rememora::models::agent_invocation::{self, Caller};
use rememora::models::watermark;

/// How often `--follow` mode re-checks the file for new content.
const FOLLOW_POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct WatchTranscriptArgs {
    /// Agent kind. Only `codex` is supported in Phase 1.
    pub agent: String,
    /// Path to the rollout JSONL.
    pub path: PathBuf,
    /// If set, poll the file for new content after each pass.
    pub follow: bool,
    /// Project name override. Falls back to `session_meta.cwd` basename from
    /// the rollout, then to `"unknown"`.
    pub project: Option<String>,
    /// If true, curator shows what it *would* do without executing.
    pub dry_run: bool,
}

pub fn run(conn: &Connection, args: &WatchTranscriptArgs, json_output: bool) -> Result<()> {
    if !args.agent.eq_ignore_ascii_case("codex") {
        bail!(
            "Phase 1 supports --agent codex only (got: {}). Gemini CLI and a \
             declarative --schema option are tracked as Phase 2.",
            args.agent
        );
    }

    if !args.path.exists() {
        bail!("Transcript file not found: {}", args.path.display());
    }

    // One-shot pass first. `--follow` wraps it in a polling loop that honors
    // the watermark — the pass is always incremental.
    loop {
        run_once(conn, args, json_output)?;

        if !args.follow {
            break;
        }

        std::thread::sleep(FOLLOW_POLL_INTERVAL);
    }

    Ok(())
}

fn run_once(conn: &Connection, args: &WatchTranscriptArgs, json_output: bool) -> Result<()> {
    let path_str = args.path.to_string_lossy();

    let offset = watermark::get(conn, &path_str)?
        .map(|w| w.byte_offset)
        .unwrap_or(0);

    let parse_result = jsonl_codex::parse_file(&args.path, offset)?;

    if parse_result.entries.is_empty() {
        // No new content — in --follow mode this is the common case. Stay
        // quiet unless the user asked for JSON.
        if json_output && !args.follow {
            println!(
                "{{\"status\":\"no_new_content\",\"file\":\"{path_str}\",\"offset\":{offset}}}"
            );
        }
        return Ok(());
    }

    let transcript = jsonl_codex::render_transcript(&parse_result.entries);

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

    // Signal gate (Haiku).
    let gate = curator::signal_gate(&transcript)?;
    let project_for_telemetry = resolve_project(args, &parse_result.cwd, &args.path);
    if let Some(t) = &gate.telemetry {
        agent_invocation::try_insert(
            conn,
            &agent_invocation::record_from_subagent(
                Caller::SignalGate,
                Some(project_for_telemetry.clone()),
                None,
                t,
            ),
        );
    }

    if gate.signal == Signal::No {
        watermark::set(
            conn,
            &path_str,
            parse_result.new_offset,
            parse_result.lines_processed as u64,
        )?;
        watermark::log_action(conn, &path_str, "noop", None, "No signal (codex)", "")?;
        if !json_output {
            eprintln!("    → no signal, skipping");
        }
        return Ok(());
    }

    if !json_output {
        eprintln!(
            "    → signal detected, curating for project '{}'...",
            project_for_telemetry
        );
    }

    let result = curator::curate(&transcript, &project_for_telemetry, args.dry_run)?;

    if let Some(t) = &result.telemetry {
        agent_invocation::try_insert(
            conn,
            &agent_invocation::record_from_subagent(
                Caller::Curator,
                Some(project_for_telemetry.clone()),
                None,
                t,
            ),
        );
    }

    watermark::set(
        conn,
        &path_str,
        parse_result.new_offset,
        parse_result.lines_processed as u64,
    )?;

    let action = if args.dry_run { "noop" } else { "add" };
    let reason = result.curator_output.as_deref().unwrap_or("");
    let log_reason = if reason.len() > 500 {
        &reason[..500]
    } else {
        reason
    };
    watermark::log_action(conn, &path_str, action, None, log_reason, "sonnet")?;

    if json_output {
        let output = serde_json::json!({
            "agent": "codex",
            "file": path_str,
            "signal": "yes",
            "dry_run": args.dry_run,
            "entries_processed": parse_result.entries.len(),
            "project": project_for_telemetry,
            "curator_output": result.curator_output,
        });
        println!("{}", serde_json::to_string(&output)?);
    } else if let Some(out) = &result.curator_output {
        println!("{out}");
    }

    Ok(())
}

/// Project resolution order: explicit `--project` → `session_meta.cwd`
/// basename → path-based fallback → `"unknown"`.
fn resolve_project(args: &WatchTranscriptArgs, cwd: &Option<String>, path: &Path) -> String {
    if let Some(p) = args.project.as_deref() {
        return p.to_string();
    }
    if let Some(cwd) = cwd.as_deref() {
        if let Some(basename) = std::path::Path::new(cwd).file_name().and_then(|s| s.to_str()) {
            if !basename.is_empty() {
                return basename.to_string();
            }
        }
    }
    if let Some(basename) = path.file_stem().and_then(|s| s.to_str()) {
        return basename.to_string();
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_prefers_explicit_override() {
        let args = WatchTranscriptArgs {
            agent: "codex".into(),
            path: PathBuf::from("/tmp/session.jsonl"),
            follow: false,
            project: Some("my-app".into()),
            dry_run: false,
        };
        let got = resolve_project(&args, &Some("/Users/me/other".into()), &args.path);
        assert_eq!(got, "my-app");
    }

    #[test]
    fn project_falls_back_to_session_cwd_basename() {
        let args = WatchTranscriptArgs {
            agent: "codex".into(),
            path: PathBuf::from("/tmp/session.jsonl"),
            follow: false,
            project: None,
            dry_run: false,
        };
        let got = resolve_project(&args, &Some("/Users/me/homeserver".into()), &args.path);
        assert_eq!(got, "homeserver");
    }

    #[test]
    fn project_falls_back_to_file_stem_when_no_cwd() {
        let args = WatchTranscriptArgs {
            agent: "codex".into(),
            path: PathBuf::from("/tmp/session.jsonl"),
            follow: false,
            project: None,
            dry_run: false,
        };
        let got = resolve_project(&args, &None, &args.path);
        assert_eq!(got, "session");
    }
}
