use crate::hierarchy::{ContextAssembly, ScoredContext};
use crate::models::context::ContextRecord;
use crate::models::session::SessionRecord;
use crate::search::SearchResult;

/// Default `context --max-tokens`. 2500 tokens holds a handoff, a few active
/// sessions, five relevant and five recent memories, ten top memories in full
/// and about forty one-line abstracts: enough to orient an agent at session
/// start, while staying around 1% of a 200k window however large the memory
/// base grows. Anything beyond it is one `rememora search` away.
pub const DEFAULT_CONTEXT_MAX_TOKENS: usize = 2500;
/// How many of the ranked memories are printed in full before the rest drop
/// to one-line abstracts.
pub const CONTEXT_TOP_FULL: usize = 10;
/// Handoffs older than this many days carry an explicit age warning.
pub const STALE_HANDOFF_DAYS: i64 = 14;
const CONTEXT_ABSTRACT_LEN: usize = 120;

/// The one token estimator `context` uses: characters / 4, rounded up.
pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

/// Indices into each `ContextAssembly` section that made it into the output.
#[derive(Debug, Default)]
pub struct ContextShown {
    pub relevant: Vec<usize>,
    pub recent: Vec<usize>,
    pub top: Vec<usize>,
    pub abstracts: Vec<usize>,
}

pub struct RenderedContext {
    pub markdown: String,
    /// `estimate_tokens(&markdown)`.
    pub tokens_used: usize,
    /// Memories in scope that were left out to fit the budget.
    pub omitted: usize,
    pub shown: ContextShown,
    pub max_tokens: Option<usize>,
}

struct Budget {
    out: String,
    limit: Option<usize>,
    reserve: usize,
}

impl Budget {
    fn fits(&self, s: &str) -> bool {
        match self.limit {
            None => true,
            Some(n) => {
                let mut t = self.out.clone();
                t.push_str(s);
                estimate_tokens(&t) + self.reserve <= n
            }
        }
    }
    fn try_push(&mut self, s: &str) -> bool {
        if self.fits(s) {
            self.out.push_str(s);
            true
        } else {
            false
        }
    }
    /// Push `s`, cutting it short (with an ellipsis) if only part fits.
    fn push_truncated(&mut self, s: &str) {
        if self.try_push(s) {
            return;
        }
        let Some(n) = self.limit else { return };
        let room_chars = n.saturating_sub(self.reserve) * 4;
        let used = self.out.chars().count();
        let mut avail = room_chars.saturating_sub(used + 1);
        while avail > 8 {
            let mut cut = truncate_ellipsis(s, avail);
            cut.push('\n');
            if self.try_push(&cut) {
                return;
            }
            avail -= 4;
        }
    }
}

/// Said once in the footer instead of a full URI on every memory line.
pub const CONTEXT_GET_HINT: &str = "`rememora get <id>` shows the full memory.";

fn context_footer(used: usize, limit: Option<usize>, omitted: usize, has_memories: bool) -> String {
    let budget = match limit {
        Some(n) => format!("budget {n}"),
        None => "no budget".to_string(),
    };
    let get_hint = if has_memories { format!(" {CONTEXT_GET_HINT}") } else { String::new() };
    if omitted == 0 {
        format!("_~{used} tokens ({budget}), nothing omitted.{get_hint}_\n")
    } else {
        format!(
            "_~{used} tokens ({budget}), {omitted} memories omitted. Find them with `rememora search \"<topic>\"`.{get_hint}_\n"
        )
    }
}

fn human_age(ts: &str) -> Option<(String, i64)> {
    let t = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
    let days = secs / 86_400;
    let label = if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{days}d ago")
    };
    Some((label, days))
}

/// One line describing an active session: agent, intent, start time and id.
pub fn active_session_line(s: &SessionRecord) -> String {
    let intent = if s.intent.is_empty() { "(no intent)" } else { &s.intent };
    let age = human_age(&s.started_at)
        .map(|(a, _)| a)
        .unwrap_or_else(|| s.started_at.clone());
    let mut line = format!("- {}: {} (started {}, `{}`", s.agent, intent, age, s.id);
    if let Some(ref b) = s.branch {
        line.push_str(&format!(", branch {b}"));
    }
    line.push(')');
    line
}

/// The `## Handoff` section: who handed off, how long ago (with a note past
/// `STALE_HANDOFF_DAYS`), intent, summary and working state. Shared by
/// `context` and `context --cheatsheet`.
pub fn handoff_block(h: &SessionRecord) -> String {
    let mut md = String::from("## Handoff\n\n");
    let when = h.ended_at.as_deref().unwrap_or(&h.started_at);
    let age = human_age(when);
    let age_label = age.as_ref().map(|(a, _)| a.as_str()).unwrap_or("at an unknown time");
    md.push_str(&format!(
        "- From: {}, {} {} (`{}`)\n",
        h.agent, h.status, age_label, h.id
    ));
    if let Some((_, days)) = age {
        if days > STALE_HANDOFF_DAYS {
            md.push_str(&format!(
                "- Note: this handoff is {days} days old, check it is still current.\n"
            ));
        }
    }
    if !h.intent.is_empty() {
        md.push_str(&format!("- Intent: {}\n", h.intent));
    }
    if !h.summary.is_empty() {
        md.push_str(&format!("- Summary: {}\n", h.summary));
    }
    if !h.working_state.is_empty() {
        md.push_str(&format!("- Working state: {}\n", h.working_state));
    }
    md.push('\n');
    md
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn memory_tag(sc: &ScoredContext, global_mode: bool) -> String {
    let cat = sc.context.category.as_deref().unwrap_or(&sc.context.context_type);
    if global_mode {
        format!("[{}] [{}]", project_label_for(&sc.context.uri), cat)
    } else {
        format!("[{cat}]")
    }
}

fn full_text(ctx: &ContextRecord) -> &str {
    if !ctx.overview.is_empty() {
        &ctx.overview
    } else if !ctx.abstract_text.is_empty() {
        &ctx.abstract_text
    } else {
        &ctx.name
    }
}

fn full_line(sc: &ScoredContext, global_mode: bool) -> String {
    format!(
        "- {} {} `{}`\n",
        memory_tag(sc, global_mode),
        one_line(full_text(&sc.context)),
        sc.context.id
    )
}

fn abstract_line(sc: &ScoredContext, global_mode: bool, suffix: &str) -> String {
    let text = one_line(pick_abstract(&sc.context));
    format!(
        "- {} {}{} `{}`\n",
        memory_tag(sc, global_mode),
        truncate_ellipsis(&text, CONTEXT_ABSTRACT_LEN),
        suffix,
        sc.context.id
    )
}

/// Fill a section: push its heading with the first item that fits, then every
/// further item that fits. Returns the indices shown.
fn fill_section(b: &mut Budget, heading: &str, lines: &[String]) -> Vec<usize> {
    let mut shown = Vec::new();
    let mut opened = false;
    for (i, line) in lines.iter().enumerate() {
        let ok = if opened {
            b.try_push(line)
        } else {
            let first = format!("{heading}{line}");
            let ok = b.try_push(&first);
            opened = ok;
            ok
        };
        if ok {
            shown.push(i);
        }
    }
    if opened {
        b.out.push('\n');
    }
    shown
}

/// Render `context` as markdown within `max_tokens` (by `estimate_tokens`).
///
/// Priority: header, handoff, active sessions, relevant, recent, top memories
/// in full, then one-line abstracts by score. A footer reports the tokens used
/// and how many memories were omitted. The whole output never exceeds the
/// budget; `None` means unlimited.
pub fn render_context(assembly: &ContextAssembly, max_tokens: Option<usize>) -> RenderedContext {
    let global_mode = assembly.project_name.is_none();
    let total = assembly.memory_count();
    let mut b = Budget { out: String::new(), limit: max_tokens, reserve: 0 };
    // Keep room for the footer at its widest (largest numbers it can print).
    let widest = context_footer(max_tokens.unwrap_or(0).max(99_999), max_tokens, total.max(1), true);
    let footer_room = estimate_tokens(&widest) + 1;
    let footer_fits = max_tokens.is_none_or(|n| n > footer_room);
    if footer_fits && max_tokens.is_some() {
        b.reserve = footer_room;
    }

    // Header, with the project registry row folded in once.
    let mut header = match assembly.project_name {
        Some(ref p) => format!("# Rememora Context: {p}\n\n"),
        None => "# Rememora Context: Global\n\n".to_string(),
    };
    if let Some(ref info) = assembly.project_info {
        let mut line = String::new();
        if !info.description.is_empty() {
            line.push_str(info.description.trim_end_matches('.'));
            line.push('.');
        }
        if !info.tech_stack.is_empty() {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&format!("Stack: {}.", info.tech_stack.join(", ")));
        }
        if !line.is_empty() {
            header.push_str(&line);
            header.push_str("\n\n");
        }
    }
    b.push_truncated(&header);

    if let Some(ref h) = assembly.handoff {
        b.push_truncated(&handoff_block(h));
    }
    if !assembly.active_sessions.is_empty() {
        let lines: Vec<String> = assembly
            .active_sessions
            .iter()
            .map(|s| format!("{}\n", active_session_line(s)))
            .collect();
        fill_section(&mut b, "## Active sessions\n\n", &lines);
    }

    let mut shown = ContextShown::default();
    if let Some(ref q) = assembly.query {
        let lines: Vec<String> = assembly.relevant.iter().map(|s| full_line(s, global_mode)).collect();
        let src = if assembly.query_from_session { " (from active session)" } else { "" };
        if lines.is_empty() {
            b.try_push(&format!("## Relevant to: {q}{src}\n\nNo matching memories.\n\n"));
        } else {
            shown.relevant = fill_section(&mut b, &format!("## Relevant to: {q}{src}\n\n"), &lines);
        }
    }

    let lines: Vec<String> = assembly
        .recent
        .iter()
        .map(|s| {
            let t = if s.context.updated_at > s.context.created_at { &s.context.updated_at } else { &s.context.created_at };
            let age = human_age(t).map(|(a, _)| format!(" ({a})")).unwrap_or_default();
            abstract_line(s, global_mode, &age)
        })
        .collect();
    shown.recent = fill_section(&mut b, "## Recent (last 7 days)\n\n", &lines);

    let split = assembly.ranked.len().min(CONTEXT_TOP_FULL);
    let lines: Vec<String> = assembly.ranked[..split].iter().map(|s| full_line(s, global_mode)).collect();
    shown.top = fill_section(&mut b, "## Top memories\n\n", &lines);
    // A top memory that did not fit in full still gets a chance as an abstract.
    let mut tail: Vec<usize> = (0..split).filter(|i| !shown.top.contains(i)).collect();
    tail.extend(split..assembly.ranked.len());
    let lines: Vec<String> = tail.iter().map(|&i| abstract_line(&assembly.ranked[i], global_mode, "")).collect();
    shown.abstracts = fill_section(&mut b, "## More memories\n\n", &lines)
        .into_iter()
        .map(|k| tail[k])
        .collect();

    let shown_count = shown.relevant.len() + shown.recent.len() + shown.top.len() + shown.abstracts.len();
    let omitted = total - shown_count;

    if total == 0 && assembly.handoff.is_none() && assembly.active_sessions.is_empty() {
        let msg = if global_mode {
            "*No memories found in the database.*\n"
        } else {
            "*No memories or sessions found for this project.*\n"
        };
        b.try_push(msg);
    }

    let mut markdown = b.out;
    if footer_fits {
        // Fixed point: the footer states the token count of the output that
        // includes the footer itself.
        let mut used = estimate_tokens(&markdown);
        for _ in 0..4 {
            let candidate = format!("{markdown}{}", context_footer(used, max_tokens, omitted, total > 0));
            let actual = estimate_tokens(&candidate);
            if actual == used {
                break;
            }
            used = actual;
        }
        markdown.push_str(&context_footer(used, max_tokens, omitted, total > 0));
    }
    let tokens_used = estimate_tokens(&markdown);
    RenderedContext { markdown, tokens_used, omitted, shown, max_tokens }
}

/// Unbudgeted markdown (kept for callers that want everything).
pub fn context_to_markdown(assembly: &ContextAssembly) -> String {
    render_context(assembly, None).markdown
}

fn context_item_json(s: &ScoredContext) -> serde_json::Value {
    serde_json::json!({
        "id": s.context.id,
        "uri": s.context.uri,
        "category": s.context.category,
        "name": s.context.name,
        "abstract": s.context.abstract_text,
        "overview": s.context.overview,
        "importance": s.context.importance,
        "score": s.score,
        "created_at": s.context.created_at,
        "updated_at": s.context.updated_at,
    })
}

fn context_session_json(s: &SessionRecord) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "agent": s.agent,
        "status": s.status,
        "intent": s.intent,
        "summary": s.summary,
        "working_state": s.working_state,
        "started_at": s.started_at,
        "ended_at": s.ended_at,
        "branch": s.branch,
    })
}

/// JSON with the same sections and the same budget selection as the
/// markdown. `tokens_used` is the markdown estimate.
pub fn context_to_json(assembly: &ContextAssembly, max_tokens: Option<usize>) -> String {
    let r = render_context(assembly, max_tokens);
    let pick = |items: &[ScoredContext], idx: &[usize]| -> Vec<serde_json::Value> {
        idx.iter().map(|&i| context_item_json(&items[i])).collect()
    };
    let project = assembly.project_name.as_ref().map(|name| {
        serde_json::json!({
            "name": name,
            "description": assembly.project_info.as_ref().map(|i| i.description.clone()),
            "stack": assembly.project_info.as_ref().map(|i| i.tech_stack.clone()).unwrap_or_default(),
        })
    });
    let v = serde_json::json!({
        "project": project,
        "handoff": assembly.handoff.as_ref().map(context_session_json),
        "active_sessions": assembly.active_sessions.iter().map(context_session_json).collect::<Vec<_>>(),
        "query": assembly.query,
        "relevant": pick(&assembly.relevant, &r.shown.relevant),
        "recent": pick(&assembly.recent, &r.shown.recent),
        "top": pick(&assembly.ranked, &r.shown.top),
        "abstracts": pick(&assembly.ranked, &r.shown.abstracts),
        "tokens_used": r.tokens_used,
        "max_tokens": r.max_tokens,
        "omitted": r.omitted,
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Render a short label for the project a context belongs to, used in
/// Global-mode context output. Falls back to `global` for global-scoped
/// memories and `?` for unparseable URIs.
fn project_label_for(uri: &str) -> String {
    if let Some(name) = crate::uri::extract_project(uri) {
        return name;
    }
    if uri.starts_with("rememora://global/") {
        return "global".to_string();
    }
    "?".to_string()
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

/// Characters of abstract kept per compact search line. Long enough that a
/// one-sentence memory survives whole, short enough to stay one line.
const COMPACT_ABSTRACT_LEN: usize = 120;
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
/// Shape: `[category] abstract — rememora://... (id <ULID>, rank=-1.23)`
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
        // A fixed abstract budget: deriving it from the URI length cut every
        // project memory down to 40 characters, too little to judge a hit by.
        let abs_short = truncate_ellipsis(abstract_text, COMPACT_ABSTRACT_LEN);
        out.push_str(&format!(
            "[{}] {} — {} (id {}, rank={:.2})\n",
            cat, abs_short, ctx.uri, ctx.id, result.rank
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

/// JSON search hits with enough fields to act on without a `get` per hit.
/// `content` (L2, possibly long) is included only when `full` is set.
pub fn search_results_to_json(results: &[SearchResult], full: bool) -> String {
    let items: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            let mut v = serde_json::json!({
                "id": r.context.id,
                "uri": r.context.uri,
                "project": crate::uri::extract_project(&r.context.uri),
                "name": r.context.name,
                "type": r.context.context_type,
                "category": r.context.category,
                "source_agent": r.context.source_agent,
                "abstract": r.context.abstract_text,
                "overview": r.context.overview,
                "importance": r.context.importance,
                "created_at": r.context.created_at,
                "updated_at": r.context.updated_at,
                "rank": r.rank,
            });
            if full {
                v["content"] = serde_json::Value::String(r.context.content.clone());
            }
            v
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
                worktree: None,
                branch: None,
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
