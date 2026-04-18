//! `rememora usage` — aggregate agent-invocation telemetry.
//!
//! Reads from the `agent_invocations` table (migration 004), grouped by
//! caller / model / project / session / total, since a relative cutoff.
//! By default the CLI uses `--since all` so it shows usage across all
//! sessions, not just the current time window.

use anyhow::{bail, Context, Result};
use chrono::{Duration, Utc};
use rusqlite::Connection;

use rememora::models::agent_invocation::{self, GroupBy, UsageAggregate};

pub struct UsageArgs {
    pub since: String,
    pub by: String,
}

pub fn run(conn: &Connection, args: &UsageArgs, json_output: bool) -> Result<()> {
    let since = parse_since(&args.since).context("invalid --since value")?;
    let group_by = parse_group_by(&args.by).context("invalid --by value")?;

    let rows = agent_invocation::aggregate(conn, since.as_deref(), group_by)?;

    if json_output {
        let out = serde_json::json!({
            "since": args.since,
            "by": args.by,
            "rows": rows,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print_table(&args.by, &args.since, &rows);
    }

    Ok(())
}

/// Turn a relative window like `7d`, `24h`, `30m`, or `all` into an ISO8601
/// cutoff. `all` returns `None` (no WHERE on ts).
fn parse_since(s: &str) -> Result<Option<String>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("all") || s.is_empty() {
        return Ok(None);
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
        other => bail!("unknown time unit `{other}` (use m/h/d/w or `all`)"),
    };

    let cutoff = Utc::now() - duration;
    Ok(Some(cutoff.to_rfc3339()))
}

fn parse_group_by(s: &str) -> Result<GroupBy> {
    match s.to_ascii_lowercase().as_str() {
        "total" | "all" => Ok(GroupBy::Total),
        "caller" => Ok(GroupBy::Caller),
        "model" => Ok(GroupBy::Model),
        "project" => Ok(GroupBy::Project),
        "session" | "parent_session" => Ok(GroupBy::ParentSession),
        other => bail!("unknown --by `{other}` (expected total|caller|model|project|session)"),
    }
}

fn print_table(by: &str, since: &str, rows: &[UsageAggregate]) {
    if rows.is_empty() {
        if since.trim().eq_ignore_ascii_case("all") || since.trim().is_empty() {
            println!("No agent invocations recorded.");
        } else {
            println!("No agent invocations recorded in this window.");
        }
        return;
    }

    let label = match by {
        "total" | "all" => "scope",
        other => other,
    };

    println!(
        "{:<28} {:>6} {:>6} {:>12} {:>12} {:>14} {:>10} {:>10}",
        label, "calls", "errs", "in_tok", "out_tok", "cache_read_tok", "avg_ms", "cost_usd"
    );
    println!("{}", "-".repeat(112));

    let mut total_invocations = 0i64;
    let mut total_cost = 0.0f64;
    let mut total_in = 0i64;
    let mut total_out = 0i64;
    let mut total_cache_read = 0i64;

    for row in rows {
        total_invocations += row.invocations;
        total_cost += row.cost_usd;
        total_in += row.input_tokens;
        total_out += row.output_tokens;
        total_cache_read += row.cache_read_tokens;

        println!(
            "{:<28} {:>6} {:>6} {:>12} {:>12} {:>14} {:>10} {:>10}",
            truncate(&row.bucket, 28),
            row.invocations,
            row.errors,
            row.input_tokens,
            row.output_tokens,
            row.cache_read_tokens,
            row.avg_duration_ms
                .map(|v| format!("{:.0}", v))
                .unwrap_or_else(|| "-".into()),
            format!("{:.4}", row.cost_usd),
        );
    }

    if rows.len() > 1 {
        println!("{}", "-".repeat(112));
        println!(
            "{:<28} {:>6} {:>6} {:>12} {:>12} {:>14} {:>10} {:>10}",
            "TOTAL",
            total_invocations,
            "",
            total_in,
            total_out,
            total_cache_read,
            "",
            format!("{:.4}", total_cost),
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_windows() {
        assert!(parse_since("7d").unwrap().is_some());
        assert!(parse_since("24h").unwrap().is_some());
        assert!(parse_since("30m").unwrap().is_some());
        assert!(parse_since("2w").unwrap().is_some());
    }

    #[test]
    fn all_means_no_cutoff() {
        assert!(parse_since("all").unwrap().is_none());
        assert!(parse_since("ALL").unwrap().is_none());
    }

    #[test]
    fn rejects_unknown_units() {
        assert!(parse_since("5x").is_err());
        assert!(parse_since("abc").is_err());
    }

    #[test]
    fn group_by_aliases() {
        assert_eq!(parse_group_by("caller").unwrap(), GroupBy::Caller);
        assert_eq!(parse_group_by("Project").unwrap(), GroupBy::Project);
        assert_eq!(parse_group_by("session").unwrap(), GroupBy::ParentSession);
        assert_eq!(parse_group_by("total").unwrap(), GroupBy::Total);
        assert!(parse_group_by("weird").is_err());
    }
}
