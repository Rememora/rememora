//! Structural-zero regression coverage for `rememora eval`.
//!
//! Two of the metrics that tell the team whether Rememora works were pinned to
//! zero by construction, not by behavior:
//!
//!   * `memory_save_rate` joined on `contexts.source_session`, which
//!     `commands::save` hardcoded to `None`. No user-reachable path ever wrote
//!     the column, so the rate was 0 however much anyone saved.
//!   * `context_load_rate` keyed off `contexts.updated_at` inside a 60-second
//!     window after `started_at` — a window nothing writes into, opened before
//!     the session row exists, and compared across two timestamp formats.
//!
//! The in-module unit tests in `src/commands/eval.rs` seed `source_session`
//! with raw SQL, so they passed against the broken `save`. These tests drive
//! the real binary instead: register a project, start a session, save through
//! the CLI, and read the metric back out of `eval --json`. That is the only
//! arrangement that can catch the write path going quiet again.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Create a scratch home + plain (unencrypted) DB.
///
/// Never touches `~/.rememora/rememora.db`: `REMEMORA_DB` redirects
/// `db::default_db_path`, and the file is pre-created so the first-run gate in
/// `main` passes without invoking `setup`.
fn scratch_db(home: &TempDir) -> PathBuf {
    let db_path = home.path().join("rememora.db");
    let conn = rememora::db::open_with_options(&db_path, true).expect("create scratch db");
    drop(conn);
    db_path
}

fn cli(db_path: &Path) -> Command {
    let mut cmd = Command::cargo_bin("rememora").expect("binary built");
    cmd.env("REMEMORA_DB", db_path)
        .env("REMEMORA_DISABLE_HOOKS", "1")
        .arg("--no-encryption");
    cmd
}

fn run(db_path: &Path, args: &[&str]) -> String {
    let out = cli(db_path).args(args).output().expect("run rememora");
    assert!(
        out.status.success(),
        "`rememora {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

fn eval_json(db_path: &Path, extra: &[&str]) -> Value {
    let mut args = vec!["--json", "eval"];
    args.extend_from_slice(extra);
    serde_json::from_str(&run(db_path, &args)).expect("eval emits valid json")
}

/// The headline regression: a memory saved while a session is open must be
/// attributed to it, so the save rate reflects behavior instead of a schema
/// gap. Fails against a `save` that passes `source_session: None`.
#[test]
fn save_during_active_session_makes_save_rate_nonzero() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "myapp", "--description", "test"]);
    run(
        &db,
        &[
            "session", "start", "--agent", "claude-code", "--project", "myapp", "--intent",
            "regression",
        ],
    );
    run(
        &db,
        &[
            "save",
            "chose SQLite over Postgres for single-binary distribution",
            "--project",
            "myapp",
            "--category",
            "decision",
        ],
    );

    let report = eval_json(&db, &["--project", "myapp"]);
    let ms = &report["memory_save_rate"];

    assert_eq!(
        ms["total_memories"], 1,
        "a memory saved inside an active session must be attributed to it; \
         got a structurally-zero save rate instead: {ms}"
    );
    assert_eq!(ms["status"], "available");
    assert_eq!(ms["unattributed_memories"], 0);
    assert!(
        ms["avg_per_session"].as_f64().unwrap() > 0.0,
        "avg_per_session must be a real rate, got {ms}"
    );
}

/// Attribution must survive without `--project`: the hook-installed
/// `session start` uses `basename(cwd)`, and agents routinely `save` with no
/// flags. Exercises the `detect_from_cwd` path `save` shares with
/// `session end-active`.
#[test]
fn save_resolves_active_session_from_cwd() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    let work = TempDir::new().unwrap();
    // Canonicalize: on macOS `getcwd()` in the child returns `/private/var/...`
    // while `TempDir` hands back `/var/...`, and `detect_from_cwd` compares by
    // string prefix.
    let work_path = std::fs::canonicalize(work.path()).unwrap();

    run(
        &db,
        &[
            "project",
            "add",
            "cwdapp",
            "--path",
            work_path.to_str().unwrap(),
        ],
    );
    run(
        &db,
        &[
            "session", "start", "--agent", "claude-code", "--project", "cwdapp", "--intent", "cwd",
        ],
    );

    let out = cli(&db)
        .current_dir(&work_path)
        .args(["save", "pattern: hooks must exit 0", "--category", "pattern"])
        .output()
        .expect("run rememora save");
    assert!(out.status.success());

    let report = eval_json(&db, &["--project", "cwdapp"]);
    assert_eq!(
        report["memory_save_rate"]["total_memories"], 1,
        "save with no --project must still resolve the active session from cwd"
    );
}

/// A save with no session open is legitimate. It must be reported as
/// unattributed — visibly — rather than disappear into a rate of zero.
#[test]
fn save_without_session_is_reported_as_unattributed() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "solo", "--description", "test"]);
    run(&db, &["save", "a memory with nobody watching", "--project", "solo"]);

    let report = eval_json(&db, &["--project", "solo"]);
    let ms = &report["memory_save_rate"];

    assert_eq!(ms["total_memories"], 0);
    assert_eq!(
        ms["unattributed_memories"], 1,
        "an unattributable memory must be counted somewhere visible: {ms}"
    );
}

/// Ending a session must not retroactively strip attribution — the join is on
/// session id, not on `status = 'active'`.
#[test]
fn attribution_survives_session_end() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "ending", "--description", "test"]);
    let session_id = run(
        &db,
        &[
            "session", "start", "--agent", "codex", "--project", "ending", "--intent", "work",
        ],
    )
    .trim()
    .to_string();
    run(&db, &["save", "fixed the FD leak", "--project", "ending", "--category", "case"]);
    run(&db, &["session", "end", &session_id, "--summary", "done"]);

    let report = eval_json(&db, &["--project", "ending"]);
    assert_eq!(report["memory_save_rate"]["total_memories"], 1);
    assert_eq!(report["memory_save_rate"]["sessions_with_zero_saves"], 0);
}

/// `--agent` is optional on `save` and no caller passes it, so the per-agent
/// memory count must come from the session, not from `contexts.source_agent`.
#[test]
fn per_agent_breakdown_counts_memories_saved_without_agent_flag() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "agents", "--description", "test"]);
    run(
        &db,
        &[
            "session", "start", "--agent", "claude-code", "--project", "agents", "--intent", "x",
        ],
    );
    // Deliberately no `--agent` — this is how every real save is written.
    run(&db, &["save", "memory from claude", "--project", "agents"]);

    let report = eval_json(&db, &["--project", "agents"]);
    let agents = report["per_agent"].as_array().unwrap();
    let claude = agents
        .iter()
        .find(|a| a["agent"] == "claude-code")
        .expect("claude-code row");

    assert_eq!(
        claude["memories"], 1,
        "per-agent memories must attribute through the session: {claude}"
    );
}

/// The load rate has no signal behind it. It must say "unavailable" and must
/// not ship a percentage field that consumers can mistake for a measurement.
#[test]
fn context_load_rate_reports_unavailable_never_a_fake_zero() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "loads", "--description", "test"]);
    run(
        &db,
        &[
            "session", "start", "--agent", "claude-code", "--project", "loads", "--intent", "x",
        ],
    );

    let report = eval_json(&db, &["--project", "loads"]);
    let cl = &report["context_load_rate"];

    assert_eq!(cl["status"], "unavailable");
    assert!(
        cl["unavailable_reason"].is_string(),
        "an unavailable metric must explain itself: {cl}"
    );
    assert!(
        cl.get("load_pct").is_none(),
        "the fake percentage must be gone from the payload entirely: {cl}"
    );
    assert_eq!(cl["sessions_in_window"], 1);

    // And the human-readable report must not print a bare 0%.
    let text = run(&db, &["eval", "--project", "loads"]);
    assert!(
        text.contains("UNAVAILABLE"),
        "text report must flag the metric as unavailable:\n{text}"
    );
}

/// Structural-zero guard for the retrieval proxy that replaced the
/// `updated_at` window: run a real search, then require the reach to move.
#[test]
fn retrieval_reach_moves_after_a_real_search() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "reach", "--description", "test"]);
    run(
        &db,
        &[
            "save",
            "sqlcipher keeps the database encrypted at rest",
            "--project",
            "reach",
        ],
    );

    let before = eval_json(&db, &["--project", "reach"]);
    assert_eq!(before["context_load_rate"]["retrieval_reach"]["retrieved_memories"], 0);
    assert_eq!(before["context_load_rate"]["retrieval_reach"]["total_memories"], 1);

    let hits = run(&db, &["--json", "search", "sqlcipher", "--project", "reach"]);
    assert!(hits.contains("sqlcipher"), "fixture must be findable: {hits}");

    let after = eval_json(&db, &["--project", "reach"]);
    let rr = &after["context_load_rate"]["retrieval_reach"];
    assert_eq!(
        rr["retrieved_memories"], 1,
        "search returned the memory but retrieval reach stayed 0 — the metric \
         has come unhooked from the retrieval path: {rr}"
    );
    assert!(rr["reach_pct"].as_f64().unwrap() > 0.0);
}

/// An empty database must produce nulls and "unavailable", not a page of 0.0%.
#[test]
fn empty_db_reports_unavailable_rather_than_zero() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    let report = eval_json(&db, &[]);

    assert_eq!(report["memory_save_rate"]["status"], "unavailable");
    assert!(report["memory_save_rate"]["avg_per_session"].is_null());
    assert!(report["memory_save_rate"]["zero_save_pct"].is_null());
    assert_eq!(report["context_load_rate"]["status"], "unavailable");
    assert!(report["context_load_rate"]["retrieval_reach"]["reach_pct"].is_null());
}
