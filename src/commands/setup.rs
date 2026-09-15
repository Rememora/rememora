use anyhow::{Context, Result};
use std::path::PathBuf;

const REMEMORA_MARKER: &str = "## Rememora";

/// Substrings whose presence in a hook's `command` field marks the entry as
/// rememora-managed. Used by `strip_rememora_hooks` to find and remove any
/// automatic hook wiring a previous version of `setup --apply` installed.
///
/// Rememora no longer fires anything automatically from Claude Code / Gemini
/// CLI lifecycle hooks — memory capture happens via explicit CLI invocation
/// (`rememora save` / `rememora curate` / `rememora dream`) or an agent's own
/// judgment, not an unattended Stop/SessionStart/SessionEnd/UserPromptSubmit
/// trigger. `.rememora/hooks/` catches the deployed-script command form
/// shipped by older versions; the `rememora <verb>` tokens catch the earlier
/// inline-command form that predates it.
const REMEMORA_COMMAND_TOKENS: &[&str] = &[
    "rememora context",
    "rememora search",
    "rememora session",
    "rememora curate",
    ".rememora/hooks/",
];

fn is_rememora_command(cmd: &str) -> bool {
    REMEMORA_COMMAND_TOKENS.iter().any(|t| cmd.contains(t))
}

// ---------------------------------------------------------------------------
// Instruction snippets — behavioral triggers + urgency framing.
// These are the only automatic-capture guidance rememora ships now: no hook
// fires on the agent's behalf, so these snippets are what tells it to call
// `rememora search`/`save`/`session` itself.
// ---------------------------------------------------------------------------

const CLAUDE_CODE_SNIPPET: &str = r#"
## Rememora — Cross-Agent Persistent Memory

You have access to `rememora`, a persistent memory system shared across agents and sessions.
Your context will be lost when this session ends. Anything not saved to rememora is gone forever.
You are not the only agent on this project — save what others will need, search before you assume.
Nothing here runs automatically — no hook captures memory on your behalf. Every search, save, and
session call below is something you invoke yourself, as you work.

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

### Catching up
If session transcripts have piled up without being curated, or memories need dedup/consolidation,
run `rememora dream [--project <name>]` — it curates pending sessions then consolidates in one pass.

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
Nothing here runs automatically — no hook captures memory on your behalf. Every search, save, and
session call below is something you invoke yourself, as you work.

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

### Catching up
If session transcripts have piled up without being curated, or memories need dedup/consolidation,
run `rememora dream [--project <name>]` — it curates pending sessions then consolidates in one pass.

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
Nothing here runs automatically — no hook captures memory on your behalf. Every search, save, and
session call below is something you invoke yourself, as you work.

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

### Catching up
If session transcripts have piled up without being curated, or memories need dedup/consolidation,
run `rememora dream [--project <name>]` — it curates pending sessions then consolidates in one pass.

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
    /// Path to the settings/hooks JSON file, if the agent has one. Used only
    /// to detect and strip stale rememora-managed hook entries left by an
    /// older `setup --apply` — we no longer write anything here.
    hooks_path: Option<PathBuf>,
}

fn home() -> PathBuf {
    dirs::home_dir().expect("Could not determine home directory")
}

/// Directory older versions of `setup --apply` deployed bundled hook scripts
/// to. Tracks `REMEMORA_DB` (mirrors `crypto::default_key_file_path`) so
/// integration tests pointed at a scratch DB do not stomp the user's real
/// `~/.rememora/hooks/`. Used only for one-time cleanup — nothing deploys
/// scripts here anymore.
pub fn default_hooks_dir() -> PathBuf {
    if let Ok(p) = std::env::var("REMEMORA_DB") {
        let db = PathBuf::from(p);
        if let Some(parent) = db.parent() {
            return parent.join("hooks");
        }
    }
    home().join(".rememora").join("hooks")
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
        });
    }

    // Codex — binary: codex, instructions: ~/.codex/AGENTS.md
    if binary_exists("codex") {
        agents.push(AgentConfig {
            name: "Codex",
            config_path: home().join(".codex").join("AGENTS.md"),
            snippet: CODEX_SNIPPET,

            hooks_path: None,
        });
    }

    // Gemini CLI — binary: gemini, instructions: ~/.gemini/GEMINI.md, hooks: ~/.gemini/settings.json
    if binary_exists("gemini") {
        agents.push(AgentConfig {
            name: "Gemini CLI",
            config_path: home().join(".gemini").join("GEMINI.md"),
            snippet: GEMINI_SNIPPET,

            hooks_path: Some(home().join(".gemini").join("settings.json")),
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

/// True if `path` contains any rememora-managed hook entry (legacy flat
/// shape or canonical envelope shape) that `setup --apply` should strip.
/// Missing/unreadable/malformed files mean "nothing to clean up", not an
/// error — `setup` never fails on this check.
fn hooks_need_cleanup(path: &PathBuf) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    hooks.values().any(|entry| {
        entry
            .as_array()
            .map(|arr| arr.iter().any(entry_is_rememora_managed))
            .unwrap_or(false)
    })
}

fn tilde_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(suffix) = path.strip_prefix(&home) {
            return format!("~/{}", suffix.display());
        }
    }
    path.display().to_string()
}

/// Remove every rememora-managed hook entry from an existing settings.json,
/// preserving all non-rememora user content untouched.
///
/// Rememora no longer wires anything into Claude Code / Gemini CLI lifecycle
/// hooks — this heals any settings.json an older `setup --apply` wrote by
/// deleting those entries (both the legacy flat shape and the canonical
/// envelope shape; see `is_rememora_command`). An event array that ends up
/// empty is dropped entirely rather than left as `"Event": []`. A missing or
/// empty file is a no-op, not an error.
fn strip_rememora_hooks(path: &PathBuf) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(());
    }
    let mut root: serde_json::Value = serde_json::from_str(&content)?;

    let Some(hooks_obj) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    hooks_obj.retain(|_event, entry| {
        let Some(arr) = entry.as_array_mut() else {
            // Some other tool wrote a non-array under this key — leave it
            // alone rather than clobbering user state.
            return true;
        };
        arr.retain(|item| !entry_is_rememora_managed(item));
        !arr.is_empty()
    });

    let formatted = serde_json::to_string_pretty(&root)?;
    std::fs::write(path, formatted)?;

    Ok(())
}

/// Detect whether an entry inside `settings.json -> hooks -> <Event>`
/// is rememora-managed. Handles both shapes we may encounter:
///   * legacy/broken flat: `{"type": "command", "command": "rememora ..."}`
///   * canonical envelope: `{"matcher"?: ..., "hooks": [{"type", "command": "rememora ..."}, ...]}`
fn entry_is_rememora_managed(item: &serde_json::Value) -> bool {
    if let Some(cmd) = item.get("command").and_then(|c| c.as_str()) {
        if is_rememora_command(cmd) {
            return true;
        }
    }
    if let Some(inner) = item.get("hooks").and_then(|h| h.as_array()) {
        for leaf in inner {
            if let Some(cmd) = leaf.get("command").and_then(|c| c.as_str()) {
                if is_rememora_command(cmd) {
                    return true;
                }
            }
        }
    }
    false
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
        let needs_cleanup = agent.hooks_path.as_ref().map(hooks_need_cleanup).unwrap_or(false);

        let action = if instructions_done && !needs_cleanup {
            Action::AlreadyConfigured
        } else {
            Action::NeedsWork {
                instructions: !instructions_done,
                hooks_cleanup: needs_cleanup,
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
            Action::NeedsWork { instructions, hooks_cleanup } => {
                let mut parts = Vec::new();
                if *instructions {
                    if agent.config_path.exists() {
                        parts.push("instructions (append)");
                    } else {
                        parts.push("instructions (create)");
                    }
                }
                if *hooks_cleanup {
                    parts.push("hooks (remove stale rememora entries)");
                }
                cliclack::log::info(format!(
                    "{:<13} {} — will configure: {}",
                    agent.name, path, parts.join(", "),
                ))?;
            }
        }
    }

    let pending: Vec<_> = actions
        .iter()
        .filter(|(_, a)| !matches!(a, Action::AlreadyConfigured))
        .collect();

    // One-time cleanup: remove any hook scripts an older `setup --apply`
    // deployed to `~/.rememora/hooks/`. Rememora no longer fires anything
    // automatically, so nothing reads these anymore. Idempotent — a no-op
    // once the directory is gone.
    let hooks_dir = default_hooks_dir();
    if apply && hooks_dir.exists() {
        std::fs::remove_dir_all(&hooks_dir)
            .with_context(|| format!("Failed to remove {}", hooks_dir.display()))?;
        cliclack::log::success(format!(
            "Removed deployed hook scripts at {}",
            tilde_path(&hooks_dir),
        ))?;
    }

    if pending.is_empty() {
        maybe_print_update_hint();
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
        let (needs_instructions, needs_hooks_cleanup) = match action {
            Action::AlreadyConfigured => continue,
            Action::NeedsWork { instructions, hooks_cleanup } => (*instructions, *hooks_cleanup),
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

        // --- Hooks cleanup ---
        if needs_hooks_cleanup {
            if let Some(hooks_path) = &agent.hooks_path {
                // Backup existing hooks file
                if hooks_path.exists() {
                    let backup = hooks_path.with_extension("json.bak");
                    std::fs::copy(hooks_path, &backup)?;
                }

                strip_rememora_hooks(hooks_path)?;
                cliclack::log::success(format!(
                    "{}: removed automatic rememora hooks ({})",
                    agent.name,
                    tilde_path(hooks_path),
                ))?;
            }
        }
    }

    maybe_print_update_hint();
    cliclack::outro("All agents configured.")?;

    Ok(())
}

/// Best-effort check for a newer release on GitHub. Honours
/// `REMEMORA_NO_UPDATE_CHECK=1`, uses the 24h cache, and silently swallows
/// every failure mode (offline, parse error, rate-limited). The point is to
/// never block setup on this — print a hint when we have one, otherwise
/// the user sees nothing different.
fn maybe_print_update_hint() {
    let advice = match rememora::update_check::check(false) {
        Ok(Some(a)) => a,
        Ok(None) => return,
        Err(_) => return,
    };
    let _ = cliclack::log::info(advice.render_hint());
}

fn setup_encryption() -> Result<()> {
    let db_path = rememora::db::default_db_path();

    // Already encrypted — confirm we can still open it before claiming success.
    //
    // Issue #113: previously we returned Ok immediately on `is_db_encrypted`,
    // even if every key source (env / file / keychain) was empty. Setup would
    // print "already configured" and exit green while every subsequent
    // `rememora` call errored on a no-tty key prompt. Now we also probe the
    // key chain via `resolve_key_no_prompt` and, if no key is reachable, fall
    // through to a recovery branch that re-runs the persist path (or surfaces
    // a clear error in non-interactive environments).
    if db_path.exists() && rememora::crypto::is_db_encrypted(&db_path) {
        if rememora::crypto::resolve_key_no_prompt()?.is_some() {
            cliclack::log::success("Encryption: already enabled")?;
            return Ok(());
        }

        cliclack::log::warning(
            "Database is encrypted but no key is reachable in env, file, or keychain.\n\
             Paste your existing encryption key to restore access (or Ctrl-C to abort).",
        )?;
        let key = cliclack::password("Encryption key:")
            .interact()
            .context(
                "Cannot prompt for key in non-interactive setup. \
                 Set REMEMORA_KEY, restore ~/.rememora/key, or run `rememora decrypt` to recover.",
            )?;
        if key.trim().is_empty() {
            anyhow::bail!(
                "Empty key provided; cannot recover encrypted database. \
                 Restore the original key file or use `rememora decrypt` if you have a backup."
            );
        }

        // Verify the key actually opens the DB before persisting it — otherwise
        // we'd happily store a wrong value and break every subsequent run.
        std::env::set_var("REMEMORA_KEY", key.trim());
        rememora::db::open(&db_path).context(
            "Provided key did not open the database. \
             Check for typos or restore from a known-good source.",
        )?;

        match rememora::crypto::persist_key(key.trim())? {
            rememora::crypto::KeyStorageOutcome::Keychain => {
                cliclack::log::success("Encryption key restored to OS keychain")?;
            }
            rememora::crypto::KeyStorageOutcome::File { path, .. } => {
                cliclack::log::success(format!(
                    "Encryption key restored to {} (mode 600)",
                    tilde_path(&path),
                ))?;
            }
        }
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
            // Round-trip readback already verified the value persisted
            // (issue #109). Safe to claim keychain success.
            cliclack::log::success("Encryption key stored in OS keychain")?;
        }
        rememora::crypto::KeyStorageOutcome::File { path, keychain_error: _ } => {
            // Surface the trade-off explicitly — the file fallback is fine for
            // CI / Docker / vanilla Linux but is weaker than a keychain entry.
            // The exact `keychain_error` is intentionally elided from the
            // user-facing message; it stays in code paths/logs for debugging.
            cliclack::log::warning(format!(
                "Encryption: keychain unavailable. Key written to {} (mode 600).\n\
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
        hooks_cleanup: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_need_cleanup_false_for_missing_or_hookless_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.json");
        assert!(!hooks_need_cleanup(&missing));

        let empty = dir.path().join("settings.json");
        std::fs::write(&empty, r#"{"permissions": {"allow": ["Bash(ls:*)"]}}"#).unwrap();
        assert!(!hooks_need_cleanup(&empty));
    }

    #[test]
    fn hooks_need_cleanup_true_for_legacy_and_envelope_shapes() {
        let dir = tempfile::tempdir().unwrap();

        let legacy = dir.path().join("legacy.json");
        std::fs::write(
            &legacy,
            serde_json::to_string(&serde_json::json!({
                "hooks": {
                    "Stop": [
                        { "type": "command", "command": "bash -c '(rememora curate --auto) &'" }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(hooks_need_cleanup(&legacy), "legacy flat-shape entry should be detected");

        let envelope = dir.path().join("envelope.json");
        std::fs::write(
            &envelope,
            serde_json::to_string(&serde_json::json!({
                "hooks": {
                    "Stop": [
                        { "hooks": [{ "type": "command", "command": "bash ~/.rememora/hooks/stop-curate.sh 2>/dev/null || true" }] }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(hooks_need_cleanup(&envelope), "deployed-script envelope entry should be detected");
    }

    /// `strip_rememora_hooks` must remove every rememora-managed entry —
    /// legacy flat shape and canonical envelope shape alike — while leaving
    /// user-managed hooks and unrelated settings.json content untouched.
    #[test]
    fn strip_rememora_hooks_removes_managed_entries_preserves_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let seeded = serde_json::json!({
            "permissions": { "allow": ["Bash(ls:*)"] },
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "rememora context --auto 2>/dev/null || true" },
                    { "type": "command", "command": "echo user-hook-please-keep" }
                ],
                "UserPromptSubmit": [
                    { "hooks": [{ "type": "command", "command": "bash ~/.rememora/hooks/prompt-search.sh 2>/dev/null || true" }] }
                ],
                "SessionEnd": [
                    { "type": "command", "command": "rememora session end-active --auto-summary 2>/dev/null || true" }
                ],
                "Stop": [
                    { "hooks": [{ "type": "command", "command": "bash ~/.rememora/hooks/stop-curate.sh 2>/dev/null || true" }] },
                    { "hooks": [{ "type": "command", "command": "echo also-keep-this-one" }] }
                ]
            }
        });
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&seeded).unwrap()).unwrap();

        strip_rememora_hooks(&path).expect("strip_rememora_hooks");

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        // Non-hooks settings must survive untouched.
        assert_eq!(
            parsed
                .get("permissions")
                .and_then(|p| p.get("allow"))
                .and_then(|a| a.as_array())
                .map(|a| a.len()),
            Some(1),
            "non-hooks settings clobbered by strip",
        );

        let hooks = parsed.get("hooks").and_then(|v| v.as_object()).unwrap();

        // SessionEnd and UserPromptSubmit had nothing but rememora entries —
        // the whole event key should be gone, not left as an empty array.
        assert!(hooks.get("SessionEnd").is_none(), "SessionEnd should be fully removed");
        assert!(hooks.get("UserPromptSubmit").is_none(), "UserPromptSubmit should be fully removed");

        // SessionStart keeps only the user-managed entry.
        let ss = hooks.get("SessionStart").and_then(|v| v.as_array()).unwrap();
        assert_eq!(ss.len(), 1);
        assert_eq!(
            ss[0].get("command").and_then(|c| c.as_str()),
            Some("echo user-hook-please-keep"),
        );

        // Stop keeps only the user-managed entry.
        let stop = hooks.get("Stop").and_then(|v| v.as_array()).unwrap();
        assert_eq!(stop.len(), 1);
        let stop_cmd = stop[0]
            .get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|a| a.first())
            .and_then(|leaf| leaf.get("command"))
            .and_then(|c| c.as_str());
        assert_eq!(stop_cmd, Some("echo also-keep-this-one"));
    }

    #[test]
    fn strip_rememora_hooks_is_noop_on_missing_or_hookless_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.json");
        strip_rememora_hooks(&missing).expect("missing file is a no-op");
        assert!(!missing.exists(), "strip must not create a file that wasn't there");

        let hookless = dir.path().join("settings.json");
        let content = r#"{"permissions": {"allow": ["Bash(ls:*)"]}}"#;
        std::fs::write(&hookless, content).unwrap();
        strip_rememora_hooks(&hookless).expect("hookless file is a no-op");
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hookless).unwrap()).unwrap();
        assert_eq!(
            parsed.get("permissions").and_then(|p| p.get("allow")).and_then(|a| a.as_array()).map(|a| a.len()),
            Some(1),
        );
    }

    #[test]
    fn strip_rememora_hooks_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({
                "hooks": {
                    "Stop": [
                        { "hooks": [{ "type": "command", "command": "bash ~/.rememora/hooks/stop-curate.sh 2>/dev/null || true" }] }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        strip_rememora_hooks(&path).unwrap();
        strip_rememora_hooks(&path).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            parsed.get("hooks").and_then(|h| h.get("Stop")).is_none(),
            "second run should still be a clean no-op",
        );
    }
}
