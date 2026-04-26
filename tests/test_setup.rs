//! Integration tests for `rememora setup --apply`.
//!
//! Issue #100 / #101 regression coverage: on hosts without an OS keychain
//! backend (vanilla Linux containers, CI, Docker images), setup must:
//!   * fall back to a 0600 key file at `<data-dir>/key`
//!   * actually create the SQLite DB so the first-run gate passes
//!   * wire the full canonical hook set (including UserPromptSubmit from #87)
//!
//! We run the binary in a subprocess with an overridden `HOME`/`REMEMORA_DB`
//! so the test cannot corrupt the developer's real `~/.rememora` /
//! `~/.claude/settings.json`.

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn setup_apply_no_keychain_creates_db_key_and_hooks() {
    let home = TempDir::new().expect("tempdir");
    let home_path = home.path();

    // Pre-create a fake `claude` binary on PATH so the agent-detection branch
    // for Claude Code fires and writes settings.json. We point HOME at the
    // tempdir so all `~/.claude/...` paths resolve under the test sandbox.
    let bin_dir = home_path.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let claude_stub = bin_dir.join("claude");
    std::fs::write(&claude_stub, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&claude_stub).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&claude_stub, perms).unwrap();
    }

    let db_path = home_path.join(".rememora").join("rememora.db");
    let key_path = home_path.join(".rememora").join("key");
    let settings_path = home_path.join(".claude").join("settings.json");

    let mut cmd = Command::cargo_bin("rememora").expect("binary built");
    // Force the file-fallback branch in `crypto::persist_key` so the test is
    // deterministic regardless of whether the host actually has a keychain.
    cmd.env("REMEMORA_TEST_NO_KEYCHAIN", "1")
        .env("HOME", home_path)
        .env("REMEMORA_DB", &db_path)
        // Restrict PATH to our stub + the original PATH so `which claude`
        // resolves to the stub. Keep system PATH so coreutils are reachable.
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default(),
            ),
        )
        // Keep cliclack from blocking on tty queries.
        .env_remove("CI")
        .arg("setup")
        .arg("--apply");

    let output = cmd.output().expect("run setup");
    assert!(
        output.status.success(),
        "setup --apply failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // 1. Key file must exist with mode 0600.
    assert!(key_path.exists(), "key file at {} not created", key_path.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "key file perms {:o} != 600", mode & 0o777);
    }
    let key = std::fs::read_to_string(&key_path).unwrap();
    assert!(!key.trim().is_empty(), "key file is empty");

    // 2. DB must exist (first-run gate is `db_path.exists()`).
    assert!(db_path.exists(), "DB at {} not created", db_path.display());
    assert!(
        db_path.metadata().unwrap().len() > 0,
        "DB at {} is empty",
        db_path.display(),
    );

    // 3. Claude Code settings.json must contain the canonical hook set,
    //    including UserPromptSubmit (issue #101).
    assert!(
        settings_path.exists(),
        "settings.json at {} not created",
        settings_path.display(),
    );
    let raw = std::fs::read_to_string(&settings_path).unwrap();
    let parsed: Value = serde_json::from_str(&raw).expect("settings.json is valid JSON");
    let hooks = parsed
        .get("hooks")
        .and_then(|v| v.as_object())
        .expect("hooks object present");

    let mut keys: Vec<&str> = hooks.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"],
        "hook keys do not match canonical Rememora hook set",
    );

    // Spot-check the UserPromptSubmit command shape: it must shell out to
    // `rememora search` so PR #87's FTS5-injection behavior is wired.
    let ups = hooks
        .get("UserPromptSubmit")
        .and_then(|v| v.as_array())
        .expect("UserPromptSubmit array");
    let has_search = ups.iter().any(|h| {
        h.get("command")
            .and_then(|c| c.as_str())
            .map(|s| s.contains("rememora search"))
            .unwrap_or(false)
    });
    assert!(
        has_search,
        "UserPromptSubmit hook does not invoke `rememora search`: {:?}",
        ups,
    );

    // Smoke-check the emitted shell command parses under `bash -n`. The hook
    // string contains nested quoting that is easy to break — failing fast in
    // tests beats discovering it on a user's machine.
    let cmd = ups[0].get("command").and_then(|c| c.as_str()).unwrap();
    let parse = std::process::Command::new("bash")
        .arg("-n")
        .arg("-c")
        .arg(cmd)
        .output()
        .expect("spawn bash -n");
    assert!(
        parse.status.success(),
        "UserPromptSubmit command failed bash -n: {}\ncmd was: {}",
        String::from_utf8_lossy(&parse.stderr),
        cmd,
    );
}
