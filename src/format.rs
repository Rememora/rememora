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

/// Target line length for compact search output (~75 tokens).
const COMPACT_LINE_LEN: usize = 140;
/// Overall byte cap for context-mode search output (prompt injection safety).
const CONTEXT_BYTE_CAP: usize = 1200;
/// Per-line abstract length for context-mode.
const CONTEXT_LINE_LEN: usize = 100;

fn truncate_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn pick_abstract(ctx: &crate::models::context::ContextRecord) -> &str {
    if !ctx.abstract_text.is_empty() {
        &ctx.abstract_text
    } else {
        &ctx.name
    }
}

/// Compact progressive-disclosure output: one line per hit, ~75 tokens each.
///
/// Shape: `[category] abstract — rememora://... (rank=-1.23)`
///
/// Designed for agents to filter before fetching: enough signal to decide
/// whether to drill into `timeline` or `get`, without spending the token
/// budget that the default markdown format would.
pub fn search_results_to_compact(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No results found.\n".to_string();
    }

    let mut out = String::new();
    for result in results {
        let ctx = &result.context;
        let cat = ctx.category.as_deref().unwrap_or(&ctx.context_type);
        let abstract_text = pick_abstract(ctx);
        // Budget for the abstract = target line length minus the fixed overhead
        // (category tag, separator, URI, rank). Clamp to a floor so short URIs
        // don't yield absurdly long abstracts.
        let overhead = cat.len() + ctx.uri.len() + 20;
        let abs_budget = COMPACT_LINE_LEN.saturating_sub(overhead).max(40);
        let abs_short = truncate_ellipsis(abstract_text, abs_budget);
        out.push_str(&format!(
            "[{}] {} — {} (rank={:.2})\n",
            cat, abs_short, ctx.uri, result.rank
        ));
    }

    out
}

/// Length-capped context output for inline prompt injection.
///
/// Shape: `[category] short-abstract` lines. Overall byte count is bounded by
/// `CONTEXT_BYTE_CAP` so that a misbehaving DB cannot blow a prompt hook's
/// context budget.
pub fn search_results_to_context(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for result in results {
        let ctx = &result.context;
        let cat = ctx.category.as_deref().unwrap_or(&ctx.context_type);
        let abstract_text = pick_abstract(ctx);
        let abs_short = truncate_ellipsis(abstract_text, CONTEXT_LINE_LEN);
        let line = format!("[{}] {}\n", cat, abs_short);
        if out.len() + line.len() > CONTEXT_BYTE_CAP {
            break;
        }
        out.push_str(&line);
    }

    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn result_with(uri: &str, category: &str, abstract_text: &str) -> SearchResult {
        SearchResult {
            context: ContextRecord {
                id: uri.to_string(),
                uri: uri.to_string(),
                parent_uri: None,
                context_type: "preference".to_string(),
                category: Some(category.to_string()),
                name: "name".to_string(),
                abstract_text: abstract_text.to_string(),
                overview: String::new(),
                content: String::new(),
                tags: String::new(),
                source_agent: None,
                source_session: None,
                importance: 0.5,
                active_count: 0,
                created_at: String::new(),
                updated_at: String::new(),
                superseded_by: None,
            },
            rank: 1.0,
        }
    }

    #[test]
    fn context_format_empty_results_returns_empty_string() {
        let out = search_results_to_context(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn context_format_caps_total_output_under_byte_budget() {
        // Many long-abstract results; ensure we never exceed CONTEXT_BYTE_CAP.
        let results: Vec<SearchResult> = (0..50)
            .map(|i| result_with(&format!("rememora://a{i}"), "case", &"A".repeat(300)))
            .collect();
        let out = search_results_to_context(&results);
        assert!(
            out.len() <= CONTEXT_BYTE_CAP,
            "expected output under cap, got {} bytes",
            out.len()
        );
    }

    #[test]
    fn context_format_uses_context_type_when_category_missing() {
        let mut r = result_with("rememora://x", "_unused_", "abs");
        r.context.category = None;
        r.context.context_type = "pattern".into();
        let out = search_results_to_context(std::slice::from_ref(&r));
        assert!(
            out.contains("[pattern]"),
            "expected fallback category [pattern] in: {out}"
        );
    }

    #[test]
    fn context_format_truncates_long_abstracts_with_ellipsis() {
        let long = "x".repeat(500);
        let r = result_with("rememora://foo", "decision", &long);
        let out = search_results_to_context(std::slice::from_ref(&r));
        assert!(out.contains('…'), "expected ellipsis in: {out}");
    }
}
