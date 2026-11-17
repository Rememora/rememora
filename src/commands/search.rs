use anyhow::{bail, Result};
use rusqlite::Connection;

use rememora::format;
use rememora::models::project;
use rememora::propagate::PropagationConfig;
use rememora::search;

/// Output mode for `rememora search`.
///
/// - `Full`: current markdown format (numbered list, multi-line per hit).
/// - `Compact`: progressive-disclosure single-line-per-hit (~75 tokens).
/// - `Context`: tiny, length-capped form safe for inline prompt injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFormat {
    Full,
    Compact,
    Context,
}

impl SearchFormat {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "full" | "markdown" | "md" => Ok(Self::Full),
            "compact" => Ok(Self::Compact),
            "context" => Ok(Self::Context),
            other => bail!("unknown --format value: {other} (expected full|compact|context)"),
        }
    }
}

pub struct SearchArgs {
    pub query: String,
    pub project: Option<String>,
    pub category: Option<String>,
    pub limit: usize,
    pub propagate: bool,
    pub propagate_decay: f64,
    pub propagate_depth: usize,
    pub format: SearchFormat,
    /// Working directory to resolve the project from when `--project` is absent.
    /// Hooks pass the session cwd here; the CLI falls back to the process cwd.
    pub cwd: Option<String>,
    /// Include each hit's full content in `--json` output.
    pub full: bool,
}

/// Project to filter on: an explicit `--project` wins, otherwise resolve from
/// the working directory.
///
/// An explicit `--project` is still put through `resolve_write_target`, because
/// the read side has to make the same rewrite the write side does. An agent in
/// a worktree that saves under a fabricated name and then searches under the
/// same fabricated name would otherwise agree with itself perfectly and find
/// nothing — the project filter is a hard URI prefix match, so a name matching
/// no project silently excludes every project memory and returns only global
/// ones. That reads as "memory doesn't work" rather than as an error.
///
/// Resolution failure with no `--project` yields `None` (search everything)
/// rather than a guessed name. See `project::resolve_for_cwd` for why that
/// asymmetry matters.
fn effective_project(conn: &Connection, args: &SearchArgs) -> Option<String> {
    let cwd = args
        .cwd
        .clone()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .unwrap_or_default();

    if args.project.is_some() {
        return project::resolve_write_target(conn, args.project.as_deref(), &cwd).project;
    }

    // Best-effort: a resolution error must never fail the search, because this
    // runs on the prompt-submit critical path.
    project::resolve_for_cwd(conn, &cwd).ok().flatten()
}

/// Warning for an explicit `--project` that names no registered project, or
/// `None` when the name is fine. The filter is a hard URI prefix, so such a
/// search can only return global memories; say so instead of failing quietly.
pub fn unregistered_project_warning(conn: &Connection, resolved: Option<&str>) -> Option<String> {
    let name = resolved?;
    if project::registered_name(conn, name).is_some() {
        return None;
    }
    Some(format!(
        "warning: project '{name}' is not registered, so only global memories can match. \
         Run `rememora project list` to see registered names."
    ))
}

pub fn run(conn: &Connection, args: &SearchArgs, json: bool) -> Result<()> {
    let project = effective_project(conn, args);
    if args.project.is_some() {
        if let Some(w) = unregistered_project_warning(conn, project.as_deref()) {
            eprintln!("{w}");
        }
    }

    let results = if args.propagate {
        let config = PropagationConfig {
            decay_factor: args.propagate_decay,
            max_depth: args.propagate_depth,
        };
        search::search_with_propagation(
            conn,
            &args.query,
            project.as_deref(),
            args.category.as_deref(),
            args.limit,
            &config,
        )?
    } else {
        search::search(
            conn,
            &args.query,
            project.as_deref(),
            args.category.as_deref(),
            args.limit,
        )?
    };

    if json {
        println!("{}", format::search_results_to_json(&results, args.full));
    } else {
        match args.format {
            SearchFormat::Full => print!("{}", format::search_results_to_markdown(&results)),
            SearchFormat::Compact => print!("{}", format::search_results_to_compact(&results)),
            SearchFormat::Context => print!("{}", format::search_results_to_context(&results)),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_only_for_an_unregistered_project() {
        let conn = rememora::db::open_memory().unwrap();
        project::add(&conn, "known", Some("/tmp/known"), "", &[]).unwrap();

        assert!(unregistered_project_warning(&conn, Some("known")).is_none());
        assert!(unregistered_project_warning(&conn, Some("KNOWN")).is_none());
        assert!(unregistered_project_warning(&conn, None).is_none());
        let w = unregistered_project_warning(&conn, Some("worktree-feat-x")).unwrap();
        assert!(w.contains("worktree-feat-x") && w.contains("not registered"), "{w}");
    }
}
