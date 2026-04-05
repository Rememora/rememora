//! Curator: manages subagent invocation for signal gating and AUDN curation.
//!
//! Uses `claude -p` (Claude Code CLI) as the subagent — subscription-included,
//! no per-token API cost.

use anyhow::{bail, Context, Result};
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

/// Result of a full curation run.
#[derive(Debug)]
pub struct CurationResult {
    /// Whether signal was detected.
    pub signal: Signal,
    /// Raw output from the curator subagent (if run).
    pub curator_output: Option<String>,
    /// Whether this was a dry run (no actual curation performed).
    pub dry_run: bool,
}

/// Check whether a transcript has signal worth curating.
///
/// Uses Haiku via subagent for a fast YES/NO classification.
pub fn signal_gate(transcript: &str) -> Result<Signal> {
    if transcript.len() < MIN_TRANSCRIPT_CHARS {
        return Ok(Signal::No);
    }

    let prompt = SIGNAL_GATE_PROMPT.replace("{transcript}", transcript);
    let output = call_subagent(&prompt, "haiku")?;

    let answer = output.trim().to_uppercase();
    if answer.contains("YES") {
        Ok(Signal::Yes)
    } else {
        Ok(Signal::No)
    }
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
        curator_output: Some(output),
        dry_run,
    })
}

/// Call a Claude Code subagent via `claude -p` with a specific model.
pub fn call_subagent(prompt: &str, model: &str) -> Result<String> {
    let output = Command::new("claude")
        .args(["-p", prompt, "--model", model])
        .output()
        .context("Failed to run 'claude' CLI. Is Claude Code installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("claude -p failed (exit {}): {stderr}", output.status);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_gate_short_transcript() {
        // Transcripts below MIN_TRANSCRIPT_CHARS should return No without calling subagent
        let short = "user: hello\nassistant: hi";
        let result = signal_gate(short).unwrap();
        assert_eq!(result, Signal::No);
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
}
