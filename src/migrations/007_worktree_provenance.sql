-- ─── Migration 007: worktree provenance ────────────────────────────────────
-- Records *where* a memory was written from, separately from *what project it
-- belongs to*.
--
-- Background: agents do their work in git worktrees (`AGENTS.md` mandates
-- `.agents/worktrees/`, and Claude Code spawns its own under
-- `~/.claude/worktrees/`). Every write path used to name the project after the
-- directory it happened to be standing in, so a worktree produced a project
-- name matching no registered project — `worktree-brave-meadow-e668`,
-- `-Users-ovidb-Projects-rememora-rememora`, and friends. Because the project
-- filter is a hard `uri LIKE 'rememora://projects/<name>/%'` prefix match,
-- those memories became unreachable the moment the worktree was deleted.
--
-- The fix folds those writes back onto the main checkout's project (see
-- `project::resolve_write_target`), which loses the information about which
-- worktree produced them. These two columns keep it:
--
--   * `worktree` — basename of the linked worktree, or NULL when the write came
--     from the main checkout. NULL is meaningful, not missing.
--   * `branch`   — the branch checked out at write time.
--
-- Deliberately columns rather than entries in `tags`: `tags` is agent-supplied
-- free text that `context::update` overwrites wholesale, it feeds the FTS5
-- index (so a `worktree:` entry would pollute BM25 for anyone searching the
-- word "worktree"), and provenance wants an equality filter, not a match.
--
-- The `ALTER TABLE ... ADD COLUMN` statements are NOT in this file. SQLite has
-- no `ADD COLUMN IF NOT EXISTS`, and `migrate()` runs on every open, so a
-- replay would fail with "duplicate column name" and wedge the database shut.
-- They live in `db.rs` behind `column_exists` guards, exactly like migration
-- 006. Everything below is idempotent and safe to re-run.

-- Provenance is queried as "what came out of this worktree", so the index is on
-- `worktree` alone. Rows from the main checkout are NULL and SQLite omits them
-- from the b-tree, which keeps the index proportional to worktree writes rather
-- than to the whole table.
CREATE INDEX IF NOT EXISTS idx_contexts_worktree ON contexts(worktree)
    WHERE worktree IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_sessions_worktree ON sessions(worktree)
    WHERE worktree IS NOT NULL;
