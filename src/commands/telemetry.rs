//! `rememora telemetry export-otlp` — emit OTEL-compatible JSON spans from
//! the existing `agent_invocations` table.
//!
//! Each row in `agent_invocations` becomes one OTLP span. Trace IDs are
//! derived from `parent_session` so attempts that share a rememora session
//! group into a single trace; span IDs are derived from the row ULID. The
//! export is local-first and read-only — it never writes to the DB or pushes
//! to a remote collector. Output goes to stdout.
//!
//! Two shapes are supported:
//!   * `jsonl` (default) — one compact OTLP span JSON object per line.
//!   * `otlp` — a single OTLP HTTP/JSON document wrapping every span under
//!     `resourceSpans[0].scopeSpans[0]`.
//!
//! This verb intentionally re-parses `--since` locally rather than reaching
//! into `commands::usage::parse_since`, so `usage.rs`'s surface stays stable.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

use rememora::models::agent_invocation::{
    self, InvocationFilter, InvocationRow,
};

pub struct TelemetryExportArgs {
    pub since: String,
    pub caller: Option<String>,
    pub project: Option<String>,
    pub parent_session: Option<String>,
    pub format: String,
}

pub fn export_otlp(conn: &Connection, args: &TelemetryExportArgs) -> Result<()> {
    let since = parse_since(&args.since).context("invalid --since value")?;
    let format = parse_format(&args.format).context("invalid --format value")?;

    let filter = InvocationFilter {
        since,
        caller: args.caller.clone(),
        project: args.project.clone(),
        parent_session: args.parent_session.clone(),
    };

    let rows = agent_invocation::list_invocations(conn, &filter)?;

    match format {
        OutputFormat::Jsonl => {
            for row in &rows {
                let span = row_to_span(row);
                println!("{}", serde_json::to_string(&span)?);
            }
        }
        OutputFormat::Otlp => {
            let doc = spans_to_otlp_doc(&rows);
            println!("{}", serde_json::to_string_pretty(&doc)?);
        }
    }

    Ok(())
}

// ── Output format parsing ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Jsonl,
    Otlp,
}

fn parse_format(s: &str) -> Result<OutputFormat> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "jsonl" | "lines" | "otlp-jsonl" => Ok(OutputFormat::Jsonl),
        "otlp" | "otlp-json" | "doc" => Ok(OutputFormat::Otlp),
        other => bail!("unknown --format `{other}` (expected jsonl|otlp)"),
    }
}

// ── Relative window parsing (mirrors commands::usage::parse_since) ───

fn parse_since(s: &str) -> Result<Option<String>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("all") || s.is_empty() {
        return Ok(None);
    }

    // Allow callers to pass a raw ISO8601 timestamp straight through.
    if s.contains('T') && DateTime::parse_from_rfc3339(s).is_ok() {
        return Ok(Some(s.to_string()));
    }

    let (n_str, unit) = s.split_at(
        s.rfind(|c: char| c.is_ascii_digit())
            .map(|i| i + 1)
            .unwrap_or(0),
    );
    let n: i64 = n_str.parse().context("expected a number before the unit")?;
    let duration = match unit.trim() {
        "m" | "min" | "mins" => Duration::minutes(n),
        "h" | "hr" | "hrs" => Duration::hours(n),
        "d" | "day" | "days" => Duration::days(n),
        "w" | "wk" | "wks" => Duration::weeks(n),
        other => bail!("unknown time unit `{other}` (use m/h/d/w, `all`, or an ISO8601 timestamp)"),
    };

    let cutoff = Utc::now() - duration;
    Ok(Some(cutoff.to_rfc3339()))
}

// ── Row → span mapping ───────────────────────────────────────────────

/// Convert one `InvocationRow` into an OTLP-shaped JSON span.
///
/// The returned object is compatible with the OTLP/JSON schema for
/// `opentelemetry.proto.trace.v1.Span`: hex `trace_id`/`span_id`,
/// `start_time_unix_nano`/`end_time_unix_nano` as decimal strings (nanos
/// overflow i32 but fit in i64; we stringify to match OTLP's convention for
/// 64-bit fields), keyed `attributes` array with typed values, and a
/// `status` object. Resource is emitted only in the doc-level `otlp` shape,
/// not per-span in `jsonl` mode — callers that want a full document should
/// use `--format otlp`.
pub(crate) fn row_to_span(row: &InvocationRow) -> Value {
    let trace_id = trace_id_hex(row);
    let span_id = span_id_hex(&row.id);
    let name = span_name_for(&row.caller);

    let start_nano = rfc3339_to_unix_nano(&row.ts).unwrap_or(0);
    let end_nano = match row.duration_ms {
        Some(ms) if ms >= 0 => start_nano.saturating_add(ms as i128 * 1_000_000),
        _ => start_nano,
    };

    let (status_code_int, status_code_str) = if row.is_error {
        (2u32, "STATUS_CODE_ERROR")
    } else {
        (1u32, "STATUS_CODE_OK")
    };

    let attributes = build_attributes(row);

    json!({
        "traceId": trace_id,
        "spanId": span_id,
        "name": name,
        "kind": 3, // SPAN_KIND_CLIENT — we are calling a remote LLM
        "startTimeUnixNano": start_nano.to_string(),
        "endTimeUnixNano": end_nano.to_string(),
        "attributes": attributes,
        "status": {
            "code": status_code_int,
            "codeName": status_code_str,
        },
    })
}

/// Wrap spans into a single OTLP HTTP/JSON document with one resourceSpans
/// entry. Matches `opentelemetry.proto.collector.trace.v1.ExportTraceServiceRequest`.
fn spans_to_otlp_doc(rows: &[InvocationRow]) -> Value {
    let spans: Vec<Value> = rows.iter().map(row_to_span).collect();

    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    kv_string("service.name", "rememora"),
                    kv_string("service.version", env!("CARGO_PKG_VERSION")),
                ]
            },
            "scopeSpans": [{
                "scope": {
                    "name": "rememora",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "spans": spans,
            }]
        }]
    })
}

// ── Attribute builders ───────────────────────────────────────────────

fn build_attributes(row: &InvocationRow) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(16);
    out.push(kv_string("gen_ai.system", "anthropic"));
    out.push(kv_string("gen_ai.request.model", &row.model));

    if let Some(v) = row.input_tokens {
        out.push(kv_int("gen_ai.usage.input_tokens", v));
    }
    if let Some(v) = row.output_tokens {
        out.push(kv_int("gen_ai.usage.output_tokens", v));
    }
    if let Some(v) = row.cache_read_tokens {
        out.push(kv_int("gen_ai.usage.cache_read_tokens", v));
    }
    if let Some(v) = row.cache_creation_tokens {
        out.push(kv_int("gen_ai.usage.cache_creation_tokens", v));
    }
    if let Some(v) = row.cost_usd {
        out.push(kv_double("gen_ai.response.cost_usd", v));
    }
    if let Some(v) = row.stop_reason.as_deref() {
        out.push(kv_string("gen_ai.response.stop_reason", v));
    }

    out.push(kv_string("rememora.caller", &row.caller));
    if let Some(v) = row.project.as_deref() {
        out.push(kv_string("rememora.project", v));
    }
    if let Some(v) = row.parent_session.as_deref() {
        out.push(kv_string("rememora.parent_session", v));
    }
    if let Some(v) = row.child_session.as_deref() {
        out.push(kv_string("rememora.child_session", v));
    }
    if let Some(v) = row.terminal_reason.as_deref() {
        out.push(kv_string("rememora.terminal_reason", v));
    }
    if let Some(v) = row.num_turns {
        out.push(kv_int("rememora.num_turns", v));
    }

    out
}

fn kv_string(key: &str, val: &str) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), Value::String(key.into()));
    let mut inner = Map::new();
    inner.insert("stringValue".into(), Value::String(val.into()));
    m.insert("value".into(), Value::Object(inner));
    Value::Object(m)
}

fn kv_int(key: &str, val: i64) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), Value::String(key.into()));
    let mut inner = Map::new();
    // OTLP/JSON represents int64 as a decimal string to preserve precision.
    inner.insert("intValue".into(), Value::String(val.to_string()));
    m.insert("value".into(), Value::Object(inner));
    Value::Object(m)
}

fn kv_double(key: &str, val: f64) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), Value::String(key.into()));
    let mut inner = Map::new();
    inner.insert(
        "doubleValue".into(),
        serde_json::Number::from_f64(val)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    );
    m.insert("value".into(), Value::Object(inner));
    Value::Object(m)
}

// ── Name & id derivation ─────────────────────────────────────────────

fn span_name_for(caller: &str) -> String {
    match caller {
        "signal_gate" => "rememora.signal_gate.run".to_string(),
        "curator" => "rememora.curator.run".to_string(),
        "extract" => "rememora.extract.run".to_string(),
        "evolve" => "rememora.evolve.consolidate_cluster".to_string(),
        "consolidate" => "rememora.consolidate.run".to_string(),
        "agent_run" => "rememora.agent_run.attempt".to_string(),
        other => format!("rememora.{other}.run"),
    }
}

fn trace_id_hex(row: &InvocationRow) -> String {
    // OTLP trace IDs are 16 bytes / 32 hex chars. Group attempts that share
    // a parent session; fall back to the row id so orphaned rows still have
    // a stable (per-span) trace id.
    match row.parent_session.as_deref() {
        Some(session) if !session.is_empty() => hash_hex_16(session.as_bytes(), b"trace"),
        _ => hash_hex_16(row.id.as_bytes(), b"trace-orphan"),
    }
}

fn span_id_hex(row_id: &str) -> String {
    // OTLP span IDs are 8 bytes / 16 hex chars.
    hash_hex_8(row_id.as_bytes(), b"span")
}

fn rfc3339_to_unix_nano(ts: &str) -> Option<i128> {
    let parsed = DateTime::parse_from_rfc3339(ts).ok()?;
    let secs = parsed.timestamp() as i128;
    let subsec = parsed.timestamp_subsec_nanos() as i128;
    Some(secs * 1_000_000_000 + subsec)
}

// ── Deterministic hashing (FNV-1a 64-bit) ────────────────────────────
//
// We avoid pulling in a crypto crate just to derive stable trace/span ids.
// FNV-1a is trivial, deterministic across platforms, and more than enough
// for the "attempts sharing a session should group" property — we are not
// using this hash for security or for avoiding collisions across arbitrary
// adversarial input.

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x100_0000_01b3;

fn fnv1a(data: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn hash_hex_16(data: &[u8], tag: &[u8]) -> String {
    // 16 bytes of entropy: two independent FNV passes with distinct salts.
    let a = fnv1a_with_salt(data, tag, 0);
    let b = fnv1a_with_salt(data, tag, 1);
    format!("{:016x}{:016x}", a, b)
}

fn hash_hex_8(data: &[u8], tag: &[u8]) -> String {
    let a = fnv1a_with_salt(data, tag, 0);
    format!("{:016x}", a)
}

fn fnv1a_with_salt(data: &[u8], tag: &[u8], round: u8) -> u64 {
    let mut buf: Vec<u8> = Vec::with_capacity(data.len() + tag.len() + 2);
    buf.extend_from_slice(tag);
    buf.push(b':');
    buf.push(round);
    buf.extend_from_slice(data);
    fnv1a(&buf)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_row(id: &str, caller: &str) -> InvocationRow {
        InvocationRow {
            id: id.into(),
            ts: "2026-04-21T12:00:00Z".into(),
            caller: caller.into(),
            model: "claude-sonnet".into(),
            project: Some("demo".into()),
            parent_session: Some("sess-1".into()),
            child_session: Some("child-1".into()),
            duration_ms: Some(1500),
            duration_api_ms: Some(1400),
            num_turns: Some(3),
            input_tokens: Some(100),
            output_tokens: Some(200),
            cache_read_tokens: Some(50),
            cache_creation_tokens: Some(25),
            cost_usd: Some(0.0123),
            stop_reason: Some("end_turn".into()),
            terminal_reason: Some("ok".into()),
            is_error: false,
        }
    }

    #[test]
    fn span_id_is_stable_and_hex() {
        let row = mk_row("01ABC", "curator");
        let span = row_to_span(&row);
        let sid = span["spanId"].as_str().unwrap();
        assert_eq!(sid.len(), 16);
        assert!(sid.chars().all(|c| c.is_ascii_hexdigit()));
        // Stability: re-hashing yields the same id.
        let span2 = row_to_span(&row);
        assert_eq!(span["spanId"], span2["spanId"]);
    }

    #[test]
    fn trace_id_groups_by_parent_session() {
        let mut a = mk_row("01A", "curator");
        let mut b = mk_row("01B", "signal_gate");
        a.parent_session = Some("shared-session".into());
        b.parent_session = Some("shared-session".into());

        let sa = row_to_span(&a);
        let sb = row_to_span(&b);

        assert_eq!(sa["traceId"], sb["traceId"]);
        assert_ne!(sa["spanId"], sb["spanId"]);
    }

    #[test]
    fn trace_id_falls_back_on_orphan() {
        let mut a = mk_row("01A", "extract");
        let mut b = mk_row("01B", "extract");
        a.parent_session = None;
        b.parent_session = None;

        let sa = row_to_span(&a);
        let sb = row_to_span(&b);

        let ta = sa["traceId"].as_str().unwrap();
        let tb = sb["traceId"].as_str().unwrap();
        assert_eq!(ta.len(), 32);
        assert_eq!(tb.len(), 32);
        // Different orphans get different traces (derived from row id).
        assert_ne!(ta, tb);
        // Same row id stable.
        let sa2 = row_to_span(&a);
        assert_eq!(sa["traceId"], sa2["traceId"]);
    }

    #[test]
    fn status_reflects_is_error() {
        let mut row = mk_row("01ERR", "curator");
        row.is_error = true;
        let span = row_to_span(&row);
        assert_eq!(span["status"]["code"], 2);
        assert_eq!(span["status"]["codeName"], "STATUS_CODE_ERROR");

        row.is_error = false;
        let span = row_to_span(&row);
        assert_eq!(span["status"]["code"], 1);
        assert_eq!(span["status"]["codeName"], "STATUS_CODE_OK");
    }

    #[test]
    fn attributes_include_gen_ai_and_rememora() {
        let row = mk_row("01A", "curator");
        let span = row_to_span(&row);
        let attrs = span["attributes"].as_array().unwrap();

        let keys: Vec<&str> = attrs
            .iter()
            .map(|a| a["key"].as_str().unwrap())
            .collect();

        for required in [
            "gen_ai.system",
            "gen_ai.request.model",
            "gen_ai.usage.input_tokens",
            "gen_ai.usage.output_tokens",
            "gen_ai.usage.cache_read_tokens",
            "gen_ai.usage.cache_creation_tokens",
            "gen_ai.response.cost_usd",
            "gen_ai.response.stop_reason",
            "rememora.caller",
            "rememora.project",
            "rememora.parent_session",
            "rememora.child_session",
            "rememora.terminal_reason",
            "rememora.num_turns",
        ] {
            assert!(
                keys.contains(&required),
                "missing attribute key: {required} in {keys:?}"
            );
        }

        // gen_ai.system is always anthropic.
        let system = attrs
            .iter()
            .find(|a| a["key"] == "gen_ai.system")
            .unwrap();
        assert_eq!(system["value"]["stringValue"], "anthropic");
    }

    #[test]
    fn attributes_omit_null_fields() {
        let mut row = mk_row("01A", "curator");
        row.cost_usd = None;
        row.stop_reason = None;
        row.project = None;

        let span = row_to_span(&row);
        let keys: Vec<&str> = span["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["key"].as_str().unwrap())
            .collect();

        assert!(!keys.contains(&"gen_ai.response.cost_usd"));
        assert!(!keys.contains(&"gen_ai.response.stop_reason"));
        assert!(!keys.contains(&"rememora.project"));
    }

    #[test]
    fn name_maps_per_caller() {
        let cases = [
            ("signal_gate", "rememora.signal_gate.run"),
            ("curator", "rememora.curator.run"),
            ("extract", "rememora.extract.run"),
            ("evolve", "rememora.evolve.consolidate_cluster"),
            ("consolidate", "rememora.consolidate.run"),
            ("agent_run", "rememora.agent_run.attempt"),
            ("brand_new_kind", "rememora.brand_new_kind.run"),
        ];
        for (caller, expected) in cases {
            let row = mk_row("01A", caller);
            let span = row_to_span(&row);
            assert_eq!(span["name"], expected, "caller {caller}");
        }
    }

    #[test]
    fn end_time_uses_duration() {
        let mut row = mk_row("01A", "curator");
        row.duration_ms = Some(2_000);
        let span = row_to_span(&row);

        let start: i128 = span["startTimeUnixNano"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let end: i128 = span["endTimeUnixNano"].as_str().unwrap().parse().unwrap();
        assert_eq!(end - start, 2_000 * 1_000_000);

        // No duration → end collapses to start.
        row.duration_ms = None;
        let span = row_to_span(&row);
        assert_eq!(span["startTimeUnixNano"], span["endTimeUnixNano"]);
    }

    #[test]
    fn otlp_doc_wraps_spans_with_resource() {
        let row = mk_row("01A", "curator");
        let doc = spans_to_otlp_doc(&[row]);
        let rs = doc["resourceSpans"][0].clone();
        let res_attrs = rs["resource"]["attributes"].as_array().unwrap();

        let keys: Vec<&str> = res_attrs
            .iter()
            .map(|a| a["key"].as_str().unwrap())
            .collect();
        assert!(keys.contains(&"service.name"));
        assert!(keys.contains(&"service.version"));

        let spans = rs["scopeSpans"][0]["spans"].as_array().unwrap();
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn parse_since_accepts_all_and_windows() {
        assert!(parse_since("all").unwrap().is_none());
        assert!(parse_since("").unwrap().is_none());
        assert!(parse_since("7d").unwrap().is_some());
        assert!(parse_since("24h").unwrap().is_some());
        assert!(parse_since("30m").unwrap().is_some());
        assert!(parse_since("2w").unwrap().is_some());
    }

    #[test]
    fn parse_since_accepts_rfc3339() {
        let s = "2026-04-21T12:00:00+00:00";
        assert_eq!(parse_since(s).unwrap().as_deref(), Some(s));
    }

    #[test]
    fn parse_format_aliases() {
        assert_eq!(parse_format("").unwrap(), OutputFormat::Jsonl);
        assert_eq!(parse_format("jsonl").unwrap(), OutputFormat::Jsonl);
        assert_eq!(parse_format("otlp").unwrap(), OutputFormat::Otlp);
        assert_eq!(parse_format("OTLP").unwrap(), OutputFormat::Otlp);
        assert!(parse_format("yaml").is_err());
    }
}
