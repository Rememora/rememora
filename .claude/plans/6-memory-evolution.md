# Issue #6: Memory Evolution — LLM-based Consolidation

**Ticket**: https://github.com/Rememora/rememora/issues/6

## Summary

Add `rememora evolve` command that detects duplicate/overlapping memories, sends clusters to an LLM for consolidation decisions (merge/supersede/keep), and applies changes while preserving an audit trail via supersession.

## Implementation Steps

### 1. `src/commands/evolve.rs` — New command module
- `EvolveSummary` struct for results tracking (clusters found, merges, supersessions, kept)
- `MemoryCluster` struct grouping related `ContextRecord`s
- `LlmDecision` enum (Merge/Supersede/Keep) with serde deserialization
- Phase 1: `find_clusters()` — load non-superseded memories for project, group by category, cross-search via BM25 to find similar pairs, union-find to form clusters
- Phase 2: `consolidate_cluster()` — call Anthropic API (claude-haiku-4-5-20251001) with consolidation prompt, parse JSON response
- Phase 3: `apply_decision()` — create merged memory or supersede, respecting `--dry-run`
- `run()` — orchestrate all phases, print summary (text or JSON)

### 2. `src/commands/mod.rs` — Register module
- Add `pub mod evolve;`

### 3. `src/main.rs` — Wire up CLI
- Add `Evolve` variant to `Commands` enum with args: `--project`, `--dry-run`, `--min-similarity`, `--max-batch`
- Add match arm dispatching to `commands::evolve::run()`

### 4. Tests
- `tests/test_evolve.rs` — unit tests for cluster detection (no LLM calls needed)
  - Test that similar memories cluster together
  - Test that dissimilar memories stay separate
  - Test that superseded memories are excluded

## Testing Strategy
- Cluster detection tests use in-memory DB with seeded data
- LLM consolidation is not unit-tested (requires API key) — tested manually
- `cargo test && cargo clippy` must pass
