use anyhow::Result;
use rusqlite::Connection;

use rememora::models::context::{self, InsertContext};
use rememora::models::{project, session};
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
            source_session: resolve_source_session(conn, args.project.as_deref()),
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

/// Attribute this memory to the session that is open right now, if any.
///
/// `source_session` was hardcoded to `None`, so the column was never populated
/// and `rememora eval`'s memory-save rate — which joins on it — read 0 no
/// matter how much the user saved.
///
/// Project resolution mirrors `session::end_active` (issue #114) so the two
/// commands agree on which session is "the" active one: explicit `--project`,
/// then registered-project lookup by cwd, then `basename(cwd)` — the string
/// the SessionStart hook passes to `session start` from an unregistered
/// directory.
///
/// Every step is best-effort. A save outside any session is legitimate (an
/// agent that never ran `session start`, a manual CLI save), and a save must
/// never fail because attribution could not be worked out — it returns `None`
/// and `eval` reports the memory as unattributed rather than pretending.
fn resolve_source_session(conn: &Connection, project: Option<&str>) -> Option<String> {
    let resolved = match project {
        Some(p) => p.to_string(),
        None => {
            let cwd = std::env::current_dir().ok()?;
            match project::detect_from_cwd(conn, cwd.to_str().unwrap_or("")) {
                Ok(Some(p)) => p,
                _ => cwd.file_name()?.to_str()?.to_string(),
            }
        }
    };

    session::get_active_for_project(conn, &resolved)
        .ok()
        .flatten()
        .map(|s| s.id)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
