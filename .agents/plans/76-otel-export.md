# Issue #76 — Thin OTEL export layer over agent_invocations

**Ticket:** https://github.com/Rememora/rememora/issues/76

## Summary

Add a CLI verb `rememora telemetry export-otlp` that reads from the existing
`agent_invocations` table (migration 004) and emits OTEL-compatible JSON spans
on demand. Export-on-demand only — no daemon, no remote push. Keeps OTEL as a
view/interop concern over the single source of truth that `rememora usage`
already aggregates.

## Implementation Steps

### 1. New module `src/commands/telemetry.rs`

Add an `export_otlp` handler mirroring the filter plumbing of `usage.rs`:

- `TelemetryExportArgs { since, caller, project, parent_session, format }`
- `format` accepts `jsonl` (default — one OTLP span JSON per line) and
  `otlp` (single OTLP HTTP/JSON doc with resourceSpans/scopeSpans/spans).
- Replicate `parse_since` locally to avoid changing `usage.rs` signature.
- Call a new reader `agent_invocation::list_invocations(conn, filters)`.
- For each row, build a span via `row_to_span`:
  - `trace_id` = 16-byte hex from `blake3(parent_session || "rememora-trace")`
    with fallback to `blake3(id || "rememora-trace-orphan")` when
    `parent_session` is null.
  - `span_id` = 8-byte hex from `blake3(id || "rememora-span")`.
  - `name` = caller-specific: signal_gate → `rememora.signal_gate.run`,
    curator → `rememora.curator.run`, extract → `rememora.extract.run`,
    evolve → `rememora.evolve.consolidate_cluster`, consolidate →
    `rememora.consolidate.run`, agent_run → `rememora.agent_run.attempt`.
    Unknown callers fall back to `rememora.<caller>.run`.
  - `start_time_unix_nano` = RFC3339(ts) in nanos.
  - `end_time_unix_nano` = start + duration_ms*1_000_000 (start when null).
  - `status.code` = STATUS_CODE_ERROR if is_error else STATUS_CODE_OK.
  - Attributes: gen_ai.system=anthropic; gen_ai.request.model;
    gen_ai.usage.{input,output,cache_read,cache_creation}_tokens;
    gen_ai.response.cost_usd; gen_ai.response.stop_reason;
    rememora.{caller,project,parent_session,child_session,
    terminal_reason,num_turns}. Nulls omitted.
- Resource: service.name=rememora, service.version=env!("CARGO_PKG_VERSION").
- JSONL mode: one compact JSON span per stdout line.
- OTLP mode: single pretty JSON object.

### 2. Reader in `src/models/agent_invocation.rs`

Add `InvocationRow` (full row shape) plus `list_invocations(conn, filters)`
that SELECTs with optional filters on ts >= ?, caller = ?, project = ?,
parent_session = ?, ordered by ts ASC. Use `params_from_iter` for dynamic
WHERE assembly. Keep `aggregate` untouched.

### 3. CLI wiring

- `src/main.rs`: new `Telemetry { action: TelemetryAction }` with
  `TelemetryAction::ExportOtlp { since, caller, project, parent_session,
  format }`.
- `src/commands/mod.rs`: add `pub mod telemetry;`.

### 4. Tests

Unit tests in `src/commands/telemetry.rs`:

- span_id_is_stable_and_hex
- trace_id_groups_by_parent_session
- trace_id_falls_back_on_orphan
- status_reflects_is_error
- attributes_include_gen_ai_and_rememora
- name_maps_per_caller
- end_time_uses_duration

Model-level test:

- list_invocations_filters_by_caller_and_project

## Testing Strategy

- cargo test — all tests green
- cargo clippy --all-targets — no new warnings
- cargo build --release — binary builds

## Out of scope

- Retention / auto-prune
- OTLP gRPC
- Remote collector push
- Touching bench/ or A/B experiment harness
