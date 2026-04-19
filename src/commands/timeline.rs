//! `rememora timeline` — chronological slice of contexts around an anchor URI.
//!
//! Thin CLI wrapper over [`rememora::timeline::build_timeline`]. Library-side
//! logic lives in `src/timeline.rs` so behavior tests can exercise it without
//! pulling the binary crate.

use anyhow::Result;
use rusqlite::Connection;

use rememora::models::context::{self, ContextRecord};
use rememora::timeline::{self, Timeline};

// Re-export so `main.rs` can stay on the `commands::timeline::*` path.
pub use rememora::timeline::{TimelineArgs, TimelineOrder};

pub fn run(conn: &Connection, args: &TimelineArgs, json: bool) -> Result<()> {
    let t = timeline::build_timeline(conn, args)?;

    // Match the `get` command's side effect: accessing the anchor bumps its
    // active_count, which feeds hotness on future searches.
    context::bump_active_count(conn, &t.anchor.id)?;

    if json {
        println!("{}", to_json(&t));
    } else {
        print!("{}", to_markdown(&t));
    }

    Ok(())
}

fn compact_line(ctx: &ContextRecord) -> String {
    let cat = ctx.category.as_deref().unwrap_or(&ctx.context_type);
    let abstract_text = if !ctx.abstract_text.is_empty() {
        ctx.abstract_text.as_str()
    } else {
        ctx.name.as_str()
    };
    format!(
        "- [{}] {} — `{}` ({})",
        cat, abstract_text, ctx.uri, ctx.created_at
    )
}

fn to_markdown(t: &Timeline) -> String {
    let mut md = String::new();
    md.push_str(&format!("# Timeline around `{}`\n\n", t.anchor.uri));

    if !t.before.is_empty() {
        md.push_str("## Before\n\n");
        for ctx in &t.before {
            md.push_str(&compact_line(ctx));
            md.push('\n');
        }
        md.push('\n');
    }

    md.push_str("## Anchor\n\n");
    md.push_str(&format!("**{}**\n", compact_line(&t.anchor)));
    md.push('\n');

    if !t.after.is_empty() {
        md.push_str("## After\n\n");
        for ctx in &t.after {
            md.push_str(&compact_line(ctx));
            md.push('\n');
        }
        md.push('\n');
    }

    md
}

fn to_json(t: &Timeline) -> String {
    let render = |c: &ContextRecord| {
        serde_json::json!({
            "id": c.id,
            "uri": c.uri,
            "name": c.name,
            "category": c.category,
            "abstract": c.abstract_text,
            "importance": c.importance,
            "created_at": c.created_at,
            "active_count": c.active_count,
        })
    };

    let payload = serde_json::json!({
        "anchor": render(&t.anchor),
        "before": t.before.iter().map(render).collect::<Vec<_>>(),
        "after": t.after.iter().map(render).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&payload).unwrap_or_default()
}
