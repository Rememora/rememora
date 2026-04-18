-- ─── Migration 004: agent-invocation telemetry ──────────────────────────────
-- Captures one row per LLM call Rememora makes, so users can answer questions
-- like "how much did the curator cost this week", "which caller dominates my
-- spend", "is the Haiku signal gate paying for itself". Without this table
-- every subagent invocation is a black box.
--
-- Rows are inserted from four sites:
--   * `src/curator.rs::call_subagent`      — `claude -p` signal-gate + curator
--   * `src/commands/extract.rs`            — direct Anthropic API
--   * `src/commands/evolve.rs`             — direct Anthropic API
--   * `src/commands/agent_run.rs`          — `claude -p` for GitHub dispatch
--
-- Column shape mirrors what `claude -p --output-format json` returns in its
-- final `{type:"result"}` entry plus what the Anthropic REST API puts on
-- `response.usage`. `cost_usd` is taken verbatim from the provider — we don't
-- re-derive from token counts because pricing changes. `parent_session` ties
-- a curator invocation back to the Rememora session that triggered it;
-- `child_session` is the `claude -p` subagent's own session_id (useful for
-- pulling its transcript when debugging permission denials).
--
-- Indexes are cost-ordered for the three queries `rememora usage` actually
-- runs: "latest N", "per-project since X", "per-caller since X".

CREATE TABLE IF NOT EXISTS agent_invocations (
    id                      TEXT PRIMARY KEY,
    ts                      TEXT NOT NULL,
    caller                  TEXT NOT NULL,
    model                   TEXT NOT NULL,
    project                 TEXT,
    parent_session          TEXT,
    child_session           TEXT,
    duration_ms             INTEGER,
    duration_api_ms         INTEGER,
    num_turns               INTEGER,
    input_tokens            INTEGER,
    output_tokens           INTEGER,
    cache_read_tokens       INTEGER,
    cache_creation_tokens   INTEGER,
    cost_usd                REAL,
    stop_reason             TEXT,
    terminal_reason         TEXT,
    is_error                INTEGER NOT NULL DEFAULT 0,
    permission_denials_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_agent_invocations_ts
    ON agent_invocations(ts DESC);
CREATE INDEX IF NOT EXISTS idx_agent_invocations_project_ts
    ON agent_invocations(project, ts DESC);
CREATE INDEX IF NOT EXISTS idx_agent_invocations_caller_ts
    ON agent_invocations(caller, ts DESC);
CREATE INDEX IF NOT EXISTS idx_agent_invocations_parent_session
    ON agent_invocations(parent_session);
