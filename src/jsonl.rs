//! Parser for Claude Code session JSONL files.
//!
//! Extracts the conversation transcript (user messages, assistant text,
//! tool calls) from a session JSONL, starting at a byte offset so curation
//! can be incremental.  Output is capped at ~32 KB to fit within a
//! subagent's context window.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Maximum condensed transcript size in bytes (~32 KB).
const MAX_TRANSCRIPT_BYTES: usize = 32_768;

/// Maximum length for a single tool input/output preview.
const TOOL_PREVIEW_LEN: usize = 200;

// ── Raw JSONL structures (only fields we need) ──────────────────────

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    message: Option<RawMessage>,
    #[serde(default)]
    content: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(default)]
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

// ── Public types ────────────────────────────────────────────────────

/// A condensed entry from the session transcript.
#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub role: String,
    pub text: String,
}

/// Result of parsing a JSONL file from a given offset.
#[derive(Debug)]
pub struct ParseResult {
    /// Condensed transcript entries.
    pub entries: Vec<TranscriptEntry>,
    /// New byte offset (end of file after reading).
    pub new_offset: u64,
    /// Number of raw JSONL lines processed.
    pub lines_processed: usize,
    /// Whether the transcript was truncated to fit within the cap.
    pub truncated: bool,
}

// ── Public API ──────────────────────────────────────────────────────

/// Parse a Claude Code session JSONL file starting at `byte_offset`.
///
/// Returns a condensed transcript capped at [`MAX_TRANSCRIPT_BYTES`].
pub fn parse_file(path: &Path, byte_offset: u64) -> Result<ParseResult> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open JSONL: {}", path.display()))?;

    let file_len = file.metadata()?.len();
    if byte_offset >= file_len {
        return Ok(ParseResult {
            entries: vec![],
            new_offset: file_len,
            lines_processed: 0,
            truncated: false,
        });
    }

    file.seek(SeekFrom::Start(byte_offset))?;
    let mut result = parse_reader(file)?;

    // Adjust offset: if not truncated, advance to end of file.
    // If truncated, advance by the bytes we actually consumed.
    if !result.truncated {
        result.new_offset += byte_offset;
    } else {
        result.new_offset += byte_offset;
    }

    Ok(result)
}

/// Parse JSONL from any reader (useful for testing with in-memory data).
pub fn parse_reader<R: Read>(reader: R) -> Result<ParseResult> {
    let buf = BufReader::new(reader);
    let mut entries = Vec::new();
    let mut total_bytes: usize = 0;
    let mut lines_processed: usize = 0;
    let mut bytes_consumed: u64 = 0;
    let mut truncated = false;

    for line_result in buf.lines() {
        let line = line_result?;
        // +1 for the newline that BufRead::lines() strips
        bytes_consumed += line.len() as u64 + 1;
        lines_processed += 1;

        if line.trim().is_empty() {
            continue;
        }

        let raw: RawLine = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue, // skip malformed lines
        };

        let entry = match condense_line(&raw) {
            Some(e) => e,
            None => continue,
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
        new_offset: bytes_consumed,
        lines_processed,
        truncated,
    })
}

/// Render the transcript entries into a single string for the curator prompt.
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

fn condense_line(raw: &RawLine) -> Option<TranscriptEntry> {
    match raw.msg_type.as_str() {
        "user" => condense_user(raw),
        "assistant" => condense_assistant(raw),
        "system" => condense_system(raw),
        // Skip: progress, file-history-snapshot, last-prompt,
        //        queue-operation, pr-link, custom-title, agent-name, attachment
        _ => None,
    }
}

fn condense_user(raw: &RawLine) -> Option<TranscriptEntry> {
    let msg = raw.message.as_ref()?;
    let text = extract_text_from_content(&msg.content);
    if text.is_empty() {
        return None;
    }
    Some(TranscriptEntry {
        role: "user".into(),
        text,
    })
}

fn condense_assistant(raw: &RawLine) -> Option<TranscriptEntry> {
    let msg = raw.message.as_ref()?;
    let content = msg.content.as_array()?;

    let mut parts = Vec::new();

    for block_val in content {
        let block: ContentBlock = match serde_json::from_value(block_val.clone()) {
            Ok(b) => b,
            Err(_) => continue,
        };

        match block.block_type.as_str() {
            "text" => {
                if let Some(t) = &block.text {
                    if !t.is_empty() {
                        parts.push(t.clone());
                    }
                }
            }
            "tool_use" => {
                let name = block.name.as_deref().unwrap_or("unknown_tool");
                let input_preview = block
                    .input
                    .as_ref()
                    .map(|v| truncate_str(&v.to_string(), TOOL_PREVIEW_LEN))
                    .unwrap_or_default();
                parts.push(format!("[tool: {name}({input_preview})]"));
            }
            // Skip thinking blocks — they contain no actionable signal
            _ => {}
        }
    }

    if parts.is_empty() {
        return None;
    }

    Some(TranscriptEntry {
        role: "assistant".into(),
        text: parts.join("\n"),
    })
}

fn condense_system(raw: &RawLine) -> Option<TranscriptEntry> {
    let subtype = raw.subtype.as_deref()?;
    match subtype {
        "stop_hook_summary" => {
            let text = raw
                .content
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return None;
            }
            Some(TranscriptEntry {
                role: "system".into(),
                text,
            })
        }
        _ => None,
    }
}

/// Extract text from a message content field (string or array of blocks).
fn extract_text_from_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => {
            // Strip XML system tags — they're just noise for curation
            strip_system_tags(s)
        }
        serde_json::Value::Array(blocks) => {
            let mut parts = Vec::new();
            for block in blocks {
                if let Some(t) = block.get("type").and_then(|v| v.as_str()) {
                    match t {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                let cleaned = strip_system_tags(text);
                                if !cleaned.is_empty() {
                                    parts.push(cleaned);
                                }
                            }
                        }
                        // tool_result blocks — skip the content, just note the tool call
                        "tool_result" => {
                            if let Some(tool_id) =
                                block.get("tool_use_id").and_then(|v| v.as_str())
                            {
                                parts.push(format!("[tool_result: {tool_id}]"));
                            }
                        }
                        _ => {}
                    }
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// Remove XML-style system tags that add no signal for curation.
fn strip_system_tags(s: &str) -> String {
    // Remove <system-reminder>...</system-reminder>, <local-command-caveat>...</local-command-caveat>, etc.
    let mut result = s.to_string();
    for tag in &[
        "system-reminder",
        "local-command-caveat",
        "local-command-stdout",
        "command-name",
        "command-message",
        "command-args",
        "available-deferred-tools",
    ] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        // Remove everything between open and close tags (including the tags)
        while let Some(start) = result.find(&open) {
            if let Some(end) = result.find(&close) {
                let end = end + close.len();
                result.replace_range(start..end, "");
            } else {
                // Unclosed tag — remove from open tag to end
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

    fn make_user_line(content: &str) -> String {
        serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": content},
            "timestamp": "2026-01-01T00:00:00Z",
            "uuid": "test-1"
        })
        .to_string()
    }

    fn make_assistant_line(text: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": text}
                ]
            },
            "timestamp": "2026-01-01T00:00:01Z",
            "uuid": "test-2"
        })
        .to_string()
    }

    fn make_assistant_with_tool(text: &str, tool_name: &str, tool_input: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": text},
                    {"type": "tool_use", "name": tool_name, "input": {"command": tool_input}, "id": "toolu_123"}
                ]
            },
            "timestamp": "2026-01-01T00:00:01Z",
            "uuid": "test-3"
        })
        .to_string()
    }

    fn make_thinking_only_line() -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "let me think about this..."}
                ]
            },
            "timestamp": "2026-01-01T00:00:01Z",
            "uuid": "test-4"
        })
        .to_string()
    }

    fn make_progress_line() -> String {
        serde_json::json!({
            "type": "progress",
            "data": "streaming..."
        })
        .to_string()
    }

    fn make_snapshot_line() -> String {
        serde_json::json!({
            "type": "file-history-snapshot",
            "snapshot": {"trackedFileBackups": {}}
        })
        .to_string()
    }

    fn make_system_stop_hook(summary: &str) -> String {
        serde_json::json!({
            "type": "system",
            "subtype": "stop_hook_summary",
            "content": summary,
            "timestamp": "2026-01-01T00:01:00Z"
        })
        .to_string()
    }

    fn parse_lines(lines: &[String]) -> ParseResult {
        let data = lines.join("\n");
        let cursor = Cursor::new(data.into_bytes());
        parse_reader(cursor).unwrap()
    }

    #[test]
    fn test_basic_conversation() {
        let lines = vec![
            make_user_line("Fix the login bug"),
            make_assistant_line("I'll fix the login bug by updating auth.rs"),
        ];
        let result = parse_lines(&lines);
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].role, "user");
        assert_eq!(result.entries[0].text, "Fix the login bug");
        assert_eq!(result.entries[1].role, "assistant");
        assert!(result.entries[1].text.contains("auth.rs"));
        assert_eq!(result.lines_processed, 2);
        assert!(!result.truncated);
    }

    #[test]
    fn test_tool_use_condensed() {
        let lines = vec![make_assistant_with_tool(
            "Let me check the file.",
            "Read",
            "src/auth.rs",
        )];
        let result = parse_lines(&lines);
        assert_eq!(result.entries.len(), 1);
        assert!(result.entries[0].text.contains("[tool: Read("));
        assert!(result.entries[0].text.contains("src/auth.rs"));
    }

    #[test]
    fn test_thinking_only_skipped() {
        let lines = vec![make_thinking_only_line()];
        let result = parse_lines(&lines);
        assert_eq!(result.entries.len(), 0);
    }

    #[test]
    fn test_noise_skipped() {
        let lines = vec![
            make_progress_line(),
            make_snapshot_line(),
            make_user_line("hello"),
            make_progress_line(),
        ];
        let result = parse_lines(&lines);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].text, "hello");
        assert_eq!(result.lines_processed, 4);
    }

    #[test]
    fn test_system_stop_hook() {
        let lines = vec![make_system_stop_hook("Session completed successfully")];
        let result = parse_lines(&lines);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].role, "system");
        assert!(result.entries[0].text.contains("Session completed"));
    }

    #[test]
    fn test_system_non_stop_hook_skipped() {
        let lines = vec![serde_json::json!({
            "type": "system",
            "subtype": "turn_duration",
            "content": "42",
            "timestamp": "2026-01-01T00:00:00Z"
        })
        .to_string()];
        let result = parse_lines(&lines);
        assert_eq!(result.entries.len(), 0);
    }

    #[test]
    fn test_strip_system_tags() {
        let input = "hello <system-reminder>some noise</system-reminder> world";
        assert_eq!(strip_system_tags(input), "hello  world");
    }

    #[test]
    fn test_strip_system_tags_nested() {
        let input = "<local-command-caveat>caveat text</local-command-caveat>actual message";
        assert_eq!(strip_system_tags(input), "actual message");
    }

    #[test]
    fn test_truncation() {
        // Create entries that exceed MAX_TRANSCRIPT_BYTES
        let big_text = "x".repeat(20_000);
        let lines = vec![
            make_user_line(&big_text),
            make_user_line(&big_text),
            make_user_line("this should be dropped"),
        ];
        let result = parse_lines(&lines);
        // First fits, second may or may not, third should not
        assert!(result.truncated);
        assert!(result.entries.len() < 3);
    }

    #[test]
    fn test_empty_input() {
        let result = parse_lines(&[]);
        assert_eq!(result.entries.len(), 0);
        assert_eq!(result.lines_processed, 0);
        assert!(!result.truncated);
    }

    #[test]
    fn test_malformed_json_skipped() {
        let lines = vec![
            "not valid json".to_string(),
            make_user_line("valid message"),
        ];
        let result = parse_lines(&lines);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].text, "valid message");
    }

    #[test]
    fn test_render_transcript() {
        let entries = vec![
            TranscriptEntry {
                role: "user".into(),
                text: "Fix the bug".into(),
            },
            TranscriptEntry {
                role: "assistant".into(),
                text: "Done".into(),
            },
        ];
        let rendered = render_transcript(&entries);
        assert_eq!(rendered, "user: Fix the bug\n\nassistant: Done\n\n");
    }

    #[test]
    fn test_file_parse_with_offset() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();

        let line1 = make_user_line("first message");
        let line2 = make_user_line("second message");
        writeln!(f, "{}", line1).unwrap();
        let offset = f.metadata().unwrap().len();
        writeln!(f, "{}", line2).unwrap();
        drop(f);

        // Parse from offset — should only get second message
        let result = parse_file(&path, offset).unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].text, "second message");
    }

    #[test]
    fn test_file_offset_past_end() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{}", make_user_line("msg")).unwrap();
        drop(f);

        let result = parse_file(&path, 999999).unwrap();
        assert_eq!(result.entries.len(), 0);
        assert_eq!(result.lines_processed, 0);
    }

    #[test]
    fn test_user_with_tool_result_content() {
        let line = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_abc", "content": [{"type": "text", "text": "file contents..."}]},
                    {"type": "text", "text": "Now fix the bug"}
                ]
            },
            "timestamp": "2026-01-01T00:00:00Z"
        })
        .to_string();
        let result = parse_lines(&[line]);
        assert_eq!(result.entries.len(), 1);
        assert!(result.entries[0].text.contains("tool_result"));
        assert!(result.entries[0].text.contains("Now fix the bug"));
    }
}
