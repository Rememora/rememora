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
    //    including UserPromptSubmit (issue #101) AND it must be wrapped in
    //    Claude Code's envelope shape (issue #107):
    //      "hooks": { "<Event>": [ { "matcher"?, "hooks": [{type, command}] } ] }
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

    // Envelope-shape contract: each event entry must be
    //   { matcher?: string, hooks: [ { type: "command", command: "..." }, ... ] }
    // and must NOT have a top-level `type`/`command` (the old broken flat shape).
    for ev in ["SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"] {
        let arr = hooks
            .get(ev)
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("{ev} array missing"));
        assert!(!arr.is_empty(), "{ev} array is empty");
        for entry in arr {
            assert!(
                entry.get("type").is_none(),
                "{ev} entry has flat-shape `type` (issue #107): {entry:?}",
            );
            assert!(
                entry.get("command").is_none(),
                "{ev} entry has flat-shape `command` (issue #107): {entry:?}",
            );
            if let Some(matcher) = entry.get("matcher") {
                assert!(
                    matcher.as_str().is_some(),
                    "{ev} matcher is not a string: {matcher:?}",
                );
            }
            let inner = entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .unwrap_or_else(|| panic!("{ev} envelope missing inner `hooks` array"));
            assert!(!inner.is_empty(), "{ev} inner hooks array empty");
            for leaf in inner {
                assert_eq!(
                    leaf.get("type").and_then(|t| t.as_str()),
                    Some("command"),
                    "{ev} inner leaf must be type=command: {leaf:?}",
                );
                assert!(
                    leaf.get("command").and_then(|c| c.as_str()).is_some(),
                    "{ev} inner leaf missing command string: {leaf:?}",
                );
            }
        }
    }

    // SessionStart must propagate the matcher from plugin/hooks/hooks.json.
    let ss_first = &hooks
        .get("SessionStart")
        .and_then(|v| v.as_array())
        .unwrap()[0];
    assert_eq!(
        ss_first.get("matcher").and_then(|m| m.as_str()),
        Some("startup|clear|compact|resume"),
        "SessionStart matcher not propagated from plugin manifest",
    );

    // Spot-check the UserPromptSubmit command shape: it must shell out to
    // the deployed `prompt-search.sh` (issue #111). The script — bundled
    // via `include_str!` and redeployed on every `setup --apply` — is what
    // calls `rememora search` and emits the FTS5-injected hits PR #87 added.
    let ups = hooks
        .get("UserPromptSubmit")
        .and_then(|v| v.as_array())
        .expect("UserPromptSubmit array");
    let ups_inner = ups[0]
        .get("hooks")
        .and_then(|h| h.as_array())
        .expect("UserPromptSubmit envelope missing inner hooks");
    let has_deployed_script = ups_inner.iter().any(|h| {
        h.get("command")
            .and_then(|c| c.as_str())
            .map(|s| s.contains(".rememora/hooks/prompt-search.sh"))
            .unwrap_or(false)
    });
    assert!(
        has_deployed_script,
        "UserPromptSubmit hook does not reference deployed prompt-search.sh: {:?}",
        ups,
    );

    // Smoke-check the emitted shell command parses under `bash -n`. The hook
    // string contains nested quoting that is easy to break — failing fast in
    // tests beats discovering it on a user's machine.
    let cmd = ups_inner[0]
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap();
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

    // 4. Issue #111: bundled plugin scripts must be deployed under
    //    <REMEMORA_DB parent>/hooks/ (i.e. ~/.rememora/hooks/ in production)
    //    with mode 0755, and the Stop-hook script must contain the
    //    `record-hook-event` telemetry calls that populate `hook_invocations`.
    let hooks_dir = home_path.join(".rememora").join("hooks");
    for name in ["session-start.sh", "session-end.sh", "stop-curate.sh", "prompt-search.sh"] {
        let p = hooks_dir.join(name);
        assert!(p.exists(), "{} not deployed at {}", name, p.display());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "{} perms 0o{:o} != 0o755", name, mode);
        }
    }
    let stop_curate = std::fs::read_to_string(hooks_dir.join("stop-curate.sh")).unwrap();
    assert!(
        stop_curate.contains("rememora debug record-hook-event"),
        "deployed stop-curate.sh missing record-hook-event telemetry — \
         hook_invocations will not populate for CLI installs (#111)",
    );
    let prompt_search = std::fs::read_to_string(hooks_dir.join("prompt-search.sh")).unwrap();
    assert!(
        prompt_search.contains("rememora search"),
        "deployed prompt-search.sh missing `rememora search` invocation",
    );
}

/// Issue #107 migration: an existing settings.json written by the broken
/// pre-fix setup contains flat-shape entries that Claude Code silently
/// rejects. Re-running `setup --apply` must heal it in place: rememora
/// entries get wrapped in the canonical envelope; user-managed
/// non-rememora hooks are preserved untouched.
#[test]
fn setup_apply_migrates_broken_flat_shape_settings_json() {
    let home = TempDir::new().expect("tempdir");
    let home_path = home.path();

    // Stub `claude` binary so the agent-detection branch fires.
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
    let settings_path = home_path.join(".claude").join("settings.json");

    // Seed the broken flat-shape settings.json plus a non-rememora hook to
    // verify it survives migration.
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    let broken = serde_json::json!({
        "permissions": { "allow": ["Bash(echo:*)"] },
        "hooks": {
            "SessionStart": [
                { "type": "command", "command": "rememora context --auto 2>/dev/null || true" },
                { "type": "command", "command": "echo keep-this-user-hook" }
            ],
            "SessionEnd": [
                { "type": "command", "command": "rememora session end-active --auto-summary 2>/dev/null || true" }
            ],
            "Stop": [
                { "type": "command", "command": "bash -c '(rememora curate --auto 2>/dev/null || true) &'" }
            ]
            // UserPromptSubmit deliberately omitted to exercise the
            // missing-event branch as well.
        }
    });
    std::fs::write(&settings_path, serde_json::to_string_pretty(&broken).unwrap()).unwrap();

    let mut cmd = Command::cargo_bin("rememora").expect("binary built");
    cmd.env("REMEMORA_TEST_NO_KEYCHAIN", "1")
        .env("HOME", home_path)
        .env("REMEMORA_DB", &db_path)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default(),
            ),
        )
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

    let raw = std::fs::read_to_string(&settings_path).unwrap();
    let parsed: Value = serde_json::from_str(&raw).expect("settings.json valid JSON");

    // Non-hooks user content survives.
    assert_eq!(
        parsed
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .map(|a| a.len()),
        Some(1),
        "non-hooks user settings clobbered by migration",
    );

    let hooks = parsed.get("hooks").and_then(|v| v.as_object()).unwrap();

    // All four canonical events present after migration.
    let mut keys: Vec<&str> = hooks.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"],
    );

    // Each event must contain at least one envelope-shape rememora entry,
    // and zero flat-shape rememora entries.
    for ev in ["SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"] {
        let arr = hooks.get(ev).and_then(|v| v.as_array()).unwrap();
        let mut envelope_count = 0usize;
        for entry in arr {
            // Any flat-shape rememora entry is a migration failure.
            if let Some(cmd) = entry.get("command").and_then(|c| c.as_str()) {
                let is_rmm = cmd.contains("rememora context")
                    || cmd.contains("rememora session")
                    || cmd.contains("rememora curate")
                    || cmd.contains("rememora search");
                assert!(
                    !is_rmm,
                    "{ev} still has flat-shape rememora entry: {cmd}",
                );
            }
            if entry.get("hooks").and_then(|h| h.as_array()).is_some() {
                envelope_count += 1;
            }
        }
        assert!(
            envelope_count >= 1,
            "{ev} has no envelope-shape entry after migration",
        );
    }

    // The user-managed `echo keep-this-user-hook` must survive on SessionStart.
    let ss = hooks.get("SessionStart").and_then(|v| v.as_array()).unwrap();
    let preserved = ss.iter().any(|e| {
        e.get("command")
            .and_then(|c| c.as_str())
            .map(|s| s.contains("keep-this-user-hook"))
            .unwrap_or(false)
    });
    assert!(preserved, "user-managed non-rememora hook was clobbered");
}
