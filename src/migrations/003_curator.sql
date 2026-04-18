-- ─── Migration 003: curator infrastructure ──────────────────────────────────
-- Backs the Stop-hook curator's three responsibilities:
--   1. *Incremental* curation — never re-feed the signal gate or curator with
--      content it has already seen. `watermarks` stores the byte offset we
--      have consumed up to for each Claude Code session JSONL, keyed by file
--      path. On every Stop hook we `parse_file(path, watermark.byte_offset)`
--      and only the delta reaches the subagent.
--   2. *Audit trail* — `curator_log` records every action the curator took
--      (add/update/delete/noop) with the reason and the model that decided.
--      Makes it possible to debug "why did the curator change this memory".
--   3. *Consolidation tracking* — `consolidation_runs` brackets each evolve/
--      merge pass with before/after counts, cluster count, and the trigger
--      source, so we can tell consolidation cron runs from manual invocations.
--
-- Watermark advances on Signal::No too (see `src/commands/curate.rs`), which
-- is what makes the Haiku signal gate cheap — it never re-evaluates stale
-- turns.

-- Tracks byte offset per session JSONL file for incremental curation
CREATE TABLE IF NOT EXISTS watermarks (
    file_path   TEXT PRIMARY KEY,
    byte_offset INTEGER NOT NULL DEFAULT 0,
    line_count  INTEGER NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL
);

-- Audit log of every curation action performed by the curator
CREATE TABLE IF NOT EXISTS curator_log (
    id          TEXT PRIMARY KEY,
    file_path   TEXT NOT NULL,
    action      TEXT NOT NULL CHECK (action IN ('add', 'update', 'delete', 'noop')),
    context_id  TEXT REFERENCES contexts(id),
    reason      TEXT NOT NULL DEFAULT '',
    model       TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_curator_log_file ON curator_log(file_path);
CREATE INDEX IF NOT EXISTS idx_curator_log_action ON curator_log(action);
CREATE INDEX IF NOT EXISTS idx_curator_log_created ON curator_log(created_at DESC);

-- Tracks consolidation (evolve/merge) runs
CREATE TABLE IF NOT EXISTS consolidation_runs (
    id              TEXT PRIMARY KEY,
    project         TEXT,
    memories_before INTEGER NOT NULL DEFAULT 0,
    memories_after  INTEGER NOT NULL DEFAULT 0,
    clusters_found  INTEGER NOT NULL DEFAULT 0,
    actions_taken   TEXT NOT NULL DEFAULT '[]',
    model           TEXT NOT NULL DEFAULT '',
    triggered_by    TEXT NOT NULL DEFAULT 'manual' CHECK (triggered_by IN ('manual', 'cron', 'session_start', 'curator')),
    started_at      TEXT NOT NULL,
    completed_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_consolidation_project ON consolidation_runs(project);
CREATE INDEX IF NOT EXISTS idx_consolidation_started ON consolidation_runs(started_at DESC);
