use crate::hierarchy::{ContextAssembly, ScoredContext};
use crate::models::context::ContextRecord;
use crate::models::session::SessionRecord;
use crate::search::SearchResult;

pub fn context_to_markdown(assembly: &ContextAssembly) -> String {
    let mut md = String::new();

    // Header
    if let Some(ref proj) = assembly.project_name {
        md.push_str(&format!("# Rememora Context: {proj}\n\n"));
    } else {
        md.push_str("# Rememora Context: Global\n\n");
    }

    // Latest session info
    if let Some(ref session) = assembly.latest_session {
        md.push_str("## Last Session\n\n");
        md.push_str(&format!("- **Agent**: {}\n", session.agent));
        md.push_str(&format!("- **Status**: {}\n", session.status));
        if !session.intent.is_empty() {
            md.push_str(&format!("- **Intent**: {}\n", session.intent));
        }
        if !session.summary.is_empty() {
            md.push_str(&format!("- **Summary**: {}\n", session.summary));
        }
        if !session.working_state.is_empty() {
            md.push_str(&format!("\n### Working State\n\n{}\n", session.working_state));
        }
        md.push('\n');
    }

    // L0 abstracts — memory map
    if !assembly.l0_abstracts.is_empty() {
        md.push_str("## Memory Map (L0)\n\n");
        for scored in &assembly.l0_abstracts {
            let cat = scored
                .context
                .category
                .as_deref()
                .unwrap_or(&scored.context.context_type);
            md.push_str(&format!(
                "- [{}] {} (importance: {:.1})\n",
                cat, scored.context.abstract_text, scored.context.importance
            ));
        }
        md.push('\n');
    }

    // L1 overviews — top memories with detail
    if !assembly.l1_overviews.is_empty() {
        md.push_str("## Key Context (L1)\n\n");
        for scored in &assembly.l1_overviews {
            if scored.context.overview.is_empty() {
                continue;
            }
            let cat = scored
                .context
                .category
                .as_deref()
                .unwrap_or(&scored.context.context_type);
            md.push_str(&format!("### [{}] {}\n\n", cat, scored.context.name));
            md.push_str(&format!("{}\n\n", scored.context.overview));
        }
    }

    if assembly.l0_abstracts.is_empty() && assembly.latest_session.is_none() {
        md.push_str("*No memories or sessions found for this project.*\n");
    }

    md
}

pub fn search_results_to_markdown(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No results found.\n".to_string();
    }

    let mut md = String::new();
    md.push_str(&format!("## Search Results ({} found)\n\n", results.len()));

    for (i, result) in results.iter().enumerate() {
        let ctx = &result.context;
        let cat = ctx.category.as_deref().unwrap_or(&ctx.context_type);
        md.push_str(&format!("{}. **[{}] {}**\n", i + 1, cat, ctx.name));
        if !ctx.abstract_text.is_empty() {
            md.push_str(&format!("   {}\n", ctx.abstract_text));
        }
        md.push_str(&format!("   URI: `{}`  |  ID: `{}`\n\n", ctx.uri, ctx.id));
    }

    md
}

pub fn context_record_to_markdown(ctx: &ContextRecord) -> String {
    let mut md = String::new();
    let cat = ctx.category.as_deref().unwrap_or(&ctx.context_type);

    md.push_str(&format!("# [{}] {}\n\n", cat, ctx.name));
    md.push_str(&format!("- **URI**: `{}`\n", ctx.uri));
    md.push_str(&format!("- **ID**: `{}`\n", ctx.id));
    md.push_str(&format!("- **Importance**: {:.1}\n", ctx.importance));
    md.push_str(&format!("- **Access count**: {}\n", ctx.active_count));
    if let Some(ref agent) = ctx.source_agent {
        md.push_str(&format!("- **Source agent**: {agent}\n"));
    }
    md.push_str(&format!("- **Created**: {}\n", ctx.created_at));
    md.push_str(&format!("- **Updated**: {}\n", ctx.updated_at));

    if !ctx.abstract_text.is_empty() {
        md.push_str(&format!("\n## Abstract (L0)\n\n{}\n", ctx.abstract_text));
    }
    if !ctx.overview.is_empty() {
        md.push_str(&format!("\n## Overview (L1)\n\n{}\n", ctx.overview));
    }
    if !ctx.content.is_empty() {
        md.push_str(&format!("\n## Content (L2)\n\n{}\n", ctx.content));
    }

    md
}

pub fn session_to_markdown(session: &SessionRecord) -> String {
    let mut md = String::new();
    md.push_str(&format!("# Session: {}\n\n", session.id));
    md.push_str(&format!("- **Agent**: {}\n", session.agent));
    md.push_str(&format!("- **Status**: {}\n", session.status));
    if let Some(ref proj) = session.project {
        md.push_str(&format!("- **Project**: {proj}\n"));
    }
    md.push_str(&format!("- **Started**: {}\n", session.started_at));
    if let Some(ref ended) = session.ended_at {
        md.push_str(&format!("- **Ended**: {ended}\n"));
    }
    if !session.intent.is_empty() {
        md.push_str(&format!("\n## Intent\n\n{}\n", session.intent));
    }
    if !session.summary.is_empty() {
        md.push_str(&format!("\n## Summary\n\n{}\n", session.summary));
    }
    if !session.working_state.is_empty() {
        md.push_str(&format!("\n## Working State\n\n{}\n", session.working_state));
    }
    md
}

pub fn search_results_to_json(results: &[SearchResult]) -> String {
    let items: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.context.id,
                "uri": r.context.uri,
                "name": r.context.name,
                "type": r.context.context_type,
                "category": r.context.category,
                "abstract": r.context.abstract_text,
                "importance": r.context.importance,
                "rank": r.rank,
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}

pub fn context_record_to_json(ctx: &ContextRecord) -> String {
    serde_json::to_string_pretty(ctx).unwrap_or_default()
}

pub fn scored_contexts_to_json(contexts: &[ScoredContext]) -> String {
    let items: Vec<serde_json::Value> = contexts
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.context.id,
                "uri": s.context.uri,
                "name": s.context.name,
                "type": s.context.context_type,
                "category": s.context.category,
                "abstract": s.context.abstract_text,
                "overview": s.context.overview,
                "importance": s.context.importance,
                "score": s.score,
            })
        })
        .collect();
    serde_json::to_string_pretty(&items).unwrap_or_default()
}
