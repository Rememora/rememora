use anyhow::Result;
use rusqlite::Connection;

use rememora::models::context::{self, InsertContext};
use rememora::models::{project, session};
use rememora::similar::{self, SimilarMemory};
use rememora::uri;

#[derive(Debug, Default, Clone)]
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
    /// `project` is already canonical (copied from an existing memory's URI),
    /// so skip the write-target ladder rather than risk re-resolving it.
    pub project_is_canonical: bool,
}

/// Result of [`save`]: either a new row or the live duplicate that made one
/// unnecessary.
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub id: String,
    pub uri: String,
    pub target: project::WriteTarget,
    /// `false` when an identical live memory already existed and was reused.
    pub created: bool,
    pub similar: Vec<SimilarMemory>,
}

/// Validate and write one memory, without printing anything.
///
/// Shared by `save` and `supersede --text`, which runs it inside a transaction.
pub fn save(conn: &Connection, args: &SaveArgs) -> Result<SaveOutcome> {
    context::validate_category(&args.category)?;
    context::validate_importance(args.importance)?;

    // `--project` is a request, not the answer. In a git worktree the name an
    // agent supplies is usually the worktree's own directory — a namespace no
    // search will ever filter to. `resolve_write_target` folds that onto the
    // main checkout's project and hands back the provenance that fold loses.
    let cwd = current_dir();
    let target = if args.project_is_canonical {
        project::WriteTarget {
            project: args.project.clone(),
            ..project::resolve_write_target(conn, None, &cwd)
        }
    } else {
        project::resolve_write_target(conn, args.project.as_deref(), &cwd)
    };

    let slug = uri::slugify(&args.text.chars().take(60).collect::<String>());
    let base_uri = uri::build_memory_uri(target.project.as_deref(), &args.category, &slug);
    let content = args.content_text.clone().unwrap_or_else(|| args.text.clone());

    // Saving the same fact twice is a retry, not an error: hand back the row
    // that already holds it.
    if let Some(existing) = context::find_live_duplicate(conn, &base_uri, &content)? {
        return Ok(SaveOutcome {
            id: existing.id,
            uri: existing.uri,
            target,
            created: false,
            similar: Vec::new(),
        });
    }

    let mem_uri = context::unique_uri(conn, &base_uri)?;
    let parent = uri::parent(&mem_uri)?.unwrap_or_default();

    // Use explicit tiers if provided, otherwise derive from text
    let abstract_text = args
        .abstract_text
        .clone()
        .unwrap_or_else(|| truncate(&args.text, 200));
    let overview = args.overview.clone().unwrap_or_else(|| args.text.clone());

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
            content: content.clone(),
            tags,
            source_agent: args.agent.clone(),
            source_session: resolve_source_session(conn, target.project.as_deref()),
            importance: args.importance,
            worktree: target.worktree.clone(),
            branch: target.branch.clone(),
        },
    )?;

    // Advisory only: a failure here must never fail the save.
    let similar = similar::find_similar(conn, &content, target.project.as_deref(), &id).unwrap_or_default();

    Ok(SaveOutcome {
        id,
        uri: mem_uri,
        target,
        created: true,
        similar,
    })
}

pub fn run(conn: &Connection, args: &SaveArgs, json: bool) -> Result<()> {
    let out = save(conn, args)?;
    let target = &out.target;

    if !out.created {
        eprintln!("note: identical memory already exists, not saved again: {}", out.id);
    } else if args.project.is_some() {
        if let Some(warning) = unregistered_project_warning(conn, target.project.as_deref()) {
            eprintln!("{warning}");
        }
    }
    for s in &out.similar {
        eprintln!(
            "similar existing memory: {} \"{}\". If this replaces it, run: rememora supersede {} --by {}",
            s.id, s.name, s.id, out.id
        );
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "id": out.id,
                "uri": out.uri,
                "project": target.project,
                "worktree": target.worktree,
                "branch": target.branch,
                "created": out.created,
                "duplicate_of": if out.created { None } else { Some(&out.id) },
                "similar": out.similar,
            })
        );
    } else {
        println!("{}", out.id);
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

/// A short warning when `project` is not registered. The save still goes
/// ahead ("save first, `project add` later" is the documented workflow), but a
/// typo would otherwise strand the memory under a namespace nothing searches.
pub fn unregistered_project_warning(conn: &Connection, project: Option<&str>) -> Option<String> {
    let name = project?;
    if project::registered_name(conn, name).is_some() {
        return None;
    }
    let names: Vec<String> = project::list(conn).ok()?.into_iter().map(|p| p.name).collect();
    let known = if names.is_empty() {
        "none registered".to_string()
    } else if names.len() > 8 {
        format!("registered: {}, and {} more", names[..8].join(", "), names.len() - 8)
    } else {
        format!("registered: {}", names.join(", "))
    };
    Some(format!(
        "warning: project '{name}' is not registered ({known}). Saved anyway; run `rememora project add {name}` if it is new"
    ))
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

    fn args(text: &str) -> SaveArgs {
        SaveArgs {
            text: text.into(),
            category: "decision".into(),
            project: Some("p".into()),
            importance: 0.5,
            ..Default::default()
        }
    }

    /// Saving the same text twice used to die on the UNIQUE uri constraint.
    #[test]
    fn exact_duplicate_returns_the_existing_id() {
        let conn = db::open_memory().unwrap();
        let first = save(&conn, &args("Use WAL mode for SQLite")).unwrap();
        let again = save(&conn, &args("use WAL  mode for sqlite")).unwrap();
        assert!(first.created);
        assert!(!again.created);
        assert_eq!(again.id, first.id);
    }

    /// A duplicate in another category is a different memory, not a retry.
    #[test]
    fn same_text_in_another_category_is_saved() {
        let conn = db::open_memory().unwrap();
        let first = save(&conn, &args("Use WAL mode for SQLite")).unwrap();
        let other = save(&conn, &SaveArgs { category: "pattern".into(), ..args("Use WAL mode for SQLite") }).unwrap();
        assert!(other.created);
        assert_ne!(other.id, first.id);
    }

    /// Different texts sharing their first 60 chars got the same slug.
    #[test]
    fn slug_collision_gets_a_unique_uri() {
        let conn = db::open_memory().unwrap();
        let prefix = "The build uses bazel with remote caching enabled for every target";
        let a = save(&conn, &args(&format!("{prefix} one"))).unwrap();
        let b = save(&conn, &args(&format!("{prefix} two"))).unwrap();
        assert!(b.created);
        assert_ne!(a.uri, b.uri);
        assert!(b.uri.starts_with(&a.uri));
    }

    /// A superseded memory with the same text does not swallow a new save.
    #[test]
    fn superseded_duplicate_does_not_block_a_resave() {
        let conn = db::open_memory().unwrap();
        let a = save(&conn, &args("Use WAL mode for SQLite")).unwrap();
        let b = save(&conn, &args("Use rollback journal")).unwrap();
        context::supersede(&conn, &a.id, &b.id).unwrap();
        let c = save(&conn, &args("Use WAL mode for SQLite")).unwrap();
        assert!(c.created);
    }

    #[test]
    fn near_duplicate_is_reported_but_saved() {
        let conn = db::open_memory().unwrap();
        let a = save(&conn, &args("Search ranking uses BM25 over the FTS5 index with a hotness boost")).unwrap();
        let b = save(&conn, &args("search ranking: BM25 over FTS5 index plus hotness boost")).unwrap();
        assert!(b.created);
        assert_eq!(b.similar.first().map(|s| s.id.clone()), Some(a.id));
    }

    #[test]
    fn near_duplicate_ignores_other_projects() {
        let conn = db::open_memory().unwrap();
        save(&conn, &SaveArgs { project: Some("q".into()), ..args("Search ranking uses BM25 over the FTS5 index with a hotness boost") }).unwrap();
        let b = save(&conn, &args("search ranking: BM25 over FTS5 index plus hotness boost")).unwrap();
        assert!(b.similar.is_empty());
    }

    #[test]
    fn importance_out_of_range_is_rejected() {
        let conn = db::open_memory().unwrap();
        let err = save(&conn, &SaveArgs { importance: 7.0, ..args("x") }).unwrap_err();
        assert!(err.to_string().contains("between 0.0 and 1.0"));
        assert!(save(&conn, &SaveArgs { importance: f64::NAN, ..args("x") }).is_err());
    }

    #[test]
    fn invalid_category_lists_valid_ones() {
        let conn = db::open_memory().unwrap();
        let err = save(&conn, &SaveArgs { category: "bogus".into(), ..args("x") }).unwrap_err();
        assert!(err.to_string().contains("preference, entity, decision, event, case, pattern"));
    }

    #[test]
    fn unregistered_project_warns_with_registered_names() {
        let conn = db::open_memory().unwrap();
        project::add(&conn, "known", None, "", &[]).unwrap();
        let w = unregistered_project_warning(&conn, Some("typo")).unwrap();
        assert!(w.contains("'typo'") && w.contains("registered: known"));
        assert!(unregistered_project_warning(&conn, Some("known")).is_none());
        assert!(unregistered_project_warning(&conn, None).is_none());
    }
}
