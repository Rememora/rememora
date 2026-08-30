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
//!
//! Where a test needs a row the CLI cannot write — a session backdated past
//! the window, a follow-up in the past — it opens the same scratch DB directly
//! and inserts RFC3339 timestamps, the format `session::start` writes. Seeding
//! `datetime('now', ...)` instead is what let two string-comparison defects
//! survive a green test suite.

use assert_cmd::Command;
use rusqlite::Connection;
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

/// Open the scratch DB for the writes the CLI has no command for.
fn direct(db_path: &Path) -> Connection {
    rememora::db::open_with_options(db_path, true).expect("open scratch db")
}

/// A timestamp in the format the real write path uses
/// (`chrono::Utc::now().to_rfc3339()`), offset from now.
fn rfc3339_ago(days: i64, hours: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days) - chrono::Duration::hours(hours)).to_rfc3339()
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
        ms["attributed_memories"], 1,
        "a memory saved inside an active session must be attributed to it; \
         got a structurally-zero save rate instead: {ms}"
    );
    assert_eq!(ms["status"], "available");
    assert_eq!(ms["unattributed_memories"], 0);
    assert_eq!(ms["memories_from_earlier_sessions"], 0);
    assert_eq!(ms["memories_created_in_window"], 1);
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
        report["memory_save_rate"]["attributed_memories"], 1,
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

    assert_eq!(ms["attributed_memories"], 0);
    assert_eq!(
        ms["unattributed_memories"], 1,
        "an unattributable memory must be counted somewhere visible: {ms}"
    );
    assert_eq!(ms["memories_created_in_window"], 1);
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
    assert_eq!(report["memory_save_rate"]["attributed_memories"], 1);
    assert_eq!(report["memory_save_rate"]["sessions_with_zero_saves"], 0);
}

/// The accounting hole. A memory saved today into a session that started
/// before the window is in neither the attributed bucket (its session is
/// outside the window) nor the unattributed one (it *has* a session). Under
/// the old shape it appeared nowhere at all and the report read "nothing
/// happened". Every memory created in the window must be visible somewhere.
#[test]
fn memory_saved_into_a_session_older_than_the_window_is_not_lost() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "longrun", "--description", "test"]);
    let session_id = run(
        &db,
        &[
            "session", "start", "--agent", "claude-code", "--project", "longrun", "--intent",
            "long runner",
        ],
    )
    .trim()
    .to_string();

    // Backdate the session past the window; it stays active, as a session that
    // outlives a 30-day window in the wild does.
    direct(&db)
        .execute(
            "UPDATE sessions SET started_at = ?1 WHERE id = ?2",
            rusqlite::params![rfc3339_ago(40, 0), session_id],
        )
        .unwrap();

    run(
        &db,
        &["save", "saved today inside a long-running session", "--project", "longrun"],
    );

    let ms = eval_json(&db, &["--project", "longrun", "--days", "30"])["memory_save_rate"].clone();

    assert_eq!(
        ms["memories_created_in_window"], 1,
        "the memory was written today; it must appear in the window: {ms}"
    );
    assert_eq!(
        ms["memories_from_earlier_sessions"], 1,
        "attributed to a session that started before the window — reported, \
         not dropped: {ms}"
    );
    // Not smuggled into the rate, and not miscounted as unattributed.
    assert_eq!(ms["attributed_memories"], 0);
    assert_eq!(ms["unattributed_memories"], 0);
    // The rate itself is still honestly unavailable: no session in the window.
    assert_eq!(ms["status"], "unavailable");

    // The text report must show it too, not just the JSON.
    let text = run(&db, &["eval", "--project", "longrun", "--days", "30"]);
    assert!(
        text.contains("in a session started earlier:      1"),
        "the text report must surface the memory as well:\n{text}"
    );
}

/// The buckets must partition: whatever mix of attribution is present, they
/// add up to the memories created in the window.
#[test]
fn save_buckets_account_for_every_memory_in_the_window() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "mix", "--description", "test"]);

    // 1. unattributed — saved with no session open.
    run(&db, &["save", "saved with no session open", "--project", "mix"]);

    // 2. from an earlier session — backdated, still active.
    let old = run(
        &db,
        &["session", "start", "--agent", "codex", "--project", "mix", "--intent", "old"],
    )
    .trim()
    .to_string();
    direct(&db)
        .execute(
            "UPDATE sessions SET started_at = ?1 WHERE id = ?2",
            rusqlite::params![rfc3339_ago(40, 0), old],
        )
        .unwrap();
    run(&db, &["save", "saved inside the backdated session", "--project", "mix"]);
    run(&db, &["session", "end", &old, "--summary", "done"]);

    // 3. attributed — a session that started inside the window.
    run(
        &db,
        &["session", "start", "--agent", "claude-code", "--project", "mix", "--intent", "new"],
    );
    run(&db, &["save", "saved inside the current session", "--project", "mix"]);

    let ms = eval_json(&db, &["--project", "mix", "--days", "30"])["memory_save_rate"].clone();

    assert_eq!(ms["attributed_memories"], 1, "{ms}");
    assert_eq!(ms["memories_from_earlier_sessions"], 1, "{ms}");
    assert_eq!(ms["unattributed_memories"], 1, "{ms}");
    assert_eq!(
        ms["memories_created_in_window"].as_i64().unwrap(),
        ms["attributed_memories"].as_i64().unwrap()
            + ms["memories_from_earlier_sessions"].as_i64().unwrap()
            + ms["unattributed_memories"].as_i64().unwrap(),
        "the buckets must partition the window, leaving nowhere for a memory \
         to disappear: {ms}"
    );
}

/// `--days N` must mean N days. `started_at` is RFC3339 and
/// `datetime('now','-N days')` is not; as raw strings `'T'` (0x54) sorts above
/// `' '` (0x20), so sessions from earlier on the cutoff day tested as inside
/// the window and `--days 30` silently meant "up to 31".
#[test]
fn window_cutoff_compares_instants_not_raw_strings() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "edge", "--description", "test"]);
    let conn = direct(&db);
    conn.execute(
        "INSERT INTO sessions (id, agent, project, started_at, ended_at, status, intent, summary)
         VALUES ('just_out', 'codex', 'edge', ?1, ?2, 'ended', 'before cutoff', 'x')",
        rusqlite::params![rfc3339_ago(30, 2), rfc3339_ago(30, 1)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, agent, project, started_at, ended_at, status, intent, summary)
         VALUES ('just_in', 'codex', 'edge', ?1, ?2, 'ended', 'after cutoff', 'x')",
        rusqlite::params![rfc3339_ago(29, 22), rfc3339_ago(29, 21)],
    )
    .unwrap();

    let report = eval_json(&db, &["--project", "edge", "--days", "30"]);
    assert_eq!(
        report["session_compliance"]["total"], 1,
        "a session two hours before the cutoff instant is outside a 30-day \
         window; only the one two hours after it counts: {}",
        report["session_compliance"]
    );
}

/// Same defect, same file: the transfer pickup bound compared RFC3339
/// `started_at` against `datetime(ended_at, '+1 hour')`, so every same-day
/// pickup failed and the rate read 0.
#[test]
fn transfer_pickup_counts_a_followup_written_in_rfc3339() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "xfer", "--description", "test"]);
    let conn = direct(&db);
    conn.execute(
        "INSERT INTO sessions (id, agent, project, started_at, ended_at, status, intent, summary)
         VALUES ('t1', 'claude-code', 'xfer', ?1, ?2, 'transferred', 'handoff', 'partial')",
        rusqlite::params![rfc3339_ago(2, 2), rfc3339_ago(2, 1)],
    )
    .unwrap();
    // Picked up 30 minutes after the handoff — comfortably inside `+1 hour`.
    conn.execute(
        "INSERT INTO sessions (id, agent, project, started_at, status, intent, parent_session)
         VALUES ('t2', 'codex', 'xfer', ?1, 'active', 'resume', 't1')",
        rusqlite::params![(chrono::Utc::now()
            - chrono::Duration::days(2)
            - chrono::Duration::minutes(30))
        .to_rfc3339()],
    )
    .unwrap();

    let ts = eval_json(&db, &["--project", "xfer", "--days", "30"])["transfer_success"].clone();
    assert_eq!(ts["transferred"], 1, "{ts}");
    assert_eq!(
        ts["picked_up"], 1,
        "the follow-up started 30 minutes after the handoff; the `+1 hour` \
         bound must compare instants, not RFC3339 against `datetime()`: {ts}"
    );
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
    assert_eq!(before["context_load_rate"]["retrieval_reach"]["reached_memories"], 0);
    assert_eq!(before["context_load_rate"]["retrieval_reach"]["corpus_memories"], 1);

    let hits = run(&db, &["--json", "search", "sqlcipher", "--project", "reach"]);
    assert!(hits.contains("sqlcipher"), "fixture must be findable: {hits}");

    let after = eval_json(&db, &["--project", "reach"]);
    let rr = &after["context_load_rate"]["retrieval_reach"];
    assert_eq!(
        rr["reached_memories"], 1,
        "search returned the memory but retrieval reach stayed 0 — the metric \
         has come unhooked from the retrieval path: {rr}"
    );
    assert!(rr["reach_pct"].as_f64().unwrap() > 0.0);
}

/// The other half of the metric's definition, and the reason its doc comment
/// must not say "search". `active_count` is bumped by `commands::get` and
/// `commands::timeline` as well, so a single `rememora get` moves the reach to
/// 100% with no search anywhere. Anyone reading it as search recall is being
/// misled; this test pins the claim so the wording cannot drift back.
#[test]
fn retrieval_reach_moves_on_a_bare_get_with_no_search() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "getreach", "--description", "test"]);
    run(&db, &["save", "wal mode is enabled at open", "--project", "getreach"]);

    let uri: String = direct(&db)
        .query_row(
            "SELECT uri FROM contexts WHERE context_type = 'memory'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    run(&db, &["get", &uri]);

    let rr = eval_json(&db, &["--project", "getreach"])["context_load_rate"]["retrieval_reach"]
        .clone();
    assert_eq!(
        rr["reached_memories"], 1,
        "a bare `get` bumps active_count, so reach is not a search-only \
         signal: {rr}"
    );

    // And the report must not claim search returned anything.
    let text = run(&db, &["eval", "--project", "getreach"]);
    assert!(
        text.contains("reached a caller at least once"),
        "the text report must describe reach as any retrieval path:\n{text}"
    );
    assert!(
        !text.contains("returned by search at least once"),
        "the report must not claim search returned these memories:\n{text}"
    );
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
    assert_eq!(report["memory_save_rate"]["memories_created_in_window"], 0);
    assert_eq!(report["context_load_rate"]["status"], "unavailable");
    assert!(report["context_load_rate"]["retrieval_reach"]["reach_pct"].is_null());
}

/// Two different quantities must not share a field name in one payload.
/// `memory_save_rate` is window- and session-scoped; `retrieval_reach` is
/// all-time and URI-scoped. Both used to expose `total_memories`, inviting the
/// reader to compare them.
#[test]
fn window_counts_and_all_time_counts_do_not_share_a_field_name() {
    let home = TempDir::new().unwrap();
    let db = scratch_db(&home);

    run(&db, &["project", "add", "names", "--description", "test"]);
    run(&db, &["save", "a memory", "--project", "names"]);

    let report = eval_json(&db, &["--project", "names"]);
    let ms = &report["memory_save_rate"];
    let rr = &report["context_load_rate"]["retrieval_reach"];

    assert!(
        ms.get("total_memories").is_none(),
        "window-scoped counts must name their window: {ms}"
    );
    assert!(
        rr.get("total_memories").is_none(),
        "all-time corpus counts must say so: {rr}"
    );
    assert!(ms.get("memories_created_in_window").is_some(), "{ms}");
    assert!(rr.get("corpus_memories").is_some(), "{rr}");
}
