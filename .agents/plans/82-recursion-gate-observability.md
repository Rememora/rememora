# Plan: Observability for curate recursion gates (#82)

**Ticket:** https://github.com/Rememora/rememora/issues/82

## Summary

Two recursion gates currently guard `rememora curate` against stampedes/recursion:

1. The `REMEMORA_CURATE_CHILD=1` env var (set in `curator::build_subagent_command`) — short-circuits the Stop hook in curate-spawned children.
2. The `pgrep` gate in `plugin/scripts/stop-curate.sh` — prevents concurrent curate per session at the kernel level.

We have no production telemetry on how often each gate fires vs. passes through. Before we propose a third (DB-backed, session-aware) gate, we need to *measure* the existing two. This change adds **observability only** — no new gate.

Approach:

- Add a new `hook_invocations` table (migration 005) — small, dedicated, separate from `agent_invocations` (which carries token/cost columns that are meaningless for hook events).
- Expose a private CLI verb `rememora debug record-hook-event` that the shell hook calls at each gate exit. One row per Stop-hook invocation.
- Surface the data via `rememora usage --hooks` (mirrors existing usage filters).
- The shell must never block on telemetry — emission failures are silently swallowed and curate proceeds.

## Implementation Steps

### 1. New migration — `src/migrations/005_hook_invocations.sql`

```sql
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
CREATE INDEX IF NOT EXISTS idx_hook_invocations_ts        ON hook_invocations(ts DESC);
CREATE INDEX IF NOT EXISTS idx_hook_invocations_hook_ts   ON hook_invocations(hook, ts DESC);
CREATE INDEX IF NOT EXISTS idx_hook_invocations_outcome   ON hook_invocations(outcome);
CREATE INDEX IF NOT EXISTS idx_hook_invocations_session   ON hook_invocations(session_id);
```

Wire in `src/db.rs` following the existing `MIGRATION_004` pattern.

### 2. New model — `src/models/hook_invocation.rs`

Mirror `agent_invocation.rs` shape but smaller. Provide:

- `enum HookKind { StopCurate }` with `as_str()` and `from_str()`.
- `enum Outcome { EnvVarShortCircuit, PgrepShortCircuit, CooldownShortCircuit, PassedThrough }` with `as_str()` and `from_str()`.
- `struct HookEventRecord { hook, outcome, session_id, parent_session, cooldown_state, extra }`.
- `pub fn insert(conn, record) -> Result<String>` — generates ULID + ts.
- `pub fn try_insert(conn, record)` — best-effort.
- `struct HookAggregate { hook, outcome, count }` and `pub fn aggregate_by_outcome(conn, since, hook_filter)`.

Register in `src/models/mod.rs`.

### 3. New private command — `src/commands/debug_hook.rs`

Tiny handler that validates inputs against the enums and inserts one row.

### 4. New CLI subcommand `Debug` in `src/main.rs`

Add `Debug` with nested action `RecordHookEvent`. Help text marks it experimental.

### 5. Extend `rememora usage` with `--hooks`

Per the issue, prefer `--hooks` over a sibling verb. When set, `usage::run` calls the hook aggregator and prints a hook-shaped table. JSON mode mirrors structure. Since-filter reuses `parse_since`.

### 6. Modify `plugin/scripts/stop-curate.sh`

Add `_emit` helper that backgrounds the call so the hook returns instantly and redirects all streams. Insertion points:

- env-var gate (early exit) → `env_var_short_circuit` (no session_id at that point).
- pgrep gate → `pgrep_short_circuit`.
- cooldown gate → `cooldown_short_circuit`.
- just before fork → `passed_through`.

Kill-switch (`REMEMORA_DISABLE_HOOKS`) is NOT instrumented — it is user config, not a recursion gate.

### 7. Tests

- Rust unit tests in `models/hook_invocation.rs`, `commands/debug_hook.rs`, `commands/usage.rs`.
- Integration test `tests/test_hook_invocations.rs` exercising the `record` path.
- Shell smoke test invoking `stop-curate.sh` against a scratch DB.

## Testing Strategy

```bash
cargo test
cargo clippy -- -D warnings
REMEMORA_DB=/tmp/rememora-smoke.db \
  rememora debug record-hook-event --hook stop-curate --outcome passed_through --session-id smoke-1
REMEMORA_DB=/tmp/rememora-smoke.db rememora usage --hooks --since all
```

## Out of Scope

- New gates (DB-backed session-aware).
- OTLP export of hook events.
- Backfill of historical data.
- Reusing `agent_invocations`.
