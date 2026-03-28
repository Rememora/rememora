-- Embedding storage for vector search
-- Stores f32 embeddings as little-endian BLOB alongside context references

CREATE TABLE IF NOT EXISTS context_embeddings (
    context_id  TEXT PRIMARY KEY REFERENCES contexts(id) ON DELETE CASCADE,
    embedding   BLOB NOT NULL,
    dimensions  INTEGER NOT NULL,
    model_name  TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_embed_ctx ON context_embeddings(context_id);
