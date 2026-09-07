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
            // A project registration is not a worktree observation — it
            // describes the repo, not the tree someone happened to run
            // `project add` from.
            worktree: None,
            branch: None,
        },
    )?;

    Ok(id)
}

/// Every registered project.
///
/// The limit is deliberately far above any plausible project count: this backs
/// `registered_name`, `detect_from_cwd`, `detect_canonical` and
/// `find_stranded`'s registered set, and `list_by_scope` orders by
/// `importance DESC, created_at DESC`. A tight limit would silently drop the
/// oldest projects off the end, so step 1 of the resolution ladder would miss a
/// real project and `find_stranded` would report a *registered* project as
/// stranded — which `reconcile --apply` would then happily move.
pub fn list(conn: &Connection) -> Result<Vec<ContextRecord>> {
    context::list_by_scope(conn, Some("project"), None, None, 100_000)
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
    git_context(cwd).map(|g| g.main_worktree)
}

/// Where a working directory sits in git, as far as project resolution cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitContext {
    /// Absolute path to the primary working tree (the main checkout).
    pub main_worktree: String,
    /// Absolute path to *this* working tree — equal to `main_worktree` unless
    /// `cwd` is inside a linked worktree.
    pub toplevel: String,
    /// Branch checked out here, or `None` when detached.
    pub branch: Option<String>,
}

impl GitContext {
    /// Whether `cwd` is in a linked worktree rather than the main checkout.
    pub fn is_linked_worktree(&self) -> bool {
        self.toplevel != self.main_worktree
    }

    /// The linked worktree's directory name, or `None` in the main checkout.
    ///
    /// This is what lands in the `worktree` column: `None` means "written from
    /// the main checkout", which is a real answer rather than a missing one.
    pub fn worktree_name(&self) -> Option<String> {
        if !self.is_linked_worktree() {
            return None;
        }
        std::path::Path::new(&self.toplevel)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    }
}

/// One `git rev-parse` producing a single path, or `None` if git says no.
///
/// Each option is asked for separately rather than batched: `--show-toplevel`
/// fails (exit 128, "this operation must be run in a work tree") inside a bare
/// repo or a `.git` directory, and a batched call discards the `--git-common-dir`
/// line that did succeed alongside it. `--git-common-dir` is the load-bearing
/// one, so it must not be lost to its neighbour's failure.
fn rev_parse_path(cwd: &str, option: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", option])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Whether a path runs through a `.git` directory.
///
/// A submodule's common dir is `<super>/.git/modules/<name>`, and a
/// `--separate-git-dir` checkout is shaped the same way, so the parent of the
/// common dir is not a working tree at all — it is somewhere inside `.git`.
/// Without this check every submodule on the machine resolves to a project
/// literally named `modules`.
fn runs_through_git_dir(path: &str) -> bool {
    std::path::Path::new(path)
        .components()
        .any(|c| c.as_os_str() == ".git")
}

/// Git facts about `cwd`, or `None` when it is not inside a repository.
pub fn git_context(cwd: &str) -> Option<GitContext> {
    let common_dir = rev_parse_path(cwd, "--git-common-dir")?;
    let toplevel = rev_parse_path(cwd, "--show-toplevel");

    // A linked worktree's common dir is the *primary* repo's `.git`, so its
    // parent is the main working tree. In the main checkout the two coincide.
    let mut main_worktree = std::path::Path::new(&common_dir)
        .parent()?
        .to_string_lossy()
        .into_owned();

    if runs_through_git_dir(&main_worktree) {
        // A submodule or separate-git-dir checkout: its own toplevel *is* its
        // working tree, and it has no linked-worktree relationship to express.
        // With no toplevel to fall back on there is nothing trustworthy to say.
        main_worktree = toplevel.clone()?;
    }

    // `--show-toplevel` failing means cwd is not in a working tree (bare repo,
    // inside `.git`). Treating the main worktree as the toplevel keeps
    // `is_linked_worktree()` false rather than inventing a linkage.
    let toplevel = toplevel.unwrap_or_else(|| main_worktree.clone());

    let branch = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Some(GitContext {
        main_worktree,
        toplevel,
        branch,
    })
}

/// Where a memory should be written, and where it was written *from*.
///
/// `project` is the canonical scope the memory belongs to; `worktree` and
/// `branch` are provenance, recorded so folding a worktree write onto the main
/// project does not erase which tree produced it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteTarget {
    /// Canonical project name, or `None` for global scope.
    pub project: Option<String>,
    /// Linked worktree the write came from; `None` means the main checkout.
    pub worktree: Option<String>,
    /// Branch checked out at write time.
    pub branch: Option<String>,
}

/// Resolve the project a write belongs to, folding worktrees onto their main
/// checkout, and collect the provenance that fold would otherwise lose.
///
/// Every write path used to name the project after whatever directory it was
/// standing in. In a git worktree — which is where agent work happens — that
/// produced names matching no registered project, and because the project
/// filter is a hard `uri LIKE 'rememora://projects/<name>/%'` prefix match,
/// those memories were unreachable from the moment they were written. On a real
/// 300-context store, 55 contexts (18%) were filed under fabricated names, in
/// three shapes: `basename(worktree)`, Claude Code's `-Users-…-encoded-path`,
/// and case drift like `Ana` vs `ana`.
///
/// Resolution ladder, first match wins:
///
/// 1. `requested` names a **registered** project (case-insensitively) — honored
///    verbatim, in its canonical spelling.
/// 2. `requested` is an encoded filesystem path that resolves to a registered
///    project — covers the curator, which passes Claude Code's
///    `~/.claude/projects/` directory name straight through.
/// 3. `requested` is *directory-derived* — a name the tooling synthesised from
///    the filesystem rather than one a caller chose — and `cwd` resolves it.
/// 4. Same, and `cwd` is a linked worktree: fall back to the *main checkout's*
///    directory name, never the worktree's, so an unregistered project still
///    gets a name that survives the worktree being deleted.
/// 5. `requested` verbatim — a project that simply is not registered yet.
///
/// **Steps 3 and 4 are gated on [`is_directory_derived`], and that gate is the
/// difference between a fix and a hijack.** Without it, any `--project` naming a
/// not-yet-registered project gets silently redirected to whatever project the
/// caller happens to be standing in: `--project ana` from inside the `myapp`
/// worktree would write `ana`'s memory into `myapp`, `search --project ana`
/// would return `myapp`'s memories, and `evolve --project ana` would consolidate
/// `myapp`'s. The documented workflow is "save first, `project add` later", so
/// that is the common case, not an edge one. The ladder can only override a name
/// it can show the tooling invented.
///
/// `requested == None` yields `project: None`, i.e. global scope, unchanged.
/// Auto-namespacing an unscoped save would silently reclassify the global
/// preferences that agents are instructed to write without `--project`.
pub fn resolve_write_target(
    conn: &Connection,
    requested: Option<&str>,
    cwd: &str,
) -> WriteTarget {
    let git = git_context(cwd);
    let target = WriteTarget {
        project: None,
        worktree: git.as_ref().and_then(GitContext::worktree_name),
        branch: git.as_ref().and_then(|g| g.branch.clone()),
    };

    let Some(requested) = requested else {
        return target;
    };

    // 1. An explicitly named, registered project always wins.
    if let Some(canonical) = registered_name(conn, requested) {
        return WriteTarget {
            project: Some(canonical),
            ..target
        };
    }

    // 2. The name is an encoded path (the curator's input shape).
    if let Some(name) = project_from_encoded_path(conn, requested) {
        return WriteTarget {
            project: Some(name),
            ..target
        };
    }

    if is_directory_derived(requested, cwd, git.as_ref()) {
        // 3. The working directory knows, including through a worktree.
        if let Ok(Some(name)) = resolve_for_cwd(conn, cwd) {
            return WriteTarget {
                project: Some(name),
                ..target
            };
        }

        // 4. Unregistered, but name it after the main checkout rather than the
        //    disposable worktree directory.
        if let Some(git) = git.as_ref() {
            if git.is_linked_worktree() {
                if let Some(name) = basename(&git.main_worktree) {
                    return WriteTarget {
                        project: Some(name),
                        ..target
                    };
                }
            }
        }
    }

    // 5. Take the caller at their word.
    WriteTarget {
        project: Some(requested.to_string()),
        ..target
    }
}

/// Whether `requested` is a name the tooling synthesised from the filesystem,
/// rather than one a caller deliberately chose.
///
/// Only these shapes may be overridden by the working directory. Every
/// fabricated name this change exists to repair is one of them:
///
/// - an encoded path (leading `-`) — `curate`'s transcript directory;
/// - `basename(cwd)` — what the SessionStart/SessionEnd/Stop hooks pass;
/// - `basename(toplevel)` or `basename(main_worktree)` — the same thing when the
///   hook ran from a subdirectory of the tree.
///
/// Anything else is the caller's word and is left alone, even when it names no
/// project that exists yet.
fn is_directory_derived(requested: &str, cwd: &str, git: Option<&GitContext>) -> bool {
    if requested.starts_with('-') {
        return true;
    }
    if basename(cwd).as_deref() == Some(requested) {
        return true;
    }
    git.is_some_and(|g| {
        basename(&g.toplevel).as_deref() == Some(requested)
            || basename(&g.main_worktree).as_deref() == Some(requested)
    })
}

/// The canonical spelling of `name` if a project by that name is registered.
///
/// Case-insensitive because the store accumulated both `ana` and `Ana` as
/// separate URI namespaces, which is two homes for one project's memories.
pub fn registered_name(conn: &Connection, name: &str) -> Option<String> {
    let projects = list(conn).ok()?;
    projects
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| p.name)
}

/// Resolve a Claude Code encoded project-directory name to a project.
///
/// Claude Code stores transcripts under `~/.claude/projects/<encoded-cwd>/`,
/// where the encoding replaces `/` and `.` with `-`. The curator passed that
/// directory name through as the project name, which is how
/// `-Users-ovidb-Projects-rememora-rememora` became a project namespace.
///
/// The encoding is lossy — `-` replaces both `/` and `.`, and is itself a legal
/// path character — so it cannot be decoded exactly. This walks candidate
/// prefixes from longest to shortest and lets the filesystem disambiguate what
/// the encoding cannot.
///
/// A candidate is accepted only if it resolves to a **registered project**, or
/// is itself a **git working-tree root**.
///
/// The obvious-looking shortcut — "it is a real directory, so name the project
/// after it" — is how an earlier draft of this function turned
/// `-Users-ovidb-Projects-deleted-thing` into a project called `Projects` and
/// `-Users-ovidb--claude-worktrees-foo` into one called `ovidb`. Shortening the
/// path until *something* exists always succeeds eventually, so that rule
/// reliably lands on a generic ancestor and mints a brand-new junk namespace
/// that collides across every unrelated repo beneath it — reintroducing exactly
/// the bug this module exists to remove, and doing it inside `reconcile`, where
/// it gets written to disk.
///
/// "Is a repository root" is the discriminator that separates the two: a
/// project *is* a repo, while `~/Projects` and `~` are containers. See
/// [`repo_root_name`] for the guards that keep it honest. When nothing
/// qualifies the answer is `None` — the name then falls through the ladder, and
/// `reconcile` reports it rather than guessing.
///
/// Returns `None` for anything that is not an encoded absolute path, so an
/// ordinary project name falls through untouched.
pub fn project_from_encoded_path(conn: &Connection, encoded: &str) -> Option<String> {
    let candidates = decoded_candidates(encoded);

    // A registered project is the strongest answer available, so look for one
    // across every candidate before settling for a repository name.
    candidates
        .iter()
        .find_map(|candidate| resolve_for_cwd(conn, candidate).ok().flatten())
        .or_else(|| candidates.iter().find_map(|c| repo_root_name(c)))
}

/// The project name implied by `path` when `path` is a git working-tree root.
///
/// Two guards, each closing a way this could fabricate:
///
/// - `path` must be the working tree's own root, not merely somewhere inside
///   one. Without this, any subdirectory of any repo answers with that repo's
///   name, so a walk that overshoots still gets a confident-looking answer.
/// - the root must not be the user's home directory. Keeping dotfiles in a repo
///   at `~` is common, and it would otherwise make every unresolvable path under
///   `~` resolve to the username.
///
/// Returns the *main* checkout's name, so a linked worktree answers with the
/// project rather than with itself.
fn repo_root_name(path: &str) -> Option<String> {
    let git = git_context(path)?;

    let canonical = |p: &str| std::fs::canonicalize(p).ok();
    let here = canonical(path)?;
    let is_root = canonical(&git.toplevel) == Some(here.clone())
        || canonical(&git.main_worktree) == Some(here.clone());
    if !is_root {
        return None;
    }

    if dirs::home_dir().and_then(|h| canonical(h.to_str()?)) == Some(here) {
        return None;
    }

    basename(&git.main_worktree)
}

/// Existing directories an encoded name could denote, longest path first.
///
/// An *empty* segment marks a `.` that the encoding flattened: Claude Code maps
/// both `/` and `.` to `-`, so `/Users/me/.claude/x` encodes as
/// `-Users-me--claude-x` and splitting on `-` yields an empty element before
/// `claude`. Rebuilding it as a dot-prefix is what lets `~/.claude/worktrees/…`
/// decode at all — the earlier version produced `//` there, which POSIX
/// collapses, so the path never existed and the walk fell through to a generic
/// ancestor.
///
/// Only directories that exist are returned; the caller decides whether any of
/// them means anything.
fn decoded_candidates(encoded: &str) -> Vec<String> {
    let Some(rest) = encoded.strip_prefix('-') else {
        // Encoded absolute paths always start with the separator that replaced
        // the leading `/`. Without this guard every unregistered project name
        // would pay for a filesystem walk.
        return Vec::new();
    };

    let mut components: Vec<String> = Vec::new();
    let mut dotted = false;
    for segment in rest.split('-') {
        if segment.is_empty() {
            dotted = true;
            continue;
        }
        components.push(if dotted {
            format!(".{segment}")
        } else {
            segment.to_string()
        });
        dotted = false;
    }

    if components.len() < 2 {
        return Vec::new();
    }

    (2..=components.len())
        .rev()
        .map(|take| format!("/{}", components[..take].join("/")))
        .filter(|candidate| std::path::Path::new(candidate).is_dir())
        .collect()
}

/// A project namespace that exists in URIs but is not a registered project.
///
/// These are the residue of the fabricated-name bug: every one of them is a
/// `rememora://projects/<name>/…` subtree that the project filter can reach
/// only if a caller happens to pass the exact same fabricated string.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StrandedNamespace {
    /// The namespace as it appears in URIs today.
    pub name: String,
    /// How many contexts live under it.
    pub memories: usize,
    /// Where it should be re-homed, or `None` when nothing could resolve it.
    pub target: Option<String>,
    /// Worktree label recovered along the way, stamped onto the moved rows.
    pub worktree: Option<String>,
    /// How `target` was arrived at — shown in the dry run so the operator can
    /// judge each rewrite rather than trusting the batch.
    pub via: &'static str,
}

/// What a reconcile run actually did.
#[derive(Debug, Default, serde::Serialize)]
pub struct ReconcileOutcome {
    pub namespaces_rewritten: usize,
    pub memories_moved: usize,
    pub sessions_moved: usize,
    /// Rows left in place because the destination URI was already taken —
    /// the same memory saved twice, once under each name.
    pub conflicts: usize,
    pub unresolved: Vec<String>,
    /// Namespaces that already have the right name and simply were never
    /// registered. Nothing to move; the fix is `rememora project add`.
    pub needs_registration: Vec<String>,
}

/// Every project namespace in the URI tree that no registered project claims.
///
/// Resolution reuses the same ladder as the write path, plus one source the
/// write path does not have: the `sessions` table records the `cwd` a session
/// ran in, so a namespace named after a long-deleted worktree can still be
/// traced back to the checkout it belonged to.
pub fn find_stranded(conn: &Connection) -> Result<Vec<StrandedNamespace>> {
    let registered: Vec<String> = list(conn)?.into_iter().map(|p| p.name).collect();

    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut stmt = conn.prepare("SELECT uri FROM contexts WHERE uri LIKE 'rememora://projects/%'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for uri in rows {
        if let Some(name) = crate::uri::extract_project(&uri?) {
            *counts.entry(name).or_default() += 1;
        }
    }

    let mut out = Vec::new();
    for (name, memories) in counts {
        // An exact registered name is home already. The case-insensitive match
        // below still catches `Ana` when `ana` is what is registered.
        if registered.iter().any(|r| r == &name) {
            continue;
        }
        let (target, worktree, via) = resolve_stranded(conn, &name);
        out.push(StrandedNamespace {
            name,
            memories,
            target,
            worktree,
            via,
        });
    }

    Ok(out)
}

/// Work out where a stranded namespace belongs, and which worktree it came from.
///
/// The worktree label is derived from the *namespace name*, never from a session
/// row. The namespace name is a property of every row in the subtree; the
/// `cwd` of the most recent session is not, so stamping that onto rows written
/// from other trees would assert something false. When the namespace name is a
/// bare directory name that differs from the resolved project, that name **is**
/// the worktree those writes came from — which is exactly the shape being
/// repaired.
fn resolve_stranded(
    conn: &Connection,
    name: &str,
) -> (Option<String>, Option<String>, &'static str) {
    // Case drift: `Ana` and `ana` are one project with two namespaces. Not a
    // worktree, so no label.
    if let Some(canonical) = registered_name(conn, name) {
        return (Some(canonical), None, "case-insensitive project name");
    }

    // Claude Code's encoded transcript directory, passed through as a name. The
    // encoded path names a directory, not necessarily a worktree, and the decode
    // is ambiguous — so claim nothing about provenance here.
    if let Some(target) = project_from_encoded_path(conn, name) {
        return (Some(target), None, "encoded path");
    }

    // The namespace is a bare directory name — almost always a worktree that
    // has since been deleted, so the name alone cannot be resolved. The
    // sessions table remembers the cwd those writes happened in.
    if let Some(cwd) = latest_session_cwd(conn, name) {
        if let Ok(Some(target)) = resolve_for_cwd(conn, &cwd) {
            let label = worktree_label(name, &target);
            return (Some(target), label, "session cwd");
        }
        if let Some(root) = git_main_worktree(&cwd) {
            if let Some(target) = basename(&root) {
                let label = worktree_label(name, &target);
                return (Some(target), label, "session cwd → main checkout");
            }
        }
    }

    (None, None, "unresolved")
}

/// The namespace name as a worktree label, or `None` when it is the project's
/// own name rather than a worktree's.
fn worktree_label(namespace: &str, target: &str) -> Option<String> {
    (namespace != target).then(|| namespace.to_string())
}

fn basename(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// The most recent working directory a session recorded under `project`.
fn latest_session_cwd(conn: &Connection, project: &str) -> Option<String> {
    conn.query_row(
        "SELECT cwd FROM sessions
         WHERE project = ?1 AND cwd IS NOT NULL AND cwd != ''
         ORDER BY started_at DESC LIMIT 1",
        params![project],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

/// Re-home every resolvable stranded namespace onto its real project.
///
/// Runs as one transaction: either the whole rewrite lands or none of it does.
/// A row whose destination URI is already occupied is left exactly where it is
/// and counted as a conflict — that means the same memory was saved twice, once
/// under each name, and silently dropping one of them is not this command's
/// call to make.
pub fn reconcile(conn: &Connection, plan: &[StrandedNamespace]) -> Result<ReconcileOutcome> {
    use rusqlite::{Transaction, TransactionBehavior};

    let mut outcome = ReconcileOutcome::default();
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    for ns in plan {
        let Some(target) = ns.target.as_deref() else {
            outcome.unresolved.push(ns.name.clone());
            continue;
        };

        // The namespace already has the right name — it is simply not a
        // registered project (`renotes` in a real store: 20 memories filed
        // coherently under a project nobody ever ran `project add` for). There
        // is nothing to move. Rewriting anyway would map every URI onto itself,
        // and the UNIQUE-collision check below would then count each row as a
        // conflict with itself.
        if target == ns.name {
            outcome.needs_registration.push(ns.name.clone());
            continue;
        }

        let old_prefix = format!("rememora://projects/{}/", ns.name);
        let new_prefix = format!("rememora://projects/{target}/");

        // `substr(...) = ?` rather than `LIKE ? || '%'`: SQLite's LIKE is
        // ASCII-case-insensitive by default, so a `Ana` namespace would sweep
        // in every `ana` row too — and case drift is one of the exact shapes
        // this command exists to repair. `=` on TEXT uses BINARY collation and
        // is case-sensitive.
        let mut stmt =
            tx.prepare("SELECT id, uri FROM contexts WHERE substr(uri, 1, length(?1)) = ?1")?;
        let rows: Vec<(String, String)> = stmt
            .query_map(params![old_prefix], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut moved = 0usize;
        for (id, uri) in rows {
            let new_uri = uri.replacen(&old_prefix, &new_prefix, 1);
            // Nothing to do, and letting it through would make the row collide
            // with itself in the check below.
            if new_uri == uri {
                continue;
            }

            // `contexts.uri` is UNIQUE, so a collision would abort the whole
            // transaction. Check first and skip the row instead.
            let taken: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM contexts WHERE uri = ?1)",
                params![new_uri],
                |row| row.get(0),
            )?;
            if taken {
                outcome.conflicts += 1;
                continue;
            }

            tx.execute(
                "UPDATE contexts
                 SET uri = ?1,
                     parent_uri = CASE
                        WHEN substr(parent_uri, 1, length(?2)) = ?2
                        THEN ?3 || substr(parent_uri, length(?2) + 1)
                        ELSE parent_uri
                     END,
                     worktree = COALESCE(worktree, ?4)
                 WHERE id = ?5",
                params![new_uri, old_prefix, new_prefix, ns.worktree, id],
            )?;

            // `relations` stores URIs, not ids, so an edge into a re-homed
            // memory dangles the moment its URI changes. Rewriting the row and
            // leaving its edges behind would trade one silent breakage for
            // another.
            tx.execute(
                "UPDATE relations SET source_uri = ?1 WHERE source_uri = ?2",
                params![new_uri, uri],
            )?;
            tx.execute(
                "UPDATE relations SET target_uri = ?1 WHERE target_uri = ?2",
                params![new_uri, uri],
            )?;

            moved += 1;
        }

        // Sessions carry the same fabricated project name, and `end-active`
        // looks sessions up by it. Move them too or the next `session
        // end-active` in a fixed-up project will not find its own row.
        let sessions = tx.execute(
            "UPDATE sessions SET project = ?1, worktree = COALESCE(worktree, ?2) WHERE project = ?3",
            params![target, ns.worktree, ns.name],
        )?;

        if moved > 0 || sessions > 0 {
            outcome.namespaces_rewritten += 1;
        }
        outcome.memories_moved += moved;
        outcome.sessions_moved += sessions;
    }

    tx.commit()?;
    Ok(outcome)
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
