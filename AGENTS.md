# Rememora

Cross-agent memory system. Rust CLI backed by SQLite.

## Architecture

- Unified `contexts` table (OpenViking pattern) with URI-based hierarchy
- 6 memory categories: preference, entity, decision, event, case, pattern
- L0/L1/L2 tiered loading (abstract/overview/content)
- Hotness scoring: sigmoid(log1p(access_count)) * exp(-age/half_life)
- Sessions with transfer chains for cross-agent continuity
- FTS5 full-text search

## Key files

- `src/main.rs` — CLI entry point (clap)
- `src/db.rs` — SQLite connection, WAL, migrations
- `src/uri.rs` — rememora:// URI parsing
- `src/models/context.rs` — Context CRUD + FTS5
- `src/models/session.rs` — Session lifecycle
- `src/hierarchy.rs` — L0/L1 context assembly
- `src/search.rs` — BM25 search
- `src/hotness.rs` — Scoring
- `src/embed/mod.rs` — EmbedBackend trait (future: candle, llama.cpp)

## Commands

```bash
cargo test                    # Run all tests
cargo build --release         # Build release binary
cargo install --path .        # Install globally
```

## DB location

`~/.rememora/rememora.db`

## Agent runtime

- Shared repo-owned agent runtime artifacts should live under `.agents/`
- Ticket lock files for local agents should live under `.agents/locks/`
- Any remaining file-based agent memory artifacts should live under `.agents/agent-memory/`
- Git worktrees for local agents must be created under `.agents/worktrees/`
- Do not create agent worktrees under `.claude/`, `../`, or temporary sibling directories
- If you need an isolated workspace for an issue, use a path like `.agents/worktrees/issue-<issue-number>`
