use anyhow::{bail, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRecord {
    pub id: String,
    pub uri: String,
    pub parent_uri: Option<String>,
    pub context_type: String,
    pub category: Option<String>,
    pub name: String,
    #[serde(rename = "abstract")]
    pub abstract_text: String,
    pub overview: String,
    pub content: String,
    pub tags: String,
    pub source_agent: Option<String>,
    pub source_session: Option<String>,
    pub importance: f64,
    pub active_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub superseded_by: Option<String>,
    /// Linked worktree this was written from; `None` means the main checkout.
    /// See migration 007 — provenance, deliberately separate from the project
    /// the memory belongs to.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Branch checked out when this was written.
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Default)]
pub struct InsertContext {
    pub uri: String,
    pub parent_uri: Option<String>,
    pub context_type: String,
    pub category: Option<String>,
    pub name: String,
    pub abstract_text: String,
    pub overview: String,
    pub content: String,
    pub tags: String,
    pub source_agent: Option<String>,
    pub source_session: Option<String>,
    pub importance: f64,
    /// Provenance from `project::WriteTarget`. Both default to `None`, which
    /// reads as "written from the main checkout" — the right answer for the
    /// non-git callers (tests, `project::add`) that never set them.
    pub worktree: Option<String>,
    pub branch: Option<String>,
}

pub fn insert(conn: &Connection, ctx: &InsertContext) -> Result<String> {
    let id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO contexts (id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, last_accessed_at, worktree, branch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, ?14, ?14, ?14, ?15, ?16)",
        params![
            id,
            ctx.uri,
            ctx.parent_uri,
            ctx.context_type,
            ctx.category,
            ctx.name,
            ctx.abstract_text,
            ctx.overview,
            ctx.content,
            ctx.tags,
            ctx.source_agent,
            ctx.source_session,
            ctx.importance,
            now,
            ctx.worktree,
            ctx.branch,
        ],
    )?;

    Ok(id)
}

pub fn get_by_id(conn: &Connection, id: &str) -> Result<Option<ContextRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by, worktree, branch
         FROM contexts WHERE id = ?1",
    )?;

    let result = stmt
        .query_row(params![id], row_to_context)
        .optional()?;

    Ok(result)
}

pub fn get_by_uri(conn: &Connection, uri: &str) -> Result<Option<ContextRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by, worktree, branch
         FROM contexts WHERE uri = ?1",
    )?;

    let result = stmt
        .query_row(params![uri], row_to_context)
        .optional()?;

    Ok(result)
}

pub fn list_by_parent(conn: &Connection, parent_uri: &str) -> Result<Vec<ContextRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by, worktree, branch
         FROM contexts WHERE parent_uri = ?1 AND superseded_by IS NULL
         ORDER BY importance DESC, created_at DESC",
    )?;

    let rows = stmt
        .query_map(params![parent_uri], row_to_context)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn update(conn: &Connection, id: &str, abstract_text: Option<&str>, overview: Option<&str>, content: Option<&str>, importance: Option<f64>, tags: Option<&str>) -> Result<()> {
    let existing = get_by_id(conn, id)?;
    if existing.is_none() {
        bail!("Context not found: {id}");
    }
    let existing = existing.unwrap();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE contexts SET abstract = ?1, overview = ?2, content = ?3, importance = ?4, tags = ?5, updated_at = ?6 WHERE id = ?7",
        params![
            abstract_text.unwrap_or(&existing.abstract_text),
            overview.unwrap_or(&existing.overview),
            content.unwrap_or(&existing.content),
            importance.unwrap_or(existing.importance),
            tags.unwrap_or(&existing.tags),
            now,
            id,
        ],
    )?;

    Ok(())
}

pub fn supersede(conn: &Connection, old_id: &str, new_id: &str) -> Result<()> {
    let old = get_by_id(conn, old_id)?;
    if old.is_none() {
        bail!("Old context not found: {old_id}");
    }
    let new = get_by_id(conn, new_id)?;
    if new.is_none() {
        bail!("New context not found: {new_id}");
    }

    conn.execute(
        "UPDATE contexts SET superseded_by = ?1, updated_at = ?2 WHERE id = ?3",
        params![new_id, chrono::Utc::now().to_rfc3339(), old_id],
    )?;

    Ok(())
}

/// The six memory categories the schema's CHECK constraint allows.
pub const CATEGORIES: &[&str] = &["preference", "entity", "decision", "event", "case", "pattern"];

/// Reject a category the schema would refuse, with a message that lists the
/// valid ones instead of SQLite's raw `CHECK constraint failed`.
pub fn validate_category(category: &str) -> Result<()> {
    if !CATEGORIES.contains(&category) {
        bail!(
            "Invalid category '{category}'. Valid categories: {}",
            CATEGORIES.join(", ")
        );
    }
    Ok(())
}

/// Reject an importance outside 0.0..=1.0. Ranking multiplies by it, so an
/// unchecked `--importance 7` silently outranked every other memory.
pub fn validate_importance(importance: f64) -> Result<()> {
    if !(0.0..=1.0).contains(&importance) {
        bail!("Invalid importance {importance}: must be between 0.0 and 1.0");
    }
    Ok(())
}

/// Look a memory up by ULID or by `rememora://` URI.
pub fn resolve_ref(conn: &Connection, reference: &str) -> Result<Option<ContextRecord>> {
    if reference.starts_with("rememora://") {
        get_by_uri(conn, reference)
    } else {
        get_by_id(conn, reference)
    }
}

/// Whitespace-collapsed, lowercased text, for exact-duplicate comparison.
pub fn normalise_content(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A live memory under `base_uri` (or a suffixed sibling of it) whose content
/// matches `content` once normalised.
///
/// An exact duplicate always slugifies to the same base URI, so this only has
/// to look at that URI and its `-<suffix>` siblings, both indexed lookups.
pub fn find_live_duplicate(conn: &Connection, base_uri: &str, content: &str) -> Result<Option<ContextRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by, worktree, branch
         FROM contexts
         WHERE (uri = ?1 OR uri LIKE ?2) AND superseded_by IS NULL AND context_type = 'memory'",
    )?;
    let wanted = normalise_content(content);
    let rows = stmt
        .query_map(params![base_uri, format!("{base_uri}-%")], row_to_context)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().find(|r| normalise_content(&r.content) == wanted))
}

/// `base_uri` if no row holds it, otherwise `base_uri` plus a short random
/// suffix. Different memories that share their first 60 characters used to
/// collide on the UNIQUE `uri` column and fail the save outright.
pub fn unique_uri(conn: &Connection, base_uri: &str) -> Result<String> {
    let taken = |uri: &str| -> Result<bool> {
        Ok(conn
            .query_row("SELECT 1 FROM contexts WHERE uri = ?1", params![uri], |_| Ok(()))
            .optional()?
            .is_some())
    };
    if !taken(base_uri)? {
        return Ok(base_uri.to_string());
    }
    loop {
        // The low bits of a ULID are random; eight base32 chars is plenty.
        let ulid = ulid::Ulid::new().to_string().to_lowercase();
        let candidate = format!("{base_uri}-{}", &ulid[ulid.len() - 8..]);
        if !taken(&candidate)? {
            return Ok(candidate);
        }
    }
}

/// What `supersede_checked` did, for the caller to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersedeOutcome {
    pub old_id: String,
    pub new_id: String,
    /// The successor the old memory pointed at before `--force` replaced it.
    pub replaced_successor: Option<String>,
    /// `(old project, new project)` when they differ; `None` is global scope.
    pub cross_project: Option<(Option<String>, Option<String>)>,
}

/// `supersede` with the guards the bare UPDATE never had.
///
/// Refuses a self-supersede and a cycle (either would hide memories from every
/// live query with nothing live left to show for them), and refuses to repoint
/// an already superseded memory unless `force`, since that silently orphans the
/// successor it already had.
pub fn supersede_checked(conn: &Connection, old_ref: &str, new_ref: &str, force: bool) -> Result<SupersedeOutcome> {
    let Some(old) = resolve_ref(conn, old_ref)? else {
        bail!("Old memory not found: {old_ref}");
    };
    let Some(new) = resolve_ref(conn, new_ref)? else {
        bail!("New memory not found: {new_ref}");
    };
    if old.id == new.id {
        bail!("Cannot supersede a memory by itself: {}", old.id);
    }

    // Walk the new memory's successor chain. Reaching `old` means linking
    // old -> new would close a loop.
    let mut cursor = new.superseded_by.clone();
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = cursor {
        if id == old.id {
            bail!(
                "Refusing to create a supersede cycle: {} is already superseded (directly or transitively) by {}",
                new.id,
                old.id
            );
        }
        if !seen.insert(id.clone()) {
            break;
        }
        cursor = get_by_id(conn, &id)?.and_then(|c| c.superseded_by);
    }

    if let Some(existing) = &old.superseded_by {
        if existing != &new.id && !force {
            bail!(
                "{} is already superseded by {existing}. Pass --force to point it at {} instead",
                old.id,
                new.id
            );
        }
    }

    let old_project = crate::uri::extract_project(&old.uri);
    let new_project = crate::uri::extract_project(&new.uri);
    let cross_project = (old_project != new_project).then_some((old_project, new_project));

    supersede(conn, &old.id, &new.id)?;

    Ok(SupersedeOutcome {
        replaced_successor: old.superseded_by.filter(|s| s != &new.id),
        old_id: old.id,
        new_id: new.id,
        cross_project,
    })
}

/// Record a retrieval: increment `active_count` and stamp `last_accessed_at`.
///
/// Deliberately does **not** touch `updated_at` — that column means "when the
/// content last changed". Bumping it here made every search result look freshly
/// written, which inflated the recency term in `hotness.rs` and turned
/// retrieval into its own ranking evidence.
pub fn bump_active_count(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE contexts SET active_count = active_count + 1, last_accessed_at = ?1 WHERE id = ?2",
        params![chrono::Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

/// When this context was last retrieved, if ever recorded.
///
/// Kept off `ContextRecord` for now: that struct is built by literal in several
/// call sites outside this module, so widening it is a separate change.
pub fn last_accessed_at(conn: &Connection, id: &str) -> Result<Option<String>> {
    let result = conn
        .query_row(
            "SELECT last_accessed_at FROM contexts WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?;

    Ok(result.flatten())
}

pub fn list_by_scope(conn: &Connection, context_type: Option<&str>, category: Option<&str>, project: Option<&str>, limit: usize) -> Result<Vec<ContextRecord>> {
    let mut sql = String::from(
        "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by, worktree, branch
         FROM contexts WHERE superseded_by IS NULL",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if let Some(ct) = context_type {
        sql.push_str(&format!(" AND context_type = ?{param_idx}"));
        param_values.push(Box::new(ct.to_string()));
        param_idx += 1;
    }

    if let Some(cat) = category {
        sql.push_str(&format!(" AND category = ?{param_idx}"));
        param_values.push(Box::new(cat.to_string()));
        param_idx += 1;
    }

    if let Some(proj) = project {
        let prefix = format!("rememora://projects/{proj}/");
        sql.push_str(&format!(" AND uri LIKE ?{param_idx}"));
        param_values.push(Box::new(format!("{prefix}%")));
        param_idx += 1;
    }

    let _ = param_idx;
    sql.push_str(" ORDER BY importance DESC, created_at DESC");
    sql.push_str(&format!(" LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(params_ref.as_slice(), row_to_context)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Every live memory row `context` may rank for a scope, unsorted and uncapped.
///
/// - `Some(project)`: the project's memories plus global preferences.
/// - `None`: every memory in the database (Global mode, issue #104).
///
/// Two deliberate differences from `list_by_scope`:
/// - No `ORDER BY importance ... LIMIT`: pre-sorting by importance and capping
///   before hotness scoring starved fresh low-importance memories out of the
///   candidate set. The caller scores in Rust; rows come back in id (ULID)
///   order so ties stay deterministic.
/// - The project filter is an exact prefix comparison with `substr`, not
///   `LIKE`, so `_` and `%` in a project name are literal characters
///   (`my_app` must not match `myXapp`).
pub fn list_memories_in_scope(conn: &Connection, project: Option<&str>) -> Result<Vec<ContextRecord>> {
    const COLS: &str = "SELECT id, uri, parent_uri, context_type, category, name, abstract, overview, content, tags, source_agent, source_session, importance, active_count, created_at, updated_at, superseded_by, worktree, branch
         FROM contexts WHERE superseded_by IS NULL AND context_type = 'memory'";
    let rows = match project {
        Some(proj) => {
            let prefix = format!("rememora://projects/{proj}/");
            let sql = format!(
                "{COLS} AND (substr(uri, 1, length(?1)) = ?1
                     OR (substr(uri, 1, length(?2)) = ?2 AND category = 'preference'))
                 ORDER BY id"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![prefix, "rememora://global/"], row_to_context)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        }
        None => {
            let mut stmt = conn.prepare(&format!("{COLS} ORDER BY id"))?;
            let rows = stmt
                .query_map([], row_to_context)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        }
    };
    Ok(rows)
}

fn row_to_context(row: &rusqlite::Row) -> rusqlite::Result<ContextRecord> {
    Ok(ContextRecord {
        id: row.get(0)?,
        uri: row.get(1)?,
        parent_uri: row.get(2)?,
        context_type: row.get(3)?,
        category: row.get(4)?,
        name: row.get(5)?,
        abstract_text: row.get(6)?,
        overview: row.get(7)?,
        content: row.get(8)?,
        tags: row.get(9)?,
        source_agent: row.get(10)?,
        source_session: row.get(11)?,
        importance: row.get(12)?,
        active_count: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        superseded_by: row.get(16)?,
        worktree: row.get(17)?,
        branch: row.get(18)?,
    })
}

use rusqlite::OptionalExtension;
