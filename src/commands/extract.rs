use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::io::Read;

use rememora::models::context::{self, InsertContext};
use rememora::uri;

const EXTRACT_PROMPT: &str = r#"Extract key memories from the following text. Return a JSON array of objects with these fields:
- "text": the core fact or knowledge (concise, one sentence)
- "category": one of "preference", "entity", "decision", "event", "case", "pattern"
- "importance": float 0.0-1.0 (how important is this for future sessions?)

Categories:
- preference: user or project preferences ("prefers Zustand over Redux")
- entity: key concepts, APIs, people, tools ("Stripe API uses idempotency keys")
- decision: architecture & design choices ("chose expo-router over React Navigation")
- event: milestones, releases, incidents ("v2.0 shipped 2026-03-01")
- case: specific problem + solution ("iOS build fails with Hermes + RN 0.76 — disable new arch")
- pattern: reusable processes or best practices ("always run migrations before seeding")

Rules:
- Only extract genuinely useful knowledge that would help future AI agent sessions
- Skip generic observations, small talk, and meta-commentary about the conversation
- Each memory should be self-contained and understandable without context
- Prefer fewer high-quality memories over many low-quality ones
- Return ONLY the JSON array, no other text

Text:
"#;

pub fn run(
    conn: &Connection,
    project: Option<&str>,
    agent: Option<&str>,
    file: Option<&str>,
    save: bool,
    json_output: bool,
) -> Result<()> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY environment variable not set")?;

    // Read input from file or stdin
    let input = if let Some(path) = file {
        std::fs::read_to_string(path).context("Failed to read input file")?
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("Failed to read from stdin")?;
        buf
    };

    if input.trim().is_empty() {
        bail!("No input text provided. Pipe text to stdin or use --file.");
    }

    let prompt = format!("{EXTRACT_PROMPT}{input}");

    // Call Claude API
    let body = serde_json::json!({
        "model": "claude-haiku-4-5-20251001",
        "max_tokens": 4096,
        "messages": [
            {"role": "user", "content": prompt}
        ]
    });

    let resp = ureq::post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", &api_key)
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .send_json(&body)
        .context("Failed to call Claude API")?;

    let resp_body: serde_json::Value = resp.into_json().context("Failed to parse API response")?;

    // Extract text content from response
    let content_text = resp_body["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or("[]");

    // Parse the JSON array from the response (handle markdown code blocks)
    let json_str = content_text.trim();
    let json_str = if json_str.starts_with("```") {
        // Strip markdown code fences
        let start = json_str.find('[').unwrap_or(0);
        let end = json_str.rfind(']').map(|i| i + 1).unwrap_or(json_str.len());
        &json_str[start..end]
    } else {
        json_str
    };

    let memories: Vec<ExtractedMemory> =
        serde_json::from_str(json_str).context("Failed to parse extracted memories as JSON")?;

    if memories.is_empty() {
        if json_output {
            println!("[]");
        } else {
            println!("No memories extracted.");
        }
        return Ok(());
    }

    if save {
        let mut saved = Vec::new();
        for mem in &memories {
            let slug = uri::slugify(&mem.text.chars().take(60).collect::<String>());
            let mem_uri = uri::build_memory_uri(project, &mem.category, &slug);
            let parent = uri::parent(&mem_uri)?.unwrap_or_default();

            let id = context::insert(
                conn,
                &InsertContext {
                    uri: mem_uri.clone(),
                    parent_uri: Some(parent),
                    context_type: "memory".into(),
                    category: Some(mem.category.clone()),
                    name: truncate(&mem.text, 80),
                    abstract_text: truncate(&mem.text, 200),
                    overview: mem.text.clone(),
                    content: mem.text.clone(),
                    tags: "[]".into(),
                    source_agent: agent.map(String::from),
                    source_session: None,
                    importance: mem.importance,
                },
            )?;
            saved.push(serde_json::json!({
                "id": id,
                "uri": mem_uri,
                "text": mem.text,
                "category": mem.category,
                "importance": mem.importance,
            }));
        }

        if json_output {
            println!("{}", serde_json::to_string_pretty(&saved)?);
        } else {
            println!("Extracted and saved {} memories:", saved.len());
            for s in &saved {
                println!(
                    "  [{}] {} (importance: {})",
                    s["category"].as_str().unwrap_or(""),
                    s["text"].as_str().unwrap_or(""),
                    s["importance"],
                );
            }
        }
    } else if json_output {
        println!("{}", serde_json::to_string_pretty(&memories)?);
    } else {
        println!("Extracted {} memories (use --save to persist):", memories.len());
        for mem in &memories {
            println!(
                "  [{}] {} (importance: {})",
                mem.category, mem.text, mem.importance,
            );
        }
    }

    Ok(())
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ExtractedMemory {
    text: String,
    category: String,
    importance: f64,
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
