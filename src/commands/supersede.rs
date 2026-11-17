use anyhow::{bail, Result};
use rusqlite::Connection;

use rememora::models::context::{self, SupersedeOutcome};
use rememora::uri;

use super::save::{self, SaveArgs};

/// What replaces the old memory: an existing one, or text to save now.
pub enum Replacement {
    Existing(String),
    New(NewText),
}

/// `supersede --text`: replacement text plus optional overrides. Anything left
/// `None` is inherited from the memory being superseded.
#[derive(Default)]
pub struct NewText {
    pub text: String,
    pub category: Option<String>,
    pub project: Option<String>,
    pub importance: Option<f64>,
    pub tags: Option<String>,
    pub agent: Option<String>,
}

pub fn run(conn: &Connection, old_ref: &str, replacement: Replacement, force: bool, json: bool) -> Result<()> {
    let outcome = apply(conn, old_ref, replacement, force)?;

    if let Some((old_p, new_p)) = &outcome.cross_project {
        eprintln!(
            "warning: superseding across projects ({} -> {})",
            old_p.as_deref().unwrap_or("global"),
            new_p.as_deref().unwrap_or("global")
        );
    }
    if let Some(prev) = &outcome.replaced_successor {
        eprintln!("note: --force replaced the previous successor {prev}");
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": "ok",
                "old_id": outcome.old_id,
                "new_id": outcome.new_id,
                "replaced_successor": outcome.replaced_successor,
            })
        );
    } else {
        println!("Superseded: {} → {}", outcome.old_id, outcome.new_id);
    }

    Ok(())
}

/// Link `old_ref` to its replacement. With `Replacement::New` the save and the
/// link share one transaction, so a refused link leaves no orphan memory.
pub fn apply(conn: &Connection, old_ref: &str, replacement: Replacement, force: bool) -> Result<SupersedeOutcome> {
    let new_text = match replacement {
        Replacement::Existing(new_ref) => return context::supersede_checked(conn, old_ref, &new_ref, force),
        Replacement::New(n) => n,
    };

    let Some(old) = context::resolve_ref(conn, old_ref)? else {
        bail!("Old memory not found: {old_ref}");
    };

    let (project, project_is_canonical) = match new_text.project {
        Some(p) => (Some(p), false),
        None => (uri::extract_project(&old.uri), true),
    };
    let args = SaveArgs {
        text: new_text.text,
        category: new_text
            .category
            .or(old.category.clone())
            .unwrap_or_else(|| "entity".to_string()),
        project,
        importance: new_text.importance.unwrap_or(old.importance),
        agent: new_text.agent.or(old.source_agent.clone()),
        tags: new_text.tags.or(Some(old.tags.clone())),
        project_is_canonical,
        ..Default::default()
    };

    let tx = conn.unchecked_transaction()?;
    let saved = save::save(&tx, &args)?;
    if saved.id == old.id {
        bail!("The replacement text is identical to {}; nothing to supersede", old.id);
    }
    let outcome = context::supersede_checked(&tx, &old.id, &saved.id, force)?;
    tx.commit()?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rememora::db;

    fn save_one(conn: &Connection, text: &str, project: Option<&str>) -> String {
        save::save(
            conn,
            &SaveArgs {
                text: text.into(),
                category: "decision".into(),
                project: project.map(Into::into),
                importance: 0.7,
                tags: Some(r#"["db"]"#.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn text_form_saves_and_links_inheriting_scope() {
        let conn = db::open_memory().unwrap();
        let old = save_one(&conn, "Use WAL mode", Some("p"));

        let out = apply(&conn, &old, Replacement::New(NewText { text: "Use WAL mode with NORMAL sync".into(), ..Default::default() }), false).unwrap();

        let new = context::get_by_id(&conn, &out.new_id).unwrap().unwrap();
        assert!(new.uri.starts_with("rememora://projects/p/memories/decision/"));
        assert_eq!(new.importance, 0.7);
        assert_eq!(new.tags, r#"["db"]"#);
        assert_eq!(context::get_by_id(&conn, &old).unwrap().unwrap().superseded_by, Some(out.new_id));
    }

    #[test]
    fn text_form_rolls_back_when_the_link_is_refused() {
        let conn = db::open_memory().unwrap();
        let old = save_one(&conn, "Use WAL mode", Some("p"));
        let other = save_one(&conn, "Use rollback journal", Some("p"));
        context::supersede_checked(&conn, &old, &other, false).unwrap();

        let err = apply(&conn, &old, Replacement::New(NewText { text: "Use WAL2".into(), ..Default::default() }), false).unwrap_err();
        assert!(err.to_string().contains("already superseded"));
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM contexts WHERE content = 'Use WAL2'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "the replacement must not survive a refused link");
    }

    #[test]
    fn text_form_refuses_identical_text() {
        let conn = db::open_memory().unwrap();
        let old = save_one(&conn, "Use WAL mode", Some("p"));
        let err = apply(&conn, &old, Replacement::New(NewText { text: "use  WAL mode".into(), ..Default::default() }), false).unwrap_err();
        assert!(err.to_string().contains("identical"));
    }

    #[test]
    fn global_old_memory_stays_global() {
        let conn = db::open_memory().unwrap();
        let old = save_one(&conn, "Prefer tabs", None);
        let out = apply(&conn, &old, Replacement::New(NewText { text: "Prefer spaces".into(), ..Default::default() }), false).unwrap();
        assert!(out.cross_project.is_none());
        let new = context::get_by_id(&conn, &out.new_id).unwrap().unwrap();
        assert!(new.uri.starts_with("rememora://global/"));
    }
}
