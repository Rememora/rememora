# Issue #5: Hierarchical Retrieval with Score Propagation

**Ticket**: https://github.com/Rememora/rememora/issues/5
**Branch**: `feat/5-hierarchical-score-propagation`

## Summary

When a memory matches a search query, related memories in the URI hierarchy (parents, children, siblings) should receive boosted scores. This enables contextual retrieval: if "Zustand state management" matches strongly, the parent category context and sibling decisions in that same category get a relevance boost, giving the user richer context without requiring exact keyword matches on those related memories.

## Current State

- `search.rs::search()` returns FTS5/BM25 results with `rank` (negative, lower = better match)
- `search.rs::hybrid_search()` adds optional vector search via RRF fusion
- `hierarchy.rs` has L0/L1/L2 tiered loading but no score propagation
- `uri.rs` has `parent()` function that computes parent URI from any URI
- `models/context.rs` has `list_by_parent()` to find children of a parent URI
- `models/relation.rs` tracks explicit relations between URIs
- DB schema has `parent_uri` column and `idx_ctx_parent` index

### URI Structure (how hierarchy works)

```
rememora://projects/myproj/memories/decision/use-zustand   (leaf memory)
                                    ^parent: rememora://projects/myproj/memories/decision
                                    ^grandparent: rememora://projects/myproj/memories
rememora://projects/myproj/memories/decision/prefer-hooks  (sibling of use-zustand)
```

Siblings share the same `parent_uri`. Children have `parent_uri` pointing to the matched context's `uri`.

## Design

### Approach: Post-search propagation layer

Rather than modifying the FTS5 query itself (which would be fragile and SQLite-specific), we add a **post-processing propagation step** that:

1. Takes the raw BM25 search results
2. For each matched context, finds its parent, children, and siblings via URI relationships
3. Assigns propagated scores to those related contexts using exponential decay
4. Merges propagated contexts into the result set (deduplicating with max-score)
5. Re-sorts and truncates to the requested limit

### Score Propagation Formula

For a matched context with BM25 rank `r`:
- **Normalized base score**: `base = 1.0 / (1.0 + |r|)` (maps negative BM25 rank to 0..1)
- **Parent boost**: `base * decay_factor^1` (one hop up)
- **Child boost**: `base * decay_factor^1` (one hop down)
- **Sibling boost**: `base * decay_factor^2` (up one, down one = 2 hops)

Default `decay_factor = 0.3` (configurable). Max propagation depth = 2 (configurable).

The propagated score is *additive* with any existing direct-match score, meaning a context that both matches directly AND gets a propagation boost will rank higher.

### New CLI Flags

Added to the `Search` command:
- `--propagate` (bool flag, default false): Enable hierarchical score propagation
- `--propagate-decay <f64>` (default 0.3): Decay factor per hop
- `--propagate-depth <usize>` (default 2): Maximum propagation hops

Propagation is opt-in (off by default) to avoid surprising users or slowing down simple searches. When enabled, the search fetches extra candidates to propagate through.

## Implementation Steps

### Step 1: Add `propagate` module (`src/propagate.rs`)

New module with the core propagation logic:

```rust
pub struct PropagationConfig {
    pub decay_factor: f64,  // Score multiplier per hop (default 0.3)
    pub max_depth: usize,   // Maximum hops to propagate (default 2)
}

pub fn propagate_scores(
    conn: &Connection,
    results: Vec<SearchResult>,
    config: &PropagationConfig,
) -> Result<Vec<SearchResult>>
```

Algorithm:
1. Normalize BM25 ranks to positive scores: `score = 1.0 / (1.0 + rank.abs())`
2. For each result, collect related URIs at each depth level:
   - depth 1: parent (via `uri::parent()`), children (via `context::list_by_parent()`)
   - depth 2: grandparent, parent's other children (siblings), children's children
3. For each related context found, compute propagated score = `base_score * decay^depth`
4. Merge into a HashMap<context_id, (max_score, SearchResult)> - use max of direct and propagated scores
5. Sort descending by merged score, return

Helper functions:
- `fn find_related(conn, uri, depth, max_depth) -> Vec<(ContextRecord, usize)>` - recursive URI walk
- `fn normalize_bm25_rank(rank: f64) -> f64` - convert negative BM25 rank to 0..1 score

### Step 2: Register module in `src/lib.rs`

Add `pub mod propagate;` to `src/lib.rs`.

### Step 3: Integrate with `search.rs`

Add a new public function in `search.rs`:

```rust
pub fn search_with_propagation(
    conn: &Connection,
    query: &str,
    project: Option<&str>,
    category: Option<&str>,
    limit: usize,
    propagation_config: &PropagationConfig,
) -> Result<Vec<SearchResult>>
```

This calls `search()` with an expanded limit (limit * 3), then calls `propagate::propagate_scores()`, then truncates to `limit`.

### Step 4: Add CLI flags to `Search` command in `main.rs`

Add three new fields to `Commands::Search`:
- `propagate: bool` (flag)
- `propagate_decay: f64` (default 0.3)
- `propagate_depth: usize` (default 2)

### Step 5: Update `commands/search.rs`

- Add propagation fields to `SearchArgs`
- When `propagate` is true, call `search_with_propagation()` instead of `search()`
- The output format stays the same (markdown or JSON) - propagated results just have different rank values

### Step 6: Update `format.rs` search output

Add a `propagated: bool` field to `SearchResult` (or track it separately) so the output can indicate which results came from propagation vs direct match. In markdown output, annotate propagated results with a subtle marker like `(via hierarchy)`.

Actually, simpler approach: add an optional `propagation_source` field to SearchResult. If None, it was a direct match. If Some(source_uri), it was boosted from that context's match.

### Step 7: Tests

New test file `tests/behavior_propagation.rs` (BDD-style):

1. **Parent gets boosted when child matches**: Create parent + child memories. Search for child's keyword. With propagation, parent should appear in results.
2. **Sibling gets boosted when sibling matches**: Create two siblings (same parent_uri). Search for one. With propagation, the other should appear.
3. **Children get boosted when parent matches**: Create parent + children. Search for parent's keyword. With propagation, children should appear.
4. **Decay reduces score with distance**: Verify parent boost > sibling boost (1 hop vs 2 hops).
5. **Direct match outranks propagated**: A direct match should always score higher than a propagated one at the same depth.
6. **Propagation respects max_depth**: With depth=1, siblings (2 hops) should NOT appear.
7. **Propagation disabled by default**: Without `--propagate`, results should be identical to current behavior.
8. **Superseded contexts excluded**: Propagation should not surface contexts where `superseded_by IS NOT NULL`.

Unit tests in `src/propagate.rs`:
- `normalize_bm25_rank` returns correct values
- Score decay calculation is correct

### Step 8: Integration with `hierarchy.rs` (optional enhancement)

Update `hierarchy::assemble()` to optionally use propagation when building the L0/L1 context map, so that `rememora context --project X` benefits from hierarchical scoring too. This is a stretch goal and can be deferred.

## Testing Strategy

- All new tests use BDD scenario builders (`scenarios::memory`, `scenarios::db_with_memories`)
- Integration tests hit real in-memory SQLite (no mocks)
- `cargo test` must pass (all existing tests + new propagation tests)
- `cargo clippy` must pass with no warnings

## Files Changed

| File | Change |
|------|--------|
| `src/propagate.rs` | **New** - Core propagation logic |
| `src/lib.rs` | Add `pub mod propagate;` |
| `src/search.rs` | Add `search_with_propagation()` function |
| `src/main.rs` | Add `--propagate`, `--propagate-decay`, `--propagate-depth` flags to Search |
| `src/commands/search.rs` | Route to propagation-aware search when flag is set |
| `src/format.rs` | Minor: show propagation indicator in output |
| `tests/behavior_propagation.rs` | **New** - BDD-style propagation tests |

## Risks / Open Questions

1. **Performance**: Propagation requires additional DB queries per result (fetch parent, children, siblings). For typical memory databases (100s-1000s of entries), this should be fine. The expanded initial fetch (limit * 3) and the per-result lookups are bounded.

2. **Circular propagation**: URI hierarchy is a tree (parent_uri forms a DAG), so circular propagation is not possible. The depth limit also provides a safety net.

3. **Score semantics**: BM25 ranks are negative (lower = better). After normalization and propagation, all scores are positive (higher = better). This changes the `rank` field semantics when propagation is active. The output should be clear about this.
