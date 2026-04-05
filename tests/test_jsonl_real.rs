//! Integration tests: run the JSONL parser against real Claude Code session files.
//! These tests are ignored by default since they depend on local session files.
//! Run with: cargo test --test test_jsonl_real -- --ignored

use std::path::PathBuf;

fn find_session_files() -> Vec<PathBuf> {
    let home = dirs::home_dir().expect("no home dir");
    let claude_dir = home.join(".claude").join("projects");

    if !claude_dir.exists() {
        return vec![];
    }

    let mut files = Vec::new();
    for project_entry in std::fs::read_dir(&claude_dir).unwrap() {
        let project_entry = project_entry.unwrap();
        if !project_entry.path().is_dir() {
            continue;
        }
        for file_entry in std::fs::read_dir(project_entry.path()).unwrap() {
            let file_entry = file_entry.unwrap();
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }

    // Sort by size descending to test biggest first
    files.sort_by(|a, b| {
        let a_size = a.metadata().map(|m| m.len()).unwrap_or(0);
        let b_size = b.metadata().map(|m| m.len()).unwrap_or(0);
        b_size.cmp(&a_size)
    });

    files
}

#[test]
#[ignore]
fn test_parse_real_sessions() {
    let files = find_session_files();
    assert!(!files.is_empty(), "No session files found — is Claude Code installed?");

    println!("\nFound {} session files\n", files.len());

    let mut total_entries = 0;
    let mut total_truncated = 0;
    let mut parse_errors = 0;

    // Test up to 10 files
    for path in files.iter().take(10) {
        let file_size = path.metadata().map(|m| m.len()).unwrap_or(0);
        print!("  {} ({:.1} KB) ... ", path.file_name().unwrap().to_string_lossy(), file_size as f64 / 1024.0);

        match rememora::jsonl::parse_file(path, 0) {
            Ok(result) => {
                println!(
                    "{} entries, {} lines, {:.1} KB transcript{}",
                    result.entries.len(),
                    result.lines_processed,
                    rememora::jsonl::render_transcript(&result.entries).len() as f64 / 1024.0,
                    if result.truncated { " [TRUNCATED]" } else { "" },
                );

                total_entries += result.entries.len();
                if result.truncated {
                    total_truncated += 1;
                }

                // Verify basic invariants
                assert!(result.new_offset > 0, "new_offset should be > 0 for non-empty file");
                assert!(result.lines_processed > 0, "should process at least one line");

                // Verify no empty entries
                for entry in &result.entries {
                    assert!(!entry.role.is_empty(), "entry role should not be empty");
                    assert!(!entry.text.is_empty(), "entry text should not be empty");
                    assert!(
                        entry.role == "user" || entry.role == "assistant" || entry.role == "system",
                        "unexpected role: {}",
                        entry.role
                    );
                }

                // Verify transcript fits within cap
                let transcript = rememora::jsonl::render_transcript(&result.entries);
                assert!(
                    transcript.len() <= 40_000, // some slack above 32KB
                    "transcript too large: {} bytes",
                    transcript.len()
                );

                // Test offset-based parsing — verify it doesn't crash.
                // When the first parse truncated, new_offset < file_len, and
                // a second parse from new_offset will pick up remaining content.
                let mid = result.new_offset / 2;
                let partial = rememora::jsonl::parse_file(path, mid).unwrap();
                // Just verify it produces valid output — entry/line counts
                // depend on truncation behavior and aren't easily comparable
                for entry in &partial.entries {
                    assert!(!entry.role.is_empty());
                    assert!(!entry.text.is_empty());
                }

                // Test past-end offset (use actual file length, not new_offset which may be truncation point)
                let past_end = rememora::jsonl::parse_file(path, file_size + 1000).unwrap();
                assert_eq!(past_end.entries.len(), 0, "past-end should return no entries");
            }
            Err(e) => {
                println!("ERROR: {e}");
                parse_errors += 1;
            }
        }
    }

    println!("\n=== Summary ===");
    println!("  Total entries extracted: {total_entries}");
    println!("  Files truncated:        {total_truncated}");
    println!("  Parse errors:           {parse_errors}");

    assert_eq!(parse_errors, 0, "No parse errors should occur on real files");
}

#[test]
#[ignore]
fn test_transcript_quality() {
    let files = find_session_files();
    if files.is_empty() {
        return;
    }

    // Pick the largest file
    let path = &files[0];
    let result = rememora::jsonl::parse_file(path, 0).unwrap();

    if result.entries.is_empty() {
        return;
    }

    let transcript = rememora::jsonl::render_transcript(&result.entries);

    // Quality checks
    // 1. Should not contain system-reminder tags
    assert!(
        !transcript.contains("<system-reminder>"),
        "transcript should not contain <system-reminder> tags"
    );

    // 2. Should not contain available-deferred-tools
    assert!(
        !transcript.contains("<available-deferred-tools>"),
        "transcript should not contain deferred tools"
    );

    // 3. Should not contain local-command-caveat
    assert!(
        !transcript.contains("<local-command-caveat>"),
        "transcript should not contain local-command-caveat"
    );

    // 4. Tool uses should be condensed (not full JSON blobs)
    if transcript.contains("[tool:") {
        // Verify tool references are condensed
        let tool_refs: Vec<&str> = transcript.matches("[tool:").collect();
        println!("  Found {} tool references (condensed)", tool_refs.len());
    }

    // 5. No thinking blocks should leak
    assert!(
        !transcript.contains("\"type\":\"thinking\""),
        "thinking blocks should not appear in transcript"
    );

    println!("  Transcript quality: OK ({} bytes, {} entries)", transcript.len(), result.entries.len());
}

#[test]
#[ignore]
fn test_watermark_prevents_reprocessing() {
    // Use a small session file that won't truncate to verify watermark behavior
    let files = find_session_files();
    if files.is_empty() {
        return;
    }

    // Find a small file (under 100KB) that won't hit the 32KB transcript cap
    let small_file = files.iter().rev().find(|p| {
        p.metadata().map(|m| m.len() < 100_000).unwrap_or(false)
    });

    let path = match small_file {
        Some(p) => p,
        None => {
            println!("  Skipping: no small session files found");
            return;
        }
    };

    // First parse: from beginning
    let result1 = rememora::jsonl::parse_file(path, 0).unwrap();
    if result1.truncated {
        println!("  Skipping: even small file truncated");
        return;
    }

    // For non-truncated files, new_offset should be the file length
    let file_len = path.metadata().unwrap().len();
    assert_eq!(
        result1.new_offset, file_len,
        "non-truncated parse should advance to file end"
    );

    // Second parse from watermark: should have nothing new
    let result2 = rememora::jsonl::parse_file(path, result1.new_offset).unwrap();
    assert_eq!(
        result2.entries.len(), 0,
        "second parse from watermark should have no new entries"
    );

    println!(
        "  Watermark dedup: OK (first: {} entries @ {} bytes, second: 0)",
        result1.entries.len(),
        result1.new_offset
    );

    // Also verify truncation watermark behavior with a large file
    let large_file = &files[0];
    let r1 = rememora::jsonl::parse_file(large_file, 0).unwrap();
    if r1.truncated {
        let large_len = large_file.metadata().unwrap().len();
        assert!(
            r1.new_offset < large_len,
            "truncated parse should NOT advance to file end (got {} vs file len {})",
            r1.new_offset,
            large_len
        );

        // Second parse should pick up remaining content
        let r2 = rememora::jsonl::parse_file(large_file, r1.new_offset).unwrap();
        assert!(
            !r2.entries.is_empty(),
            "second parse after truncation should find more content"
        );
        println!(
            "  Truncation continuation: OK (first: {} entries @ {}, second: {} entries @ {})",
            r1.entries.len(), r1.new_offset, r2.entries.len(), r2.new_offset
        );
    }
}
