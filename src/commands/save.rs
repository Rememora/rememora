use anyhow::Result;
use rusqlite::Connection;

use rememora::models::context::{self, InsertContext};
use rememora::uri;

pub struct SaveArgs {
    pub text: String,
    pub category: String,
    pub project: Option<String>,
    pub importance: f64,
    pub agent: Option<String>,
    pub tags: Option<String>,
    pub abstract_text: Option<String>,
    pub overview: Option<String>,
    pub content_text: Option<String>,
}

pub fn run(conn: &Connection, args: &SaveArgs, json: bool) -> Result<()> {
    let slug = uri::slugify(&args.text.chars().take(60).collect::<String>());
    let mem_uri = uri::build_memory_uri(args.project.as_deref(), &args.category, &slug);
    let parent = uri::parent(&mem_uri)?.unwrap_or_default();

    // Use explicit tiers if provided, otherwise derive from text
    let abstract_text = args
        .abstract_text
        .clone()
        .unwrap_or_else(|| truncate(&args.text, 200));
    let overview = args.overview.clone().unwrap_or_else(|| args.text.clone());
    let content = args.content_text.clone().unwrap_or_else(|| args.text.clone());

    let tags = args.tags.clone().unwrap_or_else(|| "[]".to_string());

    let id = context::insert(
        conn,
        &InsertContext {
            uri: mem_uri.clone(),
            parent_uri: Some(parent),
            context_type: "memory".to_string(),
            category: Some(args.category.clone()),
            name: truncate(&args.text, 80),
            abstract_text,
            overview,
            content,
            tags,
            source_agent: args.agent.clone(),
            source_session: None,
            importance: args.importance,
        },
    )?;

    if json {
        println!(
            "{}",
            serde_json::json!({"id": id, "uri": mem_uri})
        );
    } else {
        println!("{id}");
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
