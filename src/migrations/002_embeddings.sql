-- ─── Migration 002: embedding storage ───────────────────────────────────────
-- Parks vector embeddings for each context so vector search can run alongside
-- FTS5/BM25. Storage-only — the actual similarity query lives in sqlite-vec's
-- virtual table (`vec_contexts`, created in db.rs when the `embed-candle`
-- feature is enabled).
--
-- Shape: one row per context. `embedding` is a packed little-endian f32 BLOB
-- (dimensions × 4 bytes); `dimensions` and `model_name` let us detect and
-- migrate when the backend swaps embedders.
--
-- Why a separate table instead of a column on `contexts`: embeddings are big
-- (~1.5 KB for a 384-d f32), feature-gated, and have their own lifecycle
-- (re-embed on model change). Keeping them out of the main row avoids bloat
-- on every SELECT against `contexts`.

CREATE TABLE IF NOT EXISTS context_embeddings (
    context_id  TEXT PRIMARY KEY REFERENCES contexts(id) ON DELETE CASCADE,
    embedding   BLOB NOT NULL,
    dimensions  INTEGER NOT NULL,
    model_name  TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
