//! Parser for Codex CLI session JSONL rollout files.
//!
//! Codex sessions live at `~/.codex/sessions/YYYY/MM/DD/rollout-<id>.jsonl`.
//! Each line is `{timestamp, type, payload}`. We extract only the conversation
//! shape the curator needs: user/assistant messages, tool calls, tool outputs.
//!
//! Everything else — `reasoning` (private thinking), `event_msg` (mirrors
//! response_item; would double-count), `turn_context`, `compacted` — is
//! skipped. `session_meta` is mined only for `cwd` → project name.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Maximum condensed transcript size in bytes (~32 KB) — matches `jsonl.rs`.
const MAX_TRANSCRIPT_BYTES: usize = 32_768;

/// Maximum preview length for a single function call argument blob.
const TOOL_PREVIEW_LEN: usize = 200;

// ── Raw JSONL line shapes (payload is type-tagged) ──────────────────

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    line_type: String,
    #[serde(default)]
    payload: serde_json::Value,
}

// ── Public types ────────────────────────────────────────────────────

/// A condensed transcript entry from a Codex rollout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub role: String,
    pub text: String,
}

/// Result of parsing a Codex rollout from a given byte offset.
#[derive(Debug)]
pub struct ParseResult {
    pub entries: Vec<TranscriptEntry>,
    /// Working directory captured from `session_meta` (if present), used as a
    /// project-name fallback when the caller doesn't pass `--project`.
    pub cwd: Option<String>,
    /// New byte offset (end of file after reading).
    pub new_offset: u64,
    /// Number of raw JSONL lines processed.
    pub lines_processed: usize,
    /// True if the transcript was truncated to fit the byte cap.
    pub truncated: bool,
}

// ── Public API ──────────────────────────────────────────────────────

/// Parse a Codex rollout JSONL file starting at `byte_offset`.
pub fn parse_file(path: &Path, byte_offset: u64) -> Result<ParseResult> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open Codex JSONL: {}", path.display()))?;

    let file_len = file.metadata()?.len();
    if byte_offset >= file_len {
        return Ok(ParseResult {
            entries: vec![],
            cwd: None,
            new_offset: file_len,
            lines_processed: 0,
            truncated: false,
        });
    }

    file.seek(SeekFrom::Start(byte_offset))?;
    let mut result = parse_reader(file)?;
    result.new_offset += byte_offset;
    Ok(result)
}

/// Parse Codex JSONL from any reader (useful for tests).
pub fn parse_reader<R: Read>(reader: R) -> Result<ParseResult> {
    let buf = BufReader::new(reader);
    let mut entries: Vec<TranscriptEntry> = Vec::new();
    let mut cwd: Option<String> = None;
    let mut total_bytes: usize = 0;
    let mut lines_processed: usize = 0;
    let mut bytes_consumed: u64 = 0;
    let mut truncated = false;

    for line_result in buf.lines() {
        let line = line_result?;
        bytes_consumed += line.len() as u64 + 1; // +1 for the stripped newline
        lines_processed += 1;

        if line.trim().is_empty() {
            continue;
        }

        let raw: RawLine = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue, // skip malformed lines — non-fatal
        };

        // session_meta: pull cwd once.
        if raw.line_type == "session_meta" && cwd.is_none() {
            cwd = raw
                .payload
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            continue;
        }

        // response_item is the only carrier of conversation signal. Everything
        // else (event_msg, turn_context, reasoning, compacted) is deliberately
        // skipped — event_msg in particular would double-count messages.
        if raw.line_type != "response_item" {
            continue;
        }

        let Some(entry) = condense_response_item(&raw.payload) else {
            continue;
        };

        let entry_len = entry.role.len() + entry.text.len() + 4; // ": \n\n"
        if total_bytes + entry_len > MAX_TRANSCRIPT_BYTES {
            truncated = true;
            break;
        }

        total_bytes += entry_len;
        entries.push(entry);
    }

    Ok(ParseResult {
        entries,
        cwd,
        new_offset: bytes_consumed,
        lines_processed,
        truncated,
    })
}

/// Render the transcript entries into the curator prompt string.
pub fn render_transcript(entries: &[TranscriptEntry]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&entry.role);
        out.push_str(": ");
        out.push_str(&entry.text);
        out.push_str("\n\n");
    }
    out
}

// ── Internals ───────────────────────────────────────────────────────

fn condense_response_item(payload: &serde_json::Value) -> Option<TranscriptEntry> {
    let obj = payload.as_object()?;
    let item_type = obj.get("type")?.as_str()?;

    match item_type {
        "message" => condense_message(obj),
        "function_call" => condense_function_call(obj),
        "function_call_output" => condense_function_call_output(obj),
        // Skip: reasoning (private thinking), compacted (summarization marker),
        // custom_instructions, everything else.
        _ => None,
    }
}

fn condense_message(obj: &serde_json::Map<String, serde_json::Value>) -> Option<TranscriptEntry> {
    let role = obj.get("role")?.as_str()?.to_string();
    let content = obj.get("content")?.as_array()?;

    let mut parts: Vec<String> = Vec::new();
    for c in content {
        let ct = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        // Codex uses input_text for user messages, output_text for assistant;
        // some variants emit plain "text".
        if matches!(ct, "input_text" | "output_text" | "text") {
            if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                let cleaned = strip_environment_context(t);
                if !cleaned.is_empty() {
                    parts.push(cleaned);
                }
            }
        }
    }

    if parts.is_empty() {
        return None;
    }

    Some(TranscriptEntry {
        role,
        text: parts.join("\n"),
    })
}

fn condense_function_call(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<TranscriptEntry> {
    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
    let args = obj
        .get("arguments")
        .and_then(|v| v.as_str())
        .map(|s| truncate_str(s, TOOL_PREVIEW_LEN))
        .unwrap_or_default();

    Some(TranscriptEntry {
        role: "assistant".to_string(),
        text: format!("[tool: {name}({args})]"),
    })
}

fn condense_function_call_output(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<TranscriptEntry> {
    let call_id = obj
        .get("call_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    Some(TranscriptEntry {
        role: "user".to_string(),
        text: format!("[tool_result: {call_id}]"),
    })
}

/// Codex prefixes user turns with `<environment_context>...</environment_context>`
/// blocks that add no signal for curation. Strip them the same way the Claude
/// Code parser strips `<system-reminder>` blocks.
fn strip_environment_context(s: &str) -> String {
    let mut result = s.to_string();
    for tag in &["environment_context", "user_instructions"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let Some(start) = result.find(&open) {
            if let Some(end) = result.find(&close) {
                let end = end + close.len();
                result.replace_range(start..end, "");
            } else {
                result.truncate(start);
                break;
            }
        }
    }
    result.trim().to_string()
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse_lines(lines: &[&str]) -> ParseResult {
        let data = lines.join("\n");
        parse_reader(Cursor::new(data.into_bytes())).unwrap()
    }

    #[test]
    fn session_meta_yields_cwd() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"s1","cwd":"/Users/me/proj","originator":"codex_cli_rs","cli_version":"0.42.0"}}"#;
        let r = parse_lines(&[line]);
        assert_eq!(r.cwd.as_deref(), Some("/Users/me/proj"));
        assert!(r.entries.is_empty());
    }

    #[test]
    fn user_message_is_captured() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#;
        let r = parse_lines(&[line]);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].role, "user");
        assert_eq!(r.entries[0].text, "hello");
    }

    #[test]
    fn assistant_output_text_is_captured() {
        let line = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"world"}]}}"#;
        let r = parse_lines(&[line]);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].role, "assistant");
        assert_eq!(r.entries[0].text, "world");
    }

    #[test]
    fn reasoning_is_skipped() {
        let line = r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"reasoning","summary":"thinking","content":"x"}}"#;
        let r = parse_lines(&[line]);
        assert_eq!(r.entries.len(), 0);
    }

    #[test]
    fn event_msg_is_skipped_to_avoid_double_count() {
        // event_msg mirrors response_item and would double-count messages.
        let msg = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}"#;
        let dup = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"event_msg","payload":{"type":"user_turn","text":"hi"}}"#;
        let r = parse_lines(&[msg, dup]);
        assert_eq!(r.entries.len(), 1);
    }

    #[test]
    fn function_call_condenses_to_tool_marker() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":\"ls\"}","call_id":"c1"}}"#;
        let r = parse_lines(&[line]);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].role, "assistant");
        assert!(r.entries[0].text.starts_with("[tool: shell("));
        assert!(r.entries[0].text.contains("ls"));
    }

    #[test]
    fn function_call_output_condenses_to_tool_result_marker() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"done"}}"#;
        let r = parse_lines(&[line]);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].role, "user");
        assert!(r.entries[0].text.starts_with("[tool_result: c1"));
    }

    #[test]
    fn environment_context_tags_are_stripped() {
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/tmp</cwd>\n</environment_context>\nactual prompt"}]}}"#;
        let r = parse_lines(&[line]);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].text, "actual prompt");
    }

    #[test]
    fn turn_context_and_compacted_are_skipped() {
        let tc = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"turn_context","payload":{"tokens":42}}"#;
        let cp = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"compacted","payload":{"summary":"x"}}"#;
        let r = parse_lines(&[tc, cp]);
        assert_eq!(r.entries.len(), 0);
    }

    #[test]
    fn malformed_lines_are_skipped_without_bailing() {
        let good = r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"ok"}]}}"#;
        let r = parse_lines(&["not json", good, ""]);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].text, "ok");
    }

    #[test]
    fn truncation_caps_total_bytes() {
        let big = "x".repeat(20_000);
        let line = format!(
            r#"{{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{big}"}}]}}}}"#
        );
        let r = parse_lines(&[&line, &line, &line]);
        assert!(r.truncated);
        assert!(r.entries.len() < 3);
    }

    #[test]
    fn render_transcript_matches_claude_format() {
        let entries = vec![
            TranscriptEntry {
                role: "user".into(),
                text: "hi".into(),
            },
            TranscriptEntry {
                role: "assistant".into(),
                text: "hello".into(),
            },
        ];
        assert_eq!(render_transcript(&entries), "user: hi\n\nassistant: hello\n\n");
    }

    #[test]
    fn file_parse_with_offset() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codex.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"first"}}]}}}}"#
        )
        .unwrap();
        let offset = f.metadata().unwrap().len();
        writeln!(
            f,
            r#"{{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"second"}}]}}}}"#
        )
        .unwrap();
        drop(f);

        let r = parse_file(&path, offset).unwrap();
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].text, "second");
    }
}
