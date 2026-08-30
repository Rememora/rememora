use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::context::{self, ContextRecord, InsertContext};
use crate::uri;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: Option<String>,
    pub description: String,
    pub tech_stack: Vec<String>,
    pub conventions: String,
    pub last_active: String,
}

pub fn add(conn: &Connection, name: &str, path: Option<&str>, description: &str, stack: &[String]) -> Result<String> {
    let project_uri = uri::build_project_uri(name);
    let stack_json = serde_json::to_string(stack)?;

    // Store path and stack in the content field as structured JSON
    let content = serde_json::json!({
        "path": path,
        "tech_stack": stack,
        "conventions": "",
    })
    .to_string();

    let id = context::insert(
        conn,
        &InsertContext {
            uri: project_uri,
            parent_uri: Some("rememora://projects".to_string()),
            context_type: "project".to_string(),
            category: None,
            name: name.to_string(),
            abstract_text: description.to_string(),
            overview: format!("Project: {name}. Stack: {}", stack.join(", ")),
            content,
            tags: stack_json,
            source_agent: None,
            source_session: None,
            importance: 1.0,
        },
    )?;

    Ok(id)
}

pub fn list(conn: &Connection) -> Result<Vec<ContextRecord>> {
    context::list_by_scope(conn, Some("project"), None, None, 100)
}

pub fn get(conn: &Connection, name: &str) -> Result<Option<ContextRecord>> {
    let uri = uri::build_project_uri(name);
    context::get_by_uri(conn, &uri)
}

pub fn get_info(conn: &Connection, name: &str) -> Result<Option<ProjectInfo>> {
    let record = get(conn, name)?;
    match record {
        None => Ok(None),
        Some(rec) => {
            let content: serde_json::Value = serde_json::from_str(&rec.content).unwrap_or_default();
            let tech_stack: Vec<String> = serde_json::from_value(
                content.get("tech_stack").cloned().unwrap_or_default(),
            )
            .unwrap_or_default();

            Ok(Some(ProjectInfo {
                name: rec.name,
                path: content.get("path").and_then(|v| v.as_str()).map(String::from),
                description: rec.abstract_text,
                tech_stack,
                conventions: content
                    .get("conventions")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                last_active: rec.updated_at,
            }))
        }
    }
}

pub fn detect_from_cwd(conn: &Connection, cwd: &str) -> Result<Option<String>> {
    let projects = list(conn)?;
    for proj in projects {
        let content: serde_json::Value = serde_json::from_str(&proj.content).unwrap_or_default();
        if let Some(path) = content.get("path").and_then(|v| v.as_str()) {
            if cwd.starts_with(path) {
                return Ok(Some(proj.name));
            }
        }
    }
    Ok(None)
}

/// Resolve the project for a working directory, tolerating git worktrees.
///
/// `detect_from_cwd` prefix-matches the registered project path, which fails in
/// a git worktree: `AGENTS.md` mandates worktrees under `.agents/worktrees/`,
/// but a worktree lives outside the registered checkout so the prefix never
/// matches. Callers used to paper over this with `basename(cwd)`, which
/// fabricates a project name that matches nothing — and because the project
/// filter in `search` is a hard `uri LIKE 'rememora://projects/<name>/%'`
/// clause, a fabricated name silently excludes every project memory and leaves
/// only global ones. That reads as "memory doesn't work" rather than as an error.
///
/// Resolution order:
/// 1. Direct prefix match on the registered path.
/// 2. If `cwd` is inside a git repo, retry against the main checkout, derived
///    from `git rev-parse --git-common-dir` (a worktree's common dir points at
///    the primary `.git`, so its parent is the main working tree).
/// 3. `None` — meaning "no project filter", never a fabricated name.
///
/// Returning `None` is deliberate: an unfiltered search scores only marginally
/// worse than a correctly-filtered one, while a wrong project name drives recall
/// to zero. Prefer the graceful degradation.
pub fn resolve_for_cwd(conn: &Connection, cwd: &str) -> Result<Option<String>> {
    if let Some(name) = detect_from_cwd(conn, cwd)? {
        return Ok(Some(name));
    }

    if let Some(root) = git_main_worktree(cwd) {
        if root != cwd {
            if let Some(name) = detect_from_cwd(conn, &root)? {
                return Ok(Some(name));
            }
            // git reports fully-resolved paths, but a registered path may be
            // recorded through a symlink (on macOS `/tmp` and `/var` are
            // symlinks into `/private`). Retry with both sides canonicalized.
            if let Some(name) = detect_canonical(conn, &root)? {
                return Ok(Some(name));
            }
        }
    }

    Ok(None)
}

/// Prefix match with both sides canonicalized, for symlinked project paths.
///
/// Kept separate from `detect_from_cwd` so the cheap string comparison stays on
/// the hot path — this only runs after that has already missed, and it touches
/// the filesystem once per registered project.
fn detect_canonical(conn: &Connection, path: &str) -> Result<Option<String>> {
    let needle = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };

    for proj in list(conn)? {
        let content: serde_json::Value = serde_json::from_str(&proj.content).unwrap_or_default();
        let Some(registered) = content.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        // A registered path that no longer exists cannot be canonicalized;
        // skip rather than failing the whole resolution.
        let Ok(registered) = std::fs::canonicalize(registered) else {
            continue;
        };
        if needle.starts_with(&registered) {
            return Ok(Some(proj.name));
        }
    }

    Ok(None)
}

/// Absolute path to the main working tree containing `cwd`, if it is in a git repo.
///
/// Uses `--git-common-dir` rather than `--show-toplevel`: in a linked worktree
/// the former resolves to the primary repo's `.git`, which is what lets a
/// worktree map back to the registered project.
fn git_main_worktree(cwd: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let common_dir = String::from_utf8(out.stdout).ok()?;
    let common_dir = common_dir.trim();
    if common_dir.is_empty() {
        return None;
    }

    std::path::Path::new(common_dir)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn update_last_active(conn: &Connection, name: &str) -> Result<()> {
    let uri = uri::build_project_uri(name);
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE contexts SET updated_at = ?1 WHERE uri = ?2",
        params![now, uri],
    )?;
    Ok(())
}
