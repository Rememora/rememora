//! Integration tests for `rememora setup --apply`.
//!
//! Issue #100 / #101 regression coverage: on hosts without an OS keychain
//! backend (vanilla Linux containers, CI, Docker images), setup must:
//!   * fall back to a 0600 key file at `<data-dir>/key`
//!   * actually create the SQLite DB so the first-run gate passes
//!
//! Rememora no longer wires any Claude Code / Gemini CLI lifecycle hooks —
//! `setup --apply` only injects the agent instruction snippet and, if a
//! prior version of setup left rememora-managed hook entries behind, strips
//! them so the install self-heals.
//!
//! We run the binary in a subprocess with an overridden `HOME`/`REMEMORA_DB`
//! so the test cannot corrupt the developer's real `~/.rememora` /
//! `~/.claude/settings.json`.

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn stub_claude_binary(home_path: &std::path::Path) -> std::path::PathBuf {
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
    bin_dir
}

#[test]
fn setup_apply_no_keychain_creates_db_key_and_instructions_without_hooks() {
    let home = TempDir::new().expect("tempdir");
    let home_path = home.path();

    let bin_dir = stub_claude_binary(home_path);

    let db_path = home_path.join(".rememora").join("rememora.db");
    let key_path = home_path.join(".rememora").join("key");
    let settings_path = home_path.join(".claude").join("settings.json");
    let instructions_path = home_path.join(".claude").join("CLAUDE.md");

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

    // 3. Instructions must be injected into CLAUDE.md.
    assert!(
        instructions_path.exists(),
        "CLAUDE.md at {} not created",
        instructions_path.display(),
    );
    let instructions = std::fs::read_to_string(&instructions_path).unwrap();
    assert!(
        instructions.contains("## Rememora"),
        "CLAUDE.md missing Rememora instruction snippet",
    );

    // 4. No hooks are wired — setup must not touch/create settings.json at
    //    all when there is nothing to clean up.
    assert!(
        !settings_path.exists(),
        "settings.json should not be created — rememora no longer wires hooks: {}",
        settings_path.display(),
    );

    // 5. No hook scripts are deployed anywhere under ~/.rememora/.
    let hooks_dir = home_path.join(".rememora").join("hooks");
    assert!(
        !hooks_dir.exists(),
        "hooks dir should not exist — rememora no longer deploys hook scripts: {}",
        hooks_dir.display(),
    );
}

/// Self-healing: a settings.json left behind by an older `setup --apply`
/// (which used to wire SessionStart/SessionEnd/Stop/UserPromptSubmit hooks,
/// in both the legacy flat shape and the canonical envelope shape) must have
/// every rememora-managed entry stripped on the next `setup --apply`.
/// User-managed non-rememora hooks in the same event arrays must survive.
#[test]
fn setup_apply_strips_stale_rememora_hooks_from_existing_settings_json() {
    let home = TempDir::new().expect("tempdir");
    let home_path = home.path();

    let bin_dir = stub_claude_binary(home_path);

    let db_path = home_path.join(".rememora").join("rememora.db");
    let settings_path = home_path.join(".claude").join("settings.json");
    let instructions_path = home_path.join(".claude").join("CLAUDE.md");

    // Seed a settings.json as an older rememora version would have left it:
    // a mix of legacy flat-shape and canonical envelope-shape rememora
    // entries, plus a user-managed hook and unrelated settings that must
    // survive. Also pre-seed CLAUDE.md as already configured so this test
    // isolates hook cleanup from instruction injection.
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    let stale = serde_json::json!({
        "permissions": { "allow": ["Bash(echo:*)"] },
        "hooks": {
            "SessionStart": [
                { "type": "command", "command": "rememora context --auto 2>/dev/null || true" },
                { "type": "command", "command": "echo keep-this-user-hook" }
            ],
            "UserPromptSubmit": [
                { "hooks": [{ "type": "command", "command": "bash ~/.rememora/hooks/prompt-search.sh 2>/dev/null || true" }] }
            ],
            "SessionEnd": [
                { "type": "command", "command": "rememora session end-active --auto-summary 2>/dev/null || true" }
            ],
            "Stop": [
                { "hooks": [{ "type": "command", "command": "bash ~/.rememora/hooks/stop-curate.sh 2>/dev/null || true" }] },
                { "hooks": [{ "type": "command", "command": "echo also-keep-this" }] }
            ]
        }
    });
    std::fs::write(&settings_path, serde_json::to_string_pretty(&stale).unwrap()).unwrap();
    std::fs::create_dir_all(instructions_path.parent().unwrap()).unwrap();
    std::fs::write(&instructions_path, "some existing content\n\n## Rememora\nalready here\n").unwrap();

    // Also seed a stale deployed hooks dir, as an older `setup --apply`
    // would have left under ~/.rememora/hooks/.
    let hooks_dir = home_path.join(".rememora").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    std::fs::write(hooks_dir.join("stop-curate.sh"), "#!/usr/bin/env bash\n# stale\n").unwrap();

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
        "non-hooks user settings clobbered by cleanup",
    );

    let hooks = parsed.get("hooks").and_then(|v| v.as_object());

    if let Some(hooks) = hooks {
        // Events that were entirely rememora-managed must be gone.
        assert!(hooks.get("SessionEnd").is_none(), "SessionEnd should be fully removed");
        assert!(hooks.get("UserPromptSubmit").is_none(), "UserPromptSubmit should be fully removed");

        // SessionStart keeps only the user-managed entry.
        if let Some(ss) = hooks.get("SessionStart").and_then(|v| v.as_array()) {
            let has_rememora = ss.iter().any(|e| {
                e.get("command")
                    .and_then(|c| c.as_str())
                    .map(|s| s.contains("rememora context"))
                    .unwrap_or(false)
            });
            assert!(!has_rememora, "SessionStart still has a rememora-managed entry");
            let preserved = ss.iter().any(|e| {
                e.get("command")
                    .and_then(|c| c.as_str())
                    .map(|s| s.contains("keep-this-user-hook"))
                    .unwrap_or(false)
            });
            assert!(preserved, "user-managed non-rememora hook was clobbered");
        }

        // Stop keeps only the user-managed entry.
        if let Some(stop) = hooks.get("Stop").and_then(|v| v.as_array()) {
            for entry in stop {
                if let Some(inner) = entry.get("hooks").and_then(|h| h.as_array()) {
                    for leaf in inner {
                        let cmd = leaf.get("command").and_then(|c| c.as_str()).unwrap_or("");
                        assert!(
                            !cmd.contains(".rememora/hooks/"),
                            "Stop still references a deployed rememora hook script: {cmd}",
                        );
                    }
                }
            }
            let preserved = stop.iter().any(|e| {
                e.get("hooks")
                    .and_then(|h| h.as_array())
                    .and_then(|a| a.first())
                    .and_then(|leaf| leaf.get("command"))
                    .and_then(|c| c.as_str())
                    .map(|s| s.contains("also-keep-this"))
                    .unwrap_or(false)
            });
            assert!(preserved, "user-managed non-rememora Stop hook was clobbered");
        }
    }

    // The stale deployed hooks dir must be removed.
    assert!(
        !hooks_dir.exists(),
        "stale deployed hooks dir should have been removed: {}",
        hooks_dir.display(),
    );
}
