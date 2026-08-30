-- ─── Migration 006: separate access recency from content recency ────────────
-- `updated_at` had two jobs and did neither honestly. `bump_active_count` — run
-- on *every* row returned by a search, including hook injections nobody read —
-- stamped `updated_at`, so merely appearing in a result set marked a memory as
-- freshly written. Since `updated_at` is the recency term in `hotness.rs`, that
-- made retrieval feed ranking feed retrieval: a rich-get-richer loop.
--
-- After this migration:
--   * `updated_at`       — when the *content* last changed (insert/update/
--                          supersede only). What `hotness::final_score` decays
--                          on, and what `rememora eval` uses to detect writes
--                          inside a session window.
--   * `last_accessed_at` — when the row was last *retrieved*. Written by
--                          `bump_active_count` alongside `active_count`.
--                          Deliberately NOT part of the score today; see the
--                          rationale in `src/hotness.rs`.
--
-- Backfill note (the reason we do not "repair" `updated_at`):
--   The pre-fix `updated_at` was overwritten on every read, so its historical
--   value is literally "the timestamp of the last access" — which is exactly
--   what `last_accessed_at` is supposed to hold. Copying it across therefore
--   *recovers* the access-recency signal rather than inventing one. The
--   content-recency half is genuinely unrecoverable, and we leave those rows
--   alone: guessing (e.g. clamping to `created_at`) would silently rewrite the
--   user's only copy of their memory on no evidence. The polluted values stop
--   advancing from here on and decay out of the ranking within one half-life
--   (7 days). See the commit message for the full argument.

ALTER TABLE contexts ADD COLUMN last_accessed_at TEXT;

UPDATE contexts SET last_accessed_at = updated_at WHERE last_accessed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_ctx_last_accessed ON contexts(last_accessed_at DESC);
