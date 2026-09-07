# Rememora

Cross-agent memory system. Rust CLI backed by SQLite.

## Architecture

- Unified `contexts` table (OpenViking pattern) with URI-based hierarchy
- 6 memory categories: preference, entity, decision, event, case, pattern
- L0/L1/L2 tiered loading (abstract/overview/content)
- Hotness scoring: sigmoid(log1p(access_count)) * exp(-age/half_life)
- Sessions with transfer chains for cross-agent continuity
- FTS5 full-text search
- Worktree-aware project resolution: `project::resolve_write_target` folds a git
  worktree onto its main checkout on every write and scoping read path, so a
  `--project` naming a worktree does not create a namespace nothing can search
- `worktree`/`branch` provenance columns on `contexts` and `sessions`
  (migration 007); a NULL worktree means "written from the main checkout"

## Key files

- `src/main.rs` — CLI entry point (clap)
- `src/db.rs` — SQLite connection, WAL, migrations
- `src/uri.rs` — rememora:// URI parsing
- `src/models/context.rs` — Context CRUD + FTS5
- `src/models/session.rs` — Session lifecycle
- `src/models/project.rs` — Project registry, write-target resolution, reconcile
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
- Memories saved from a worktree are filed under the **main checkout's** project,
  not the worktree directory, with the worktree and branch kept as provenance.
  Keep passing `--project rememora` — a registered name always wins verbatim.
  Never substitute the worktree's directory name for the project name.
