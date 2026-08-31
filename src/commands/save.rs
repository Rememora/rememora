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

/// Attribute a memory to the session that is open right now, if any.
///
/// `source_session` was hardcoded to `None` on every memory-creation path, so
/// the column was never populated and `rememora eval`'s memory-save rate —
/// which joins on it — read 0 no matter how much the user saved. Shared with
/// `commands::extract`, the other user-reachable path that writes memories;
/// both must attribute or the metric goes quiet again for whichever one does
/// not.
///
/// Project resolution mirrors `session::end_active` (issue #114) so the
/// commands agree on which session is "the" active one: explicit `--project`,
/// then registered-project lookup by cwd, then `basename(cwd)` — the string
/// the SessionStart hook passes to `session start` from an unregistered
/// directory.
///
/// Every step is best-effort. A save outside any session is legitimate (an
/// agent that never ran `session start`, a manual CLI save), and a save must
/// never fail because attribution could not be worked out — it returns `None`
/// and `eval` reports the memory as unattributed rather than pretending.
pub fn resolve_source_session(conn: &Connection, project: Option<&str>) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rememora::db;

    /// Direct coverage for the helper `commands::extract` shares. `extract`
    /// itself cannot be driven in a test — it POSTs to the live Anthropic API
    /// before it writes anything — so the attribution it relies on is pinned
    /// here instead.
    #[test]
    fn resolves_the_active_session_for_an_explicit_project() {
        let conn = db::open_memory().unwrap();
        let id = session::start(&conn, "claude-code", Some("myapp"), None, "work", None).unwrap();

        assert_eq!(
            resolve_source_session(&conn, Some("myapp")),
            Some(id),
            "a memory written while a session is open must be attributed to it"
        );
    }

    /// Attribution is best-effort: no session is a legitimate state, and the
    /// write must go ahead unattributed rather than fail.
    #[test]
    fn returns_none_when_no_session_is_open() {
        let conn = db::open_memory().unwrap();
        assert_eq!(resolve_source_session(&conn, Some("myapp")), None);
    }

    /// An ended session is not the active one — attribution must not latch
    /// onto it after the agent has gone.
    #[test]
    fn ignores_a_session_that_has_already_ended() {
        let conn = db::open_memory().unwrap();
        let id = session::start(&conn, "codex", Some("myapp"), None, "work", None).unwrap();
        session::end(&conn, &id, "done", None, None).unwrap();

        assert_eq!(resolve_source_session(&conn, Some("myapp")), None);
    }
}
