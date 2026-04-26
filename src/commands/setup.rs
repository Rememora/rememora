use anyhow::{Context, Result};
use std::path::PathBuf;

const REMEMORA_MARKER: &str = "## Rememora";
const REMEMORA_HOOK_MARKER: &str = "rememora context --auto";
const REMEMORA_CURATE_MARKER: &str = "rememora curate";
const REMEMORA_PROMPT_HOOK_MARKER: &str = "rememora search --limit 3";

/// Canonical Rememora hook list written by `setup --apply`.
///
/// This is the single source of truth for which hooks setup wires into each
/// agent's `settings.json`. It mirrors `plugin/.claude-plugin/hooks.json` in
/// shape, but resolves to standalone shell commands (no `${CLAUDE_PLUGIN_ROOT}`)
/// so users on the CLI-only install path get the same behavior as plugin
/// users.
///
/// TODO(#101): drive this list from `plugin/hooks/hooks.json` at build time
/// (e.g. via `include_str!` + serde) so we cannot drift from the manifest.
/// For now the `setup_writes_canonical_hook_set` test asserts the keys match.
struct HookSpec {
    /// Top-level hook event name (`SessionStart`, `UserPromptSubmit`, ...).
    event: &'static str,
    /// Marker substring used to detect a pre-existing Rememora entry so we
    /// do not write the hook twice if the user re-runs `setup --apply`.
    marker: &'static str,
    /// Shell command written to `settings.json`.
    command: &'static str,
}

fn rememora_hooks() -> &'static [HookSpec] {
    // Keep this list in lock-step with `plugin/.claude-plugin/hooks.json`.
    // Order matters only for human-readable diffs; settings.json itself is
    // a JSON object.
    &[
        HookSpec {
            event: "SessionStart",
            // "rememora context --auto" — narrow enough that user-pasted hooks
            // with arbitrary other rememora commands won't false-match.
            marker: REMEMORA_HOOK_MARKER,
            command: "rememora context --auto 2>/dev/null || true",
        },
        HookSpec {
            event: "UserPromptSubmit",
            // Mirrors plugin/scripts/prompt-search.sh: bounded FTS5 hits via
            // `--format context`. Inlined here so the CLI-only install path
            // does not depend on plugin scripts being present on disk.
            marker: REMEMORA_PROMPT_HOOK_MARKER,
            // Defense-in-depth for #103: even though `rememora search` falls
            // back to a literal-token query on FTS5 syntax errors, strip the
            // word-bounded operators (OR/AND/NOT/NEAR) and structural punct
            // here too so the search call sees a plain bag of words.
            command: "bash -c 'p=$(cat 2>/dev/null | python3 -c \"import sys,json,re;d=json.load(sys.stdin);s=d.get(\\\"prompt\\\",\\\"\\\");s=re.sub(r\\\"\\\\b(OR|AND|NOT|NEAR)\\\\b\\\",\\\" \\\",s);s=re.sub(r\\\"[\\\\\\\"()*?:\\\\-]\\\",\\\" \\\",s);print(re.sub(r\\\"\\\\s+\\\",\\\" \\\",s).strip())\" 2>/dev/null); [ ${#p} -ge 6 ] && rememora search --limit 3 --format context \"$p\" 2>/dev/null || true'",
        },
        HookSpec {
            event: "SessionEnd",
            marker: "rememora session end-active",
            command: "rememora session end-active --auto-summary 2>/dev/null || true",
        },
        HookSpec {
            event: "Stop",
            marker: REMEMORA_CURATE_MARKER,
            command: "bash -c '(rememora curate --auto 2>/dev/null || true) &'",
        },
    ]
}

// ---------------------------------------------------------------------------
// Instruction snippets — behavioral triggers + urgency framing (Layer 2 + 3)
// Session start/end moved to hooks (Layer 1) — see write_hooks()
// ---------------------------------------------------------------------------

const CLAUDE_CODE_SNIPPET: &str = r#"
## Rememora — Cross-Agent Persistent Memory

You have access to `rememora`, a persistent memory system shared across agents and sessions.
Your context will be lost when this session ends. Anything not saved to rememora is gone forever.
You are not the only agent on this project — save what others will need, search before you assume.

### When to SEARCH (before acting)
- Before implementing anything non-trivial → `rememora search "relevant topic" --project <name>`
- When encountering unfamiliar code or architecture → search for entity knowledge
- When the user references past work or decisions → search for that context
- When stuck or blocked → search for related cases and patterns

### When to SAVE (as you work)
Save immediately when any of these happen — do not batch or defer:
- **Decision made**: team chose an approach, trade-off, or technology → `rememora save "..." --category decision --importance 0.8 --project <name>`
- **Bug solved**: non-trivial fix, workaround, or gotcha discovered → `rememora save "..." --category case --project <name>`
- **Pattern found**: convention, idiom, or reusable approach in the codebase → `rememora save "..." --category pattern --project <name>`
- **User corrected you** or stated a preference → `rememora save "..." --category preference`
- **Entity discovered**: service, API, config, key integration point → `rememora save "..." --category entity --project <name>`

### What NOT to save
- Code that can be read from files (use file paths instead)
- Git history (use `git log`)
- Anything already in the project README or docs
- Temporary debugging state

### Sessions
- Start: `rememora session start --agent claude-code --project <name> --intent "..."`
- End: `rememora session end <id> --summary "..." --working-state "..."`
- Transfer: `rememora session end <id> --status transferred --summary "..." --working-state "..."`
"#;

const CODEX_SNIPPET: &str = r#"
## Rememora — Cross-Agent Persistent Memory

You have access to `rememora`, a persistent memory system shared across agents and sessions.
Your context will be lost when this session ends. Anything not saved to rememora is gone forever.
You are not the only agent on this project — save what others will need, search before you assume.

### When to SEARCH (before acting)
- Before implementing anything non-trivial → `rememora search "relevant topic" --project <name>`
- When encountering unfamiliar code or architecture → search for entity knowledge
- When the user references past work or decisions → search for that context
- When stuck or blocked → search for related cases and patterns

### When to SAVE (as you work)
Save immediately when any of these happen — do not batch or defer:
- **Decision made**: team chose an approach, trade-off, or technology → `rememora save "..." --category decision --importance 0.8 --project <name>`
- **Bug solved**: non-trivial fix, workaround, or gotcha discovered → `rememora save "..." --category case --project <name>`
- **Pattern found**: convention, idiom, or reusable approach → `rememora save "..." --category pattern --project <name>`
- **User corrected you** or stated a preference → `rememora save "..." --category preference`
- **Entity discovered**: service, API, config, key integration point → `rememora save "..." --category entity --project <name>`

### What NOT to save
- Code that can be read from files (use file paths instead)
- Git history (use `git log`)
- Anything already in the project README or docs
- Temporary debugging state

### Sessions
- Start: `rememora session start --agent codex --project <name> --intent "..."`
- End: `rememora session end <id> --summary "..." --working-state "..."`
- Transfer: `rememora session end <id> --status transferred --summary "..." --working-state "..."`
"#;

const GEMINI_SNIPPET: &str = r#"
## Rememora — Cross-Agent Persistent Memory

You have access to `rememora`, a persistent memory system shared across agents and sessions.
Your context will be lost when this session ends. Anything not saved to rememora is gone forever.
You are not the only agent on this project — save what others will need, search before you assume.

### When to SEARCH (before acting)
- Before implementing anything non-trivial → `rememora search "relevant topic" --project <name>`
- When encountering unfamiliar code or architecture → search for entity knowledge
- When the user references past work or decisions → search for that context
- When stuck or blocked → search for related cases and patterns

### When to SAVE (as you work)
Save immediately when any of these happen — do not batch or defer:
- **Decision made**: team chose an approach, trade-off, or technology → `rememora save "..." --category decision --importance 0.8 --project <name>`
- **Bug solved**: non-trivial fix, workaround, or gotcha discovered → `rememora save "..." --category case --project <name>`
- **Pattern found**: convention, idiom, or reusable approach → `rememora save "..." --category pattern --project <name>`
- **User corrected you** or stated a preference → `rememora save "..." --category preference`
- **Entity discovered**: service, API, config, key integration point → `rememora save "..." --category entity --project <name>`

### What NOT to save
- Code that can be read from files (use file paths instead)
- Git history (use `git log`)
- Anything already in the project README or docs
- Temporary debugging state

### Sessions
- Start: `rememora session start --agent gemini --project <name> --intent "..."`
- End: `rememora session end <id> --summary "..." --working-state "..."`
- Transfer: `rememora session end <id> --status transferred --summary "..." --working-state "..."`
"#;

struct AgentConfig {
    name: &'static str,
    /// Path to the instruction/markdown file
    config_path: PathBuf,
    snippet: &'static str,
    /// Path to the settings/hooks JSON file (if hooks are supported)
    hooks_path: Option<PathBuf>,
    /// Note to display about hooks (e.g., feature-gate warning)
    hooks_note: Option<&'static str>,
}

fn home() -> PathBuf {
    dirs::home_dir().expect("Could not determine home directory")
}

fn detect_agents() -> Vec<AgentConfig> {
    let mut agents = Vec::new();

    // Claude Code — binary: claude, instructions: ~/.claude/CLAUDE.md, hooks: ~/.claude/settings.json
    if binary_exists("claude") {
        agents.push(AgentConfig {
            name: "Claude Code",
            config_path: home().join(".claude").join("CLAUDE.md"),
            snippet: CLAUDE_CODE_SNIPPET,

            hooks_path: Some(home().join(".claude").join("settings.json")),
            hooks_note: None,
        });
    }

    // Codex — binary: codex, instructions: ~/.codex/AGENTS.md
    if binary_exists("codex") {
        agents.push(AgentConfig {
            name: "Codex",
            config_path: home().join(".codex").join("AGENTS.md"),
            snippet: CODEX_SNIPPET,

            hooks_path: None,
            hooks_note: Some("Codex hooks require `codex_hooks = true` in config.toml (experimental)"),
        });
    }

    // Gemini CLI — binary: gemini, instructions: ~/.gemini/GEMINI.md, hooks: ~/.gemini/settings.json
    if binary_exists("gemini") {
        agents.push(AgentConfig {
            name: "Gemini CLI",
            config_path: home().join(".gemini").join("GEMINI.md"),
            snippet: GEMINI_SNIPPET,

            hooks_path: Some(home().join(".gemini").join("settings.json")),
            hooks_note: None,
        });
    }

    agents
}

fn binary_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn already_configured(path: &PathBuf) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        content.contains(REMEMORA_MARKER)
    } else {
        false
    }
}

fn hooks_already_configured(path: &PathBuf) -> bool {
    // We require *all* canonical hook markers to be present; if any are
    // missing (e.g. an older install pre-dating UserPromptSubmit), the
    // hooks file should be re-touched so setup wires the missing ones.
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    rememora_hooks().iter().all(|h| content.contains(h.marker))
}

fn tilde_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(suffix) = path.strip_prefix(&home) {
            return format!("~/{}", suffix.display());
        }
    }
    path.display().to_string()
}

/// Merge rememora hooks into an existing settings.json, preserving all other content.
///
/// Iterates `rememora_hooks()` so adding/removing a hook is a one-line change
/// and the unit test stays in lock-step with what setup actually writes.
fn write_hooks(path: &PathBuf) -> Result<()> {
    let mut root: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&content)?
        }
    } else {
        serde_json::json!({})
    };

    let hooks = root
        .as_object_mut()
        .expect("settings.json must be an object")
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_obj = hooks
        .as_object_mut()
        .expect("hooks must be an object");

    for spec in rememora_hooks() {
        let entry = hooks_obj
            .entry(spec.event.to_string())
            .or_insert_with(|| serde_json::json!([]));
        let Some(arr) = entry.as_array_mut() else {
            // Some other tool wrote a non-array under this key — leave it
            // alone rather than clobbering user state.
            continue;
        };
        let already = arr.iter().any(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|s| s.contains(spec.marker))
                .unwrap_or(false)
        });
        if !already {
            arr.push(serde_json::json!({
                "type": "command",
                "command": spec.command,
            }));
        }
    }

    // Write back with pretty formatting
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let formatted = serde_json::to_string_pretty(&root)?;
    std::fs::write(path, formatted)?;

    Ok(())
}

pub fn run(apply: bool) -> Result<()> {
    cliclack::intro("rememora setup")?;

    // --- Step 1: Encryption ---
    setup_encryption()?;

    // --- Step 2: Agent configuration ---
    let spinner = cliclack::spinner();
    spinner.start("Scanning for AI agents...");

    let agents = detect_agents();

    if agents.is_empty() {
        spinner.stop("No agents found");
        cliclack::log::warning(
            "No supported agents detected.\n\
             Rememora works with: Claude Code (claude), Codex (codex), Gemini CLI (gemini)",
        )?;
        cliclack::outro("Nothing to configure.")?;
        return Ok(());
    }

    let mut actions: Vec<(&AgentConfig, Action)> = Vec::new();

    for agent in &agents {
        let instructions_done = already_configured(&agent.config_path);
        let hooks_done = agent.hooks_path.as_ref().map(hooks_already_configured).unwrap_or(true);

        let action = if instructions_done && hooks_done {
            Action::AlreadyConfigured
        } else {
            Action::NeedsWork {
                instructions: !instructions_done,
                hooks: !hooks_done && agent.hooks_path.is_some(),
            }
        };
        actions.push((agent, action));
    }

    spinner.stop("Scanning for AI agents...");

    // Display agent status lines
    for (agent, action) in &actions {
        let path = tilde_path(&agent.config_path);
        match action {
            Action::AlreadyConfigured => {
                cliclack::log::success(format!(
                    "{:<13} {} — already configured",
                    agent.name, path,
                ))?;
            }
            Action::NeedsWork { instructions, hooks } => {
                let mut parts = Vec::new();
                if *instructions {
                    if agent.config_path.exists() {
                        parts.push("instructions (append)");
                    } else {
                        parts.push("instructions (create)");
                    }
                }
                if *hooks {
                    if let Some(hp) = &agent.hooks_path {
                        if hp.exists() {
                            parts.push("hooks (merge)");
                        } else {
                            parts.push("hooks (create)");
                        }
                    }
                }
                cliclack::log::info(format!(
                    "{:<13} {} — will configure: {}",
                    agent.name, path, parts.join(", "),
                ))?;
            }
        }

        // Show hooks note if applicable
        if let Some(note) = agent.hooks_note {
            cliclack::log::warning(format!("  ⚠ {}", note))?;
        }
    }

    let pending: Vec<_> = actions
        .iter()
        .filter(|(_, a)| !matches!(a, Action::AlreadyConfigured))
        .collect();

    if pending.is_empty() {
        cliclack::outro("All agents already configured.")?;
        return Ok(());
    }

    // Determine whether to proceed
    let should_apply = if apply {
        true
    } else {
        let count = pending.len();
        let prompt = format!(
            "Configure {} agent{}?",
            count,
            if count == 1 { "" } else { "s" }
        );
        cliclack::confirm(prompt).interact()?
    };

    if !should_apply {
        cliclack::outro("Setup cancelled.")?;
        return Ok(());
    }

    // Apply changes
    for (agent, action) in &actions {
        let (needs_instructions, needs_hooks) = match action {
            Action::AlreadyConfigured => continue,
            Action::NeedsWork { instructions, hooks } => (*instructions, *hooks),
        };

        // --- Instructions ---
        if needs_instructions {
            if let Some(parent) = agent.config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Backup existing file
            if agent.config_path.exists() {
                let backup = agent.config_path.with_extension("md.bak");
                std::fs::copy(&agent.config_path, &backup)?;
            }

            let mut content = if agent.config_path.exists() {
                std::fs::read_to_string(&agent.config_path)?
            } else {
                String::new()
            };
            content.push_str(agent.snippet);
            std::fs::write(&agent.config_path, content)?;

            cliclack::log::success(format!("{}: instructions configured", agent.name))?;
        }

        // --- Hooks ---
        if needs_hooks {
            if let Some(hooks_path) = &agent.hooks_path {
                // Backup existing hooks file
                if hooks_path.exists() {
                    let backup = hooks_path.with_extension("json.bak");
                    std::fs::copy(hooks_path, &backup)?;
                }

                write_hooks(hooks_path)?;
                cliclack::log::success(format!(
                    "{}: hooks configured ({})",
                    agent.name,
                    tilde_path(hooks_path),
                ))?;
            }
        }
    }

    cliclack::outro("All agents configured.")?;

    Ok(())
}

fn setup_encryption() -> Result<()> {
    let db_path = rememora::db::default_db_path();

    // Already encrypted — nothing to do beyond making sure the DB is reachable.
    if db_path.exists() && rememora::crypto::is_db_encrypted(&db_path) {
        cliclack::log::success("Encryption: already enabled")?;
        return Ok(());
    }

    // Key already in env / file / keychain — encryption will apply automatically.
    if rememora::crypto::resolve_key(false)?.is_some() {
        if db_path.exists() {
            // Unencrypted DB exists + key available — offer to encrypt.
            let should_encrypt = cliclack::confirm("Database exists but is not encrypted. Encrypt now?")
                .initial_value(true)
                .interact()?;
            if should_encrypt {
                super::encrypt::run_encrypt(&db_path)?;
            }
        } else {
            cliclack::log::success("Encryption: key found — new database will be encrypted")?;
        }
        // Ensure the DB exists so the first-run gate in main.rs passes.
        ensure_db_initialized(&db_path)?;
        return Ok(());
    }

    // No key anywhere — generate one and persist via the keychain → file
    // fallback chain. We *must not* report success unless persistence succeeds
    // (issue #100): on Linux without libsecret/secret-tool the keychain crate
    // silently fails and previously left the user with an empty `~/.rememora/`
    // and a `setup` claim that was untrue.
    let key = rememora::crypto::generate_key();

    match rememora::crypto::persist_key(&key)? {
        rememora::crypto::KeyStorageOutcome::Keychain => {
            cliclack::log::success("Encryption: key generated and stored in OS keychain")?;
        }
        rememora::crypto::KeyStorageOutcome::File { path, keychain_error } => {
            // Surface the trade-off explicitly — the file fallback is fine for
            // CI / Docker / vanilla Linux but is weaker than a keychain entry.
            cliclack::log::warning(format!(
                "Encryption: keychain unavailable ({keychain_error}).\n\
                 Key written to {} (mode 600).\n\
                 For stronger protection, install libsecret-1-0 (or equivalent)\n\
                 and re-run `rememora setup`.",
                tilde_path(&path),
            ))?;
        }
    }

    // If an unencrypted DB already exists, encrypt it in place.
    if db_path.exists() {
        super::encrypt::run_encrypt(&db_path)?;
    }

    // Always materialize the DB — the first-run gate in main.rs is
    // `db_path.exists()`. Prior to issue #100 this path could leave the
    // directory empty and every subsequent CLI call would bail with
    // "Rememora is not set up yet".
    ensure_db_initialized(&db_path)?;

    Ok(())
}

/// Touch the SQLite DB so the first-run gate (`db_path.exists()`) passes.
/// Opens once with `db::open` to apply the cipher key + run migrations,
/// then drops the connection.
fn ensure_db_initialized(db_path: &std::path::Path) -> Result<()> {
    if db_path.exists() && db_path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(());
    }
    let conn = rememora::db::open(db_path).with_context(|| {
        format!(
            "Failed to initialize database at {}",
            db_path.display(),
        )
    })?;
    drop(conn);
    cliclack::log::success(format!(
        "Database initialized at {}",
        tilde_path(db_path),
    ))?;
    Ok(())
}

enum Action {
    AlreadyConfigured,
    NeedsWork {
        instructions: bool,
        hooks: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `setup --apply` must wire exactly the canonical hook list. Asserting the
    /// keys here keeps `rememora_hooks()` honest: the moment someone forgets
    /// to wire a new hook into setup, this test fails.
    #[test]
    fn rememora_hooks_includes_user_prompt_submit() {
        // Issue #101 regression guard — UserPromptSubmit (FTS5 injection)
        // shipped in PR #87 but was missing from `setup --apply` for several
        // releases. If this assert ever fails, the hook list silently drifted
        // again.
        let events: Vec<&str> = rememora_hooks().iter().map(|h| h.event).collect();
        assert!(
            events.contains(&"UserPromptSubmit"),
            "UserPromptSubmit hook missing from rememora_hooks(); got: {events:?}",
        );
    }

    #[test]
    fn rememora_hooks_match_expected_set() {
        let mut events: Vec<&str> = rememora_hooks().iter().map(|h| h.event).collect();
        events.sort();
        // Compare against an explicit list rather than the plugin manifest
        // file: the manifest uses `${CLAUDE_PLUGIN_ROOT}` paths that only
        // resolve under the plugin install. These are the four standalone
        // hooks setup is responsible for.
        assert_eq!(
            events,
            vec!["SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"],
        );
    }

    /// `write_hooks` must produce a JSON object whose top-level `hooks` keys
    /// match the canonical list — even when the file does not yet exist. This
    /// is the end-to-end contract setup gives to its caller.
    #[test]
    fn write_hooks_creates_complete_hook_set_on_fresh_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_hooks(&path).expect("write_hooks");

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = parsed
            .get("hooks")
            .and_then(|v| v.as_object())
            .expect("hooks object present");

        for spec in rememora_hooks() {
            let arr = hooks
                .get(spec.event)
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("{} array missing", spec.event));
            let has_marker = arr.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .map(|s| s.contains(spec.marker))
                    .unwrap_or(false)
            });
            assert!(has_marker, "{} hook missing rememora command", spec.event);
        }
    }

    /// Re-running `write_hooks` against an already-configured settings.json
    /// must be a no-op: each hook entry should appear exactly once.
    #[test]
    fn write_hooks_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_hooks(&path).unwrap();
        write_hooks(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = parsed.get("hooks").and_then(|v| v.as_object()).unwrap();

        for spec in rememora_hooks() {
            let arr = hooks
                .get(spec.event)
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("{} array missing", spec.event));
            let count = arr
                .iter()
                .filter(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|s| s.contains(spec.marker))
                        .unwrap_or(false)
                })
                .count();
            assert_eq!(count, 1, "{} duplicated by repeat write_hooks", spec.event);
        }
    }
}
