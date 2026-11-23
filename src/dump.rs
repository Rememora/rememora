//! Complete JSON export and import (`rememora export --all`, `rememora import`).
//!
//! Every row of every table that carries user data is written column by
//! column, so the file rebuilds a database: contexts (projects included, with
//! `superseded_by` links, tags, importance, counts, timestamps, provenance),
//! sessions, relations and the curator and telemetry tables. Embeddings are
//! left out: they are derived from content and rebuilt on demand.

use anyhow::{bail, Context, Result};
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const FORMAT: &str = "rememora-export";
pub const FORMAT_VERSION: i64 = 1;

/// Tables in the export, in import order, with their primary key column.
pub const TABLES: &[(&str, &str)] = &[
    ("contexts", "id"),
    ("sessions", "id"),
    ("relations", "id"),
    ("watermarks", "file_path"),
    ("curator_log", "id"),
    ("consolidation_runs", "id"),
    ("agent_invocations", "id"),
    ("hook_invocations", "id"),
];

fn to_json(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(b.iter().map(|x| format!("{x:02x}")).collect()),
    }
}

fn to_sql(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(*b as i64),
        Value::Number(n) => match n.as_i64() {
            Some(i) => SqlValue::Integer(i),
            None => SqlValue::Real(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    // Table names come from the compile-time `TABLES` list only.
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
    Ok(cols)
}

/// The whole database as one JSON document.
pub fn export_all(conn: &Connection) -> Result<Value> {
    let mut tables = Map::new();
    for (table, _) in TABLES {
        let cols = columns(conn, table)?;
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut obj = Map::new();
            for (i, c) in cols.iter().enumerate() {
                obj.insert(c.clone(), to_json(row.get_ref(i)?));
            }
            out.push(Value::Object(obj));
        }
        tables.insert(table.to_string(), Value::Array(out));
    }
    let migrations: i64 = conn.query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))?;
    Ok(serde_json::json!({
        "format": FORMAT,
        "format_version": FORMAT_VERSION,
        "exported_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "rememora_version": env!("CARGO_PKG_VERSION"),
        "migrations": migrations,
        "tables": tables,
    }))
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TableReport {
    pub inserted: usize,
    pub skipped: usize,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ImportReport {
    pub dry_run: bool,
    pub tables: BTreeMap<String, TableReport>,
}

/// Load an `export_all` document into `conn` in one transaction. Rows whose
/// primary key (or another unique key, such as a context URI) already exists
/// are skipped, so importing the same file twice changes nothing. Columns the
/// target table does not have are ignored. `dry_run` rolls back at the end.
pub fn import(conn: &Connection, doc: &Value, dry_run: bool) -> Result<ImportReport> {
    if doc.get("format").and_then(Value::as_str) != Some(FORMAT) {
        bail!("Not a rememora export (expected \"format\": \"{FORMAT}\"; per-project exports cannot be imported, use `rememora export --all`)");
    }
    let version = doc.get("format_version").and_then(Value::as_i64).unwrap_or(0);
    if version < 1 || version > FORMAT_VERSION {
        bail!("Unsupported export format_version {version} (this build reads 1 to {FORMAT_VERSION})");
    }
    let tables = doc.get("tables").and_then(Value::as_object).context("Export has no \"tables\" object")?;

    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // `superseded_by` and `parent_session` may point at rows later in the file.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;
    let mut report = ImportReport { dry_run, ..Default::default() };
    for (table, _pk) in TABLES {
        let mut tr = TableReport::default();
        let rows = match tables.get(*table) {
            Some(Value::Array(rows)) => rows,
            Some(_) => bail!("\"tables.{table}\" is not an array"),
            None => {
                report.tables.insert(table.to_string(), tr);
                continue;
            }
        };
        let target_cols = columns(&tx, table)?;
        for row in rows {
            let obj = row.as_object().with_context(|| format!("Row in {table} is not an object"))?;
            let cols: Vec<&String> = target_cols.iter().filter(|c| obj.contains_key(c.as_str())).collect();
            if cols.is_empty() {
                tr.skipped += 1;
                continue;
            }
            let names = cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ");
            let marks = vec!["?"; cols.len()].join(", ");
            let sql = format!("INSERT OR IGNORE INTO {table} ({names}) VALUES ({marks})");
            let values: Vec<SqlValue> = cols.iter().map(|c| to_sql(&obj[c.as_str()])).collect();
            let n = tx
                .prepare_cached(&sql)?
                .execute(rusqlite::params_from_iter(values))
                .with_context(|| format!("Failed to import a row into {table}"))?;
            if n == 1 { tr.inserted += 1 } else { tr.skipped += 1 }
        }
        report.tables.insert(table.to_string(), tr);
    }
    if dry_run {
        tx.rollback()?;
    } else {
        tx.commit().context("Import failed at commit (a row references a missing row?)")?;
    }
    Ok(report)
}
