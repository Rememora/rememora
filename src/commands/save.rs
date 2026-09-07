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
    // `--project` is a request, not the answer. In a git worktree the name an
    // agent supplies is usually the worktree's own directory — a namespace no
    // search will ever filter to. `resolve_write_target` folds that onto the
    // main checkout's project and hands back the provenance that fold loses.
    let cwd = current_dir();
    let target = project::resolve_write_target(conn, args.project.as_deref(), &cwd);

    let slug = uri::slugify(&args.text.chars().take(60).collect::<String>());
    let mem_uri = uri::build_memory_uri(target.project.as_deref(), &args.category, &slug);
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
            source_session: resolve_source_session(conn, target.project.as_deref()),
            importance: args.importance,
            worktree: target.worktree.clone(),
            branch: target.branch.clone(),
        },
    )?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "id": id,
                "uri": mem_uri,
                "project": target.project,
                "worktree": target.worktree,
                "branch": target.branch,
            })
        );
    } else {
        println!("{id}");
        // Surface the rewrite, so an agent that asked for one project and got
        // another finds out at the moment it happens rather than the next time
        // a search comes back empty.
        if let (Some(requested), Some(actual)) = (args.project.as_deref(), target.project.as_deref())
        {
            if requested != actual {
                eprintln!("note: --project {requested} resolved to {actual} (worktree/main checkout)");
            }
        }
    }

    Ok(())
}

/// Process working directory as a string, or empty when it cannot be read.
///
/// An unreadable cwd is not a reason to fail a save — resolution simply falls
/// back to whatever the caller asked for.
fn current_dir() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
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
/// Project resolution goes through `project::resolve_write_target`, the same
/// ladder `session start` and `session end-active` now use, so all three agree
/// on which session is "the" active one. Callers pass the *already resolved*
/// project; when they have none, cwd's own basename is fed through the ladder,
/// which folds a worktree onto its main checkout before the lookup.
///
/// That agreement is the whole point: a session opened as `rememora` and a
/// memory attributed to `worktree-brave-meadow-e668` never join, which is how
/// `rememora eval`'s save rate read zero while saves were plainly happening.
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
            let cwd_str = cwd.to_str()?;
            let basename = cwd.file_name()?.to_str()?;
            project::resolve_write_target(conn, Some(basename), cwd_str).project?
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
        // Byte slicing panics mid-codepoint. Any memory longer than `max` whose
        // `max`th byte lands inside a multi-byte char — an em-dash at byte 200
        // is the one that found this — used to crash the whole command. Walk
        // back to the nearest char boundary instead. Same fix as
        // `commands::evolve::truncate`.
        let cut = (0..=max).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
        format!("{}...", &s[..cut])
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
        let id = session::start(&conn, "claude-code", Some("myapp"), None, "work", None, &session::Provenance::default()).unwrap();

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

    /// `truncate` sliced by byte index, so a memory longer than the cut whose
    /// boundary byte landed inside a multi-byte character panicked the whole
    /// command. Found the hard way: an em-dash at byte 200 of a real memory
    /// crashed `rememora save` outright, losing the write.
    #[test]
    fn truncate_does_not_panic_on_a_multibyte_boundary() {
        // 'a' * 199 then an em-dash straddling bytes 199..202 — the cut at 200
        // lands inside it.
        let text = format!("{}—{}", "a".repeat(199), "b".repeat(50));
        assert!(!text.is_char_boundary(200), "precondition for the bug");

        let out = truncate(&text, 200);

        assert!(out.starts_with(&"a".repeat(199)));
        assert!(out.ends_with("..."));
    }

    /// A string that is entirely one oversized character must still truncate
    /// rather than panic or slice mid-codepoint.
    #[test]
    fn truncate_handles_a_string_with_no_boundary_before_the_cut() {
        let text = "—".repeat(100);
        let out = truncate(&text, 2);
        assert_eq!(out, "...");
    }

    /// An ended session is not the active one — attribution must not latch
    /// onto it after the agent has gone.
    #[test]
    fn ignores_a_session_that_has_already_ended() {
        let conn = db::open_memory().unwrap();
        let id = session::start(&conn, "codex", Some("myapp"), None, "work", None, &session::Provenance::default()).unwrap();
        session::end(&conn, &id, "done", None, None).unwrap();

        assert_eq!(resolve_source_session(&conn, Some("myapp")), None);
    }
}
