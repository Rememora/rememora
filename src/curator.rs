//! Curator: manages subagent invocation for signal gating and AUDN curation.
//!
//! Uses `claude -p` (Claude Code CLI) as the subagent — subscription-included,
//! no per-token API cost.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

const SIGNAL_GATE_PROMPT: &str = include_str!("../prompts/signal-gate.md");
const CURATOR_PROMPT: &str = include_str!("../prompts/curator.md");

/// Minimum transcript length worth gating (chars). Below this, skip entirely.
const MIN_TRANSCRIPT_CHARS: usize = 500;

/// Result of the signal gate check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Yes,
    No,
}

/// Parsed telemetry from `claude -p --output-format json`'s final `result`
/// entry. Any field may be missing if the subagent errored before emitting it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubagentTelemetry {
    pub model: String,
    pub child_session_id: Option<String>,
    pub duration_ms: Option<i64>,
    pub duration_api_ms: Option<i64>,
    pub num_turns: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub stop_reason: Option<String>,
    pub terminal_reason: Option<String>,
    pub is_error: bool,
    pub permission_denials_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubagentOutput {
    /// The `result` string from the final `{type:"result"}` entry — what the
    /// agent actually replied with.
    pub text: String,
    pub telemetry: SubagentTelemetry,
}

/// Signal-gate outcome with telemetry for the Haiku call.
#[derive(Debug)]
pub struct SignalGateResult {
    pub signal: Signal,
    /// `None` when the gate short-circuited (transcript too short) — no
    /// subagent call, nothing to record.
    pub telemetry: Option<SubagentTelemetry>,
}

/// Result of a full curation run.
#[derive(Debug)]
pub struct CurationResult {
    /// Whether signal was detected.
    pub signal: Signal,
    /// Raw output from the curator subagent (if run).
    pub curator_output: Option<String>,
    /// Whether this was a dry run (no actual curation performed).
    pub dry_run: bool,
    /// Telemetry for the Sonnet curator call.
    pub telemetry: Option<SubagentTelemetry>,
}

/// Check whether a transcript has signal worth curating.
///
/// Uses Haiku via subagent for a fast YES/NO classification.
pub fn signal_gate(transcript: &str) -> Result<SignalGateResult> {
    if transcript.len() < MIN_TRANSCRIPT_CHARS {
        return Ok(SignalGateResult {
            signal: Signal::No,
            telemetry: None,
        });
    }

    let prompt = SIGNAL_GATE_PROMPT.replace("{transcript}", transcript);
    let output = call_subagent(&prompt, "haiku")?;

    let answer = output.text.trim().to_uppercase();
    let signal = if answer.contains("YES") {
        Signal::Yes
    } else {
        Signal::No
    };

    Ok(SignalGateResult {
        signal,
        telemetry: Some(output.telemetry),
    })
}

/// Run the full AUDN curation cycle on a transcript.
///
/// Uses Sonnet via subagent — the subagent gets bash access to run
/// `rememora search/save/supersede` commands directly.
pub fn curate(transcript: &str, project: &str, dry_run: bool) -> Result<CurationResult> {
    let prompt = CURATOR_PROMPT
        .replace("{transcript}", transcript)
        .replace("{project}", project);

    let full_prompt = if dry_run {
        format!(
            "DRY RUN MODE: Do NOT execute any rememora commands. \
             Instead, show what commands you WOULD run and why.\n\n{prompt}"
        )
    } else {
        prompt
    };

    let output = call_subagent(&full_prompt, "sonnet")?;

    Ok(CurationResult {
        signal: Signal::Yes,
        curator_output: Some(output.text),
        dry_run,
        telemetry: Some(output.telemetry),
    })
}

/// Call a Claude Code subagent via `claude -p` with a specific model.
///
/// Uses `--tools Bash` to restrict the subagent to only the Bash tool
/// (prevents file writes to auto-memory). Grants scoped bash access via
/// `--allowedTools` so it can run `rememora` CLI commands without prompting.
///
/// Uses `--output-format json` so we can capture usage, cost, and duration
/// telemetry alongside the agent's text reply.
pub fn call_subagent(prompt: &str, model: &str) -> Result<SubagentOutput> {
    let output = build_subagent_command(prompt, model)
        .output()
        .context("Failed to run 'claude' CLI. Is Claude Code installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("claude -p failed (exit {}): {stderr}", output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_subagent_output(&stdout, model)
}

fn build_subagent_command(prompt: &str, model: &str) -> Command {
    let mut cmd = Command::new("claude");
    cmd.args([
        "-p",
        prompt,
        "--model",
        model,
        "--output-format",
        "json",
        "--tools",
        "Bash",
        "--allowedTools",
        "Bash(rememora:*)",
    ])
    // Mark curate-spawned Claude Code children so their Stop hooks do not
    // recursively curate the child session JSONL.
    .env("REMEMORA_CURATE_CHILD", "1");
    cmd
}

/// Parse `claude -p --output-format json` stdout.
///
/// Output is a JSON array of event objects; we walk it to find the final
/// `{type:"result"}` entry which carries `result`, `usage`, `total_cost_usd`,
/// `duration_ms`, `stop_reason`, `terminal_reason`, `permission_denials`.
pub fn parse_subagent_output(stdout: &str, model: &str) -> Result<SubagentOutput> {
    let events: serde_json::Value =
        serde_json::from_str(stdout.trim()).context("claude -p returned non-JSON output")?;

    let array = events
        .as_array()
        .context("claude -p JSON was not an array")?;

    let result_entry = array
        .iter()
        .rev()
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("result"))
        .context("claude -p JSON stream had no {type:\"result\"} entry")?;

    let text = result_entry
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let usage = result_entry.get("usage");
    let telemetry = SubagentTelemetry {
        model: result_entry
            .get("modelUsage")
            .and_then(|mu| mu.as_object())
            .and_then(|obj| obj.keys().next().cloned())
            .unwrap_or_else(|| model.to_string()),
        child_session_id: result_entry
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        duration_ms: result_entry.get("duration_ms").and_then(|v| v.as_i64()),
        duration_api_ms: result_entry.get("duration_api_ms").and_then(|v| v.as_i64()),
        num_turns: result_entry.get("num_turns").and_then(|v| v.as_i64()),
        input_tokens: usage.and_then(|u| u.get("input_tokens")).and_then(|v| v.as_i64()),
        output_tokens: usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_i64()),
        cache_read_tokens: usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|v| v.as_i64()),
        cache_creation_tokens: usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|v| v.as_i64()),
        cost_usd: result_entry
            .get("total_cost_usd")
            .and_then(|v| v.as_f64()),
        stop_reason: result_entry
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        terminal_reason: result_entry
            .get("terminal_reason")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        is_error: result_entry
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        permission_denials_json: result_entry
            .get("permission_denials")
            .map(|v| v.to_string())
            .filter(|s| s != "[]" && s != "null"),
    };

    Ok(SubagentOutput { text, telemetry })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_gate_short_transcript() {
        // Transcripts below MIN_TRANSCRIPT_CHARS should return No without calling subagent
        let short = "user: hello\nassistant: hi";
        let result = signal_gate(short).unwrap();
        assert_eq!(result.signal, Signal::No);
        assert!(result.telemetry.is_none());
    }

    #[test]
    fn test_prompt_template_substitution() {
        let transcript = "test transcript content";
        let project = "my-project";

        let prompt = CURATOR_PROMPT
            .replace("{transcript}", transcript)
            .replace("{project}", project);

        assert!(prompt.contains("test transcript content"));
        assert!(prompt.contains("my-project"));
        assert!(!prompt.contains("{transcript}"));
        assert!(!prompt.contains("{project}"));
    }

    #[test]
    fn test_signal_gate_prompt_substitution() {
        let transcript = "some transcript";
        let prompt = SIGNAL_GATE_PROMPT.replace("{transcript}", transcript);
        assert!(prompt.contains("some transcript"));
        assert!(!prompt.contains("{transcript}"));
    }

    #[test]
    fn test_subagent_command_marks_curate_child() {
        let cmd = build_subagent_command("test prompt", "haiku");
        let env_value = cmd
            .get_envs()
            .find_map(|(key, value)| (key == "REMEMORA_CURATE_CHILD").then_some(value));

        assert_eq!(env_value.flatten(), Some(std::ffi::OsStr::new("1")));
    }

    #[test]
    fn test_subagent_command_requests_json_output() {
        let cmd = build_subagent_command("p", "haiku");
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        // Args appear pairwise: ["--output-format","json"]
        let pos = args
            .iter()
            .position(|a| *a == std::ffi::OsStr::new("--output-format"));
        assert!(pos.is_some(), "--output-format flag missing");
        assert_eq!(args[pos.unwrap() + 1], std::ffi::OsStr::new("json"));
    }

    #[test]
    fn test_parse_subagent_output_full() {
        let stdout = r#"[
            {"type":"system","subtype":"init","session_id":"s1"},
            {"type":"result","subtype":"success","is_error":false,"duration_ms":3208,
             "duration_api_ms":2693,"num_turns":1,"result":"Hey there, friend!",
             "stop_reason":"end_turn","session_id":"s1","total_cost_usd":0.0135815,
             "usage":{"input_tokens":10,"cache_creation_input_tokens":6366,
                      "cache_read_input_tokens":51240,"output_tokens":98},
             "modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":10,"outputTokens":98}},
             "permission_denials":[],"terminal_reason":"completed"}
        ]"#;
        let out = parse_subagent_output(stdout, "haiku").unwrap();
        assert_eq!(out.text, "Hey there, friend!");
        let t = out.telemetry;
        assert_eq!(t.model, "claude-haiku-4-5-20251001");
        assert_eq!(t.child_session_id.as_deref(), Some("s1"));
        assert_eq!(t.duration_ms, Some(3208));
        assert_eq!(t.input_tokens, Some(10));
        assert_eq!(t.output_tokens, Some(98));
        assert_eq!(t.cache_read_tokens, Some(51240));
        assert_eq!(t.cache_creation_tokens, Some(6366));
        assert_eq!(t.cost_usd, Some(0.0135815));
        assert_eq!(t.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(t.terminal_reason.as_deref(), Some("completed"));
        assert!(!t.is_error);
        assert!(t.permission_denials_json.is_none()); // filtered out "[]"
    }

    #[test]
    fn test_parse_subagent_output_missing_fields() {
        // A minimal result entry — parser must not panic, just leave Nones.
        let stdout = r#"[{"type":"result","result":"ok"}]"#;
        let out = parse_subagent_output(stdout, "sonnet").unwrap();
        assert_eq!(out.text, "ok");
        assert_eq!(out.telemetry.model, "sonnet"); // falls back to the model we asked for
        assert!(out.telemetry.input_tokens.is_none());
        assert!(out.telemetry.cost_usd.is_none());
    }

    #[test]
    fn test_parse_subagent_output_records_permission_denials() {
        let stdout = r#"[{"type":"result","result":"","permission_denials":[{"tool":"Bash","input":"rm -rf /"}]}]"#;
        let out = parse_subagent_output(stdout, "haiku").unwrap();
        assert!(out.telemetry.permission_denials_json.is_some());
        assert!(out
            .telemetry
            .permission_denials_json
            .unwrap()
            .contains("rm -rf"));
    }

    #[test]
    fn test_parse_subagent_output_error_bubbles_on_missing_result() {
        let stdout = r#"[{"type":"system","subtype":"init"}]"#;
        let err = parse_subagent_output(stdout, "haiku").unwrap_err();
        assert!(err.to_string().contains("no {type:\"result\"}"));
    }

    #[test]
    fn test_parse_subagent_output_picks_last_result_if_multiple() {
        let stdout = r#"[
            {"type":"result","result":"first"},
            {"type":"result","result":"last"}
        ]"#;
        let out = parse_subagent_output(stdout, "haiku").unwrap();
        assert_eq!(out.text, "last");
    }
}
