# Rememora Behavior Scenarios

Living documentation of rememora's behavior guarantees.
Each checked item has a corresponding BDD-style test.

## Hotness & Decay

- [x] Fresh memory with zero access has baseline hotness (0.5)
- [x] Memory accessed 100 times today has hotness > 0.95
- [x] At exactly half-life (7 days), hotness decays to ~36.8% of fresh value
- [x] 30-day-old memory decays below 0.05 regardless of access count
- [x] Hotness monotonically decreases with age for fixed access count
- [x] Higher access count gives higher hotness at same age
- [x] Custom half-life parameter changes decay rate proportionally
- [x] Final score blends importance (70%) and hotness (30%) correctly
- [x] With 70/30 weighting, high importance beats high hotness
- [x] Search ranks recent memory above stale one (same importance)
- [x] Search ranks higher importance above lower (same recency)

## Search (BM25)

- [x] BM25 matches by content field
- [x] BM25 matches by abstract field
- [x] BM25 matches by tags field
- [x] Project filter scopes results (includes global)
- [x] Category filter returns only matching category
- [x] Search for nonexistent term returns empty (not error)
- [x] Search bumps active_count for returned results

## Vector Search (embed-candle)

- [x] Cosine nearest neighbor returns closest embedding
- [x] CTE query applies post-filters (project, category)
- [x] Hybrid search fuses BM25 + vector
- [x] Vector-only results get active_count bumped
- [ ] Over-fetch compensates for post-filter row loss

## Sessions

- [x] Session start records agent, project, intent
- [x] Session end records summary and working state
- [x] Transfer handoff preserves parent chain
- [x] Context load includes last session state
- [x] Active session query returns most recent active
- [x] No active session returns None
- [x] Multiple transfer chain preserves full lineage

## Supersession

- [x] Superseded memories are excluded from list_by_scope results
- [x] Superseded memories retain audit trail (not deleted)
- [x] Superseded memories excluded from search results

## Context CRUD

- [x] Insert and retrieve by ID
- [x] Insert and retrieve by URI
- [x] List children by parent URI
- [x] Update partial fields preserves unchanged fields
- [x] Duplicate URI insert fails
- [x] FTS5 indexes content and tags

## Projects

- [x] Add project with full metadata
- [x] List projects returns all registered
- [x] Detect project from subdirectory of registered path
- [x] Detect returns None for unrelated directory

## URI Parsing

- [x] Parse project memory URI extracts all segments
- [x] Parse global URI sets Global scope
- [x] Build memory URI with/without project
- [x] Parent URI strips last segment
- [x] Root URI has no parent
- [x] Invalid URI schemes rejected
- [x] Path traversal rejected

## Hierarchy & Assembly

- [x] L0 map includes global preferences
- [x] L0 map includes project memories
- [x] L0 sorted by score descending
- [x] L1 context respects limit parameter
- [x] Full assembly renders to markdown
- [x] Empty project assembly shows "no memories" message

## Encryption

- [x] Plain DB detected as unencrypted
- [x] Encrypted DB detected as encrypted
- [x] Encrypt → decrypt roundtrip preserves data
- [x] Wrong key fails to open encrypted DB
- [x] FTS5 works with encryption
- [x] WAL mode works with encryption
- [x] Generated keys are unique and 256-bit

## Concurrency

- [x] Concurrent writers produce no data loss (WAL)

## Evolution (Clustering)

- [x] Similar memories cluster together
- [x] Superseded memories excluded from clustering
- [x] Single memory produces no clusters
- [x] Higher threshold produces fewer clusters
