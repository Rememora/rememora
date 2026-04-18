-- ─── Migration 001: initial schema ──────────────────────────────────────────
-- Establishes the core of Rememora: one unified `contexts` table holding every
-- memory, project, resource, and skill under a single `rememora://` URI tree.
-- Everything else in the DB (sessions, relations, embeddings, analytics) joins
-- back to this table.
--
-- What this migration creates:
--   * `contexts` — the polymorphic entity table. `context_type` discriminates
--     between memory/resource/skill/project; `category` is the 6-way memory
--     taxonomy (preference/entity/decision/event/case/pattern).
--   * `contexts_fts` + triggers — FTS5 virtual table mirroring name/abstract/
--     overview/content for BM25 search. Triggers keep it in sync on insert/
--     update/delete.
--   * `sessions` — cross-agent session lifecycle (start/end/transfer chains).
--   * `relations` — typed edges between contexts for knowledge graphing.
--   * `projects` — registered project roots (name, path, description).
--
-- Why one table instead of many: OpenViking's lesson — polymorphic rows with
-- a URI hierarchy and tiered loading (L0 abstract / L1 overview / L2 content)
-- outperforms a table-per-kind design for long-tail entity types, and keeps
-- hotness/importance ranking uniform across all memory categories.

CREATE TABLE IF NOT EXISTS contexts (
    id          TEXT PRIMARY KEY,
    uri         TEXT NOT NULL UNIQUE,
    parent_uri  TEXT,
    context_type TEXT NOT NULL CHECK (context_type IN ('memory', 'resource', 'skill', 'project')),
    category    TEXT CHECK (category IN ('preference', 'entity', 'decision', 'event', 'case', 'pattern') OR category IS NULL),
    name        TEXT NOT NULL,
    abstract    TEXT NOT NULL DEFAULT '',
    overview    TEXT NOT NULL DEFAULT '',
    content     TEXT NOT NULL DEFAULT '',
    tags        TEXT NOT NULL DEFAULT '[]',
    source_agent TEXT,
    source_session TEXT,
    importance  REAL NOT NULL DEFAULT 0.5,
    active_count INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    superseded_by TEXT REFERENCES contexts(id)
);

CREATE TABLE IF NOT EXISTS sessions (
    id             TEXT PRIMARY KEY,
    agent          TEXT NOT NULL,
    project        TEXT,
    cwd            TEXT,
    started_at     TEXT NOT NULL,
    ended_at       TEXT,
    summary        TEXT NOT NULL DEFAULT '',
    intent         TEXT NOT NULL DEFAULT '',
    working_state  TEXT NOT NULL DEFAULT '',
    message_count  INTEGER NOT NULL DEFAULT 0,
    token_estimate INTEGER NOT NULL DEFAULT 0,
    parent_session TEXT REFERENCES sessions(id),
    status         TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'ended', 'transferred'))
);

CREATE TABLE IF NOT EXISTS relations (
    id            TEXT PRIMARY KEY,
    source_uri    TEXT NOT NULL,
    target_uri    TEXT NOT NULL,
    relation_type TEXT NOT NULL CHECK (relation_type IN ('related', 'depends_on', 'derived_from', 'supersedes')),
    reason        TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL
);

-- FTS5 full-text search on contexts
CREATE VIRTUAL TABLE IF NOT EXISTS contexts_fts USING fts5(
    name,
    abstract,
    overview,
    content,
    tags,
    category,
    content=contexts,
    content_rowid=rowid,
    tokenize='porter unicode61'
);

-- Triggers to keep FTS5 in sync with contexts table
CREATE TRIGGER IF NOT EXISTS contexts_ai AFTER INSERT ON contexts BEGIN
    INSERT INTO contexts_fts(rowid, name, abstract, overview, content, tags, category)
    VALUES (new.rowid, new.name, new.abstract, new.overview, new.content, new.tags, new.category);
END;

CREATE TRIGGER IF NOT EXISTS contexts_ad AFTER DELETE ON contexts BEGIN
    INSERT INTO contexts_fts(contexts_fts, rowid, name, abstract, overview, content, tags, category)
    VALUES ('delete', old.rowid, old.name, old.abstract, old.overview, old.content, old.tags, old.category);
END;

CREATE TRIGGER IF NOT EXISTS contexts_au AFTER UPDATE ON contexts BEGIN
    INSERT INTO contexts_fts(contexts_fts, rowid, name, abstract, overview, content, tags, category)
    VALUES ('delete', old.rowid, old.name, old.abstract, old.overview, old.content, old.tags, old.category);
    INSERT INTO contexts_fts(rowid, name, abstract, overview, content, tags, category)
    VALUES (new.rowid, new.name, new.abstract, new.overview, new.content, new.tags, new.category);
END;

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_ctx_uri ON contexts(uri);
CREATE INDEX IF NOT EXISTS idx_ctx_parent ON contexts(parent_uri);
CREATE INDEX IF NOT EXISTS idx_ctx_type ON contexts(context_type);
CREATE INDEX IF NOT EXISTS idx_ctx_category ON contexts(category);
CREATE INDEX IF NOT EXISTS idx_ctx_importance ON contexts(importance DESC);
CREATE INDEX IF NOT EXISTS idx_ctx_created ON contexts(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_ctx_active ON contexts(active_count DESC);
CREATE INDEX IF NOT EXISTS idx_ctx_agent ON contexts(source_agent);
CREATE INDEX IF NOT EXISTS idx_ctx_superseded ON contexts(superseded_by);

CREATE INDEX IF NOT EXISTS idx_session_project ON sessions(project);
CREATE INDEX IF NOT EXISTS idx_session_agent ON sessions(agent);
CREATE INDEX IF NOT EXISTS idx_session_status ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_session_started ON sessions(started_at DESC);

CREATE INDEX IF NOT EXISTS idx_rel_source ON relations(source_uri);
CREATE INDEX IF NOT EXISTS idx_rel_target ON relations(target_uri);
