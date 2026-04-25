-- ─── Migration 005: hook-invocation telemetry ────────────────────────────────
-- One row per Stop-hook invocation, capturing which recursion gate fired (or
-- whether the curate spawn proceeded). Used to validate that the existing
-- env-var + pgrep gates are sufficient before we consider a DB-backed gate.
--
-- Distinct from `agent_invocations` because hook events carry no token/cost
-- columns and aggregate differently (counts per (hook, outcome), not cost).
--
-- Inserted from a single site:
--   * `plugin/scripts/stop-curate.sh` via `rememora debug record-hook-event`
--
-- Schema is experimental — the `extra` JSON column exists so we can add
-- forward-compat fields without a migration churn during the validation
-- window.

CREATE TABLE IF NOT EXISTS hook_invocations (
    id              TEXT PRIMARY KEY,
    ts              TEXT NOT NULL,
    hook            TEXT NOT NULL,
    outcome         TEXT NOT NULL,
    session_id      TEXT,
    parent_session  TEXT,
    cooldown_state  TEXT,
    extra           TEXT
);

CREATE INDEX IF NOT EXISTS idx_hook_invocations_ts
    ON hook_invocations(ts DESC);
CREATE INDEX IF NOT EXISTS idx_hook_invocations_hook_ts
    ON hook_invocations(hook, ts DESC);
CREATE INDEX IF NOT EXISTS idx_hook_invocations_outcome
    ON hook_invocations(outcome);
CREATE INDEX IF NOT EXISTS idx_hook_invocations_session
    ON hook_invocations(session_id);
