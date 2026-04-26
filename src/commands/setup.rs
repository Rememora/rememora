use anyhow::{Context, Result};
use std::path::PathBuf;

const REMEMORA_MARKER: &str = "## Rememora";

// Markers identifying canonical Rememora hook entries. These are substring
// fingerprints used by `hooks_already_configured` to confirm the deployed
// command is in place. They reference the deployed-script paths under
// `~/.rememora/hooks/` so the marker tracks the exact form `setup --apply`
// writes today (issue #111).
const REMEMORA_HOOK_MARKER: &str = ".rememora/hooks/session-start.sh";
const REMEMORA_CURATE_MARKER: &str = ".rememora/hooks/stop-curate.sh";
const REMEMORA_PROMPT_HOOK_MARKER: &str = ".rememora/hooks/prompt-search.sh";
const REMEMORA_SESSION_END_MARKER: &str = ".rememora/hooks/session-end.sh";

// Bundled plugin hook scripts — embedded at compile time so CLI-only
// installs (Homebrew, cargo) get the same observability-instrumented
// scripts the marketplace plugin ships (issue #111). On every
// `setup --apply` we redeploy these to `~/.rememora/hooks/<name>.sh`
// so they upgrade in lockstep with the CLI binary.
const BUNDLED_SESSION_START_SH: &str = include_str!("../../plugin/scripts/session-start.sh");
const BUNDLED_SESSION_END_SH: &str = include_str!("../../plugin/scripts/session-end.sh");
const BUNDLED_STOP_CURATE_SH: &str = include_str!("../../plugin/scripts/stop-curate.sh");
const BUNDLED_PROMPT_SEARCH_SH: &str = include_str!("../../plugin/scripts/prompt-search.sh");

/// Filename + content pairs for every script we redeploy under `~/.rememora/hooks/`.
const BUNDLED_HOOK_SCRIPTS: &[(&str, &str)] = &[
    ("session-start.sh", BUNDLED_SESSION_START_SH),
    ("session-end.sh", BUNDLED_SESSION_END_SH),
    ("stop-curate.sh", BUNDLED_STOP_CURATE_SH),
    ("prompt-search.sh", BUNDLED_PROMPT_SEARCH_SH),
];

/// Embedded canonical hook manifest — single source of truth for hook *shape*.
///
/// `setup --apply` parses this at runtime to recover the envelope structure
/// (top-level `hooks.<Name>` is an array of `{matcher?, hooks: [{type, command}]}`)
/// expected by Claude Code's settings.json. It deliberately does NOT use the
/// embedded plugin commands — those reference `${CLAUDE_PLUGIN_ROOT}` which
/// only resolves in the marketplace plugin install. We replace each leaf
/// `command` with the standalone CLI form used by Homebrew/cargo installs.
const PLUGIN_HOOKS_MANIFEST: &str = include_str!("../../plugin/hooks/hooks.json");

/// Canonical Rememora hook list written by `setup --apply`.
///
/// This is the single source of truth for which hooks setup wires into each
/// agent's `settings.json`. The *shape* (matcher, envelope) comes from
/// `plugin/hooks/hooks.json` (embedded above); the *commands* are inline
/// standalone forms so CLI-only installs (Homebrew, cargo) get identical
/// behavior without needing the plugin tree on disk.
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
    // The events listed here must each have a matching entry in
    // `plugin/hooks/hooks.json` (validated by `setup_hooks_match_manifest_subset`).
    // We deliberately exclude `Setup`: it's a marketplace-only event whose
    // command depends on `${CLAUDE_PLUGIN_ROOT}` and has no CLI-install analogue.
    //
    // Issue #111: each command points at the bundled plugin script deployed
    // to `~/.rememora/hooks/<name>.sh` by `deploy_hook_scripts()`. This gives
    // CLI installs (Homebrew, cargo) the same `_emit` recursion-gate
    // telemetry (`hook_invocations` table) the marketplace plugin already
    // had, and lets the scripts upgrade in lockstep with the CLI binary
    // (every `setup --apply` redeploys them).
    //
    // Tilde expansion is performed by the shell that runs the hook command;
    // Claude Code launches hooks via `bash -c`, so `~` resolves correctly
    // without us having to bake an absolute path that would differ per host.
    &[
        HookSpec {
            event: "SessionStart",
            marker: REMEMORA_HOOK_MARKER,
            command: "bash ~/.rememora/hooks/session-start.sh 2>/dev/null || true",
        },
        HookSpec {
            event: "UserPromptSubmit",
            marker: REMEMORA_PROMPT_HOOK_MARKER,
            command: "bash ~/.rememora/hooks/prompt-search.sh 2>/dev/null || true",
        },
        HookSpec {
            event: "SessionEnd",
            marker: REMEMORA_SESSION_END_MARKER,
            command: "bash ~/.rememora/hooks/session-end.sh 2>/dev/null || true",
        },
        HookSpec {
            event: "Stop",
            marker: REMEMORA_CURATE_MARKER,
            command: "bash ~/.rememora/hooks/stop-curate.sh 2>/dev/null || true",
        },
    ]
}

/// Substrings whose presence in a hook's `command` field marks the entry as
/// rememora-managed. Used by the migration path in `write_hooks` to identify
/// and replace older flat-shape (or otherwise broken) rememora entries that
/// pre-date this version of setup.
///
/// Keep this in sync with the markers used by `rememora_hooks()` plus any
/// historical command fragments we know we shipped (e.g. `rememora session`).
/// The `.rememora/hooks/` token catches the new deployed-script form added
/// in #111.
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

/// Look up the `matcher` value for an event in the embedded manifest, if any.
/// Returns `None` for events that have no matcher (e.g. SessionEnd, Stop).
/// Panics if the manifest cannot be parsed — that's a build-time invariant.
fn manifest_matcher_for(event: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(PLUGIN_HOOKS_MANIFEST)
        .expect("embedded plugin/hooks/hooks.json must be valid JSON");
    parsed
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|arr| arr.as_array())
        .and_then(|arr| arr.first())
        .and_then(|entry| entry.get("matcher"))
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
}

/// Build the canonical envelope-shape JSON entry for a single hook spec:
/// `{"matcher"?: "...", "hooks": [{"type": "command", "command": "..."}]}`.
fn build_envelope_entry(spec: &HookSpec) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    if let Some(matcher) = manifest_matcher_for(spec.event) {
        obj.insert("matcher".to_string(), serde_json::Value::String(matcher));
    }
    obj.insert(
        "hooks".to_string(),
        serde_json::json!([{
            "type": "command",
            "command": spec.command,
        }]),
    );
    serde_json::Value::Object(obj)
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

/// Directory under which `setup --apply` deploys the bundled plugin hook
/// scripts. Tracks `REMEMORA_DB` (mirrors `crypto::default_key_file_path`)
/// so integration tests pointed at a scratch DB do not stomp the user's
/// real `~/.rememora/hooks/`.
pub fn default_hooks_dir() -> PathBuf {
    if let Ok(p) = std::env::var("REMEMORA_DB") {
        let db = PathBuf::from(p);
        if let Some(parent) = db.parent() {
            return parent.join("hooks");
        }
    }
    home().join(".rememora").join("hooks")
}

/// Write every bundled plugin hook script to `dir/<name>.sh` with mode 0755.
/// Overwrites existing files so the deployed copy stays in lockstep with the
/// CLI binary on every `setup --apply` (issue #111).
fn deploy_hook_scripts(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create hooks dir {}", dir.display()))?;
    for (name, body) in BUNDLED_HOOK_SCRIPTS {
        let target = dir.join(name);
        std::fs::write(&target, body)
            .with_context(|| format!("Failed to write {}", target.display()))?;
        set_executable(&target).with_context(|| {
            format!("Failed to set 0755 perms on {}", target.display())
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &std::path::Path) -> Result<()> {
    // Non-Unix: shells launched from Claude Code on Windows run via WSL/bash
    // anyway; the executable bit is irrelevant to `bash <path>`.
    Ok(())
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
    // Healthy state requires (a) every canonical hook to be present AND
    // (b) every rememora-managed entry to live inside the canonical
    // envelope (`{hooks: [{type, command}]}`). The second clause is the
    // migration trigger for #107: pre-fix installs wrote flat-shape
    // `{type, command}` entries which Claude Code silently rejects.
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    for spec in rememora_hooks() {
        let Some(arr) = hooks.get(spec.event).and_then(|v| v.as_array()) else {
            return false;
        };
        let canonical_match = arr.iter().any(|entry| {
            entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .map(|inner| {
                    inner.iter().any(|leaf| {
                        leaf.get("command")
                            .and_then(|c| c.as_str())
                            .map(|s| s.contains(spec.marker))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        });
        if !canonical_match {
            return false;
        }
    }
    true
}

fn tilde_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(suffix) = path.strip_prefix(&home) {
            return format!("~/{}", suffix.display());
        }
    }
    path.display().to_string()
}

/// Merge rememora hooks into an existing settings.json, preserving all
/// non-rememora user content.
///
/// Writes each hook in Claude Code's canonical envelope shape:
/// ```json
/// "<EventName>": [
///   { "matcher": "...", "hooks": [{ "type": "command", "command": "..." }] }
/// ]
/// ```
///
/// Migration for #107: any entry whose `command` (or any nested
/// `hooks[].command`) looks rememora-managed (per `is_rememora_command`) is
/// removed and replaced with the canonical envelope. Non-rememora entries
/// in the same event array are preserved untouched. This heals settings.json
/// files written by the broken pre-fix `setup --apply` (which produced
/// flat-shape `{type, command}` entries that Claude Code silently rejected).
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

        // Strip every rememora-managed entry (flat-shape OR envelope-shape)
        // so the migration pass cannot leave duplicates. Non-rememora
        // entries — user-managed hooks pointing at unrelated commands —
        // stay where they are.
        arr.retain(|item| !entry_is_rememora_managed(item));

        // Append the canonical envelope.
        arr.push(build_envelope_entry(spec));
    }

    // Write back with pretty formatting
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
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

    // Deploy bundled plugin hook scripts to `~/.rememora/hooks/` BEFORE we
    // mutate any agent's settings.json, so that the moment the new inline
    // commands point at those scripts, the scripts are already in place
    // (issue #111). Idempotent — overwrites the existing files so the
    // scripts upgrade in lockstep with the CLI binary.
    let hooks_dir = default_hooks_dir();
    deploy_hook_scripts(&hooks_dir)?;
    cliclack::log::success(format!(
        "Deployed hook scripts to {}",
        tilde_path(&hooks_dir),
    ))?;

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
        hooks: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: walk a hook event array and return all leaf `command` strings
    /// (i.e. the `hooks[].command` reachable through the envelope).
    fn collect_envelope_commands(arr: &[serde_json::Value]) -> Vec<String> {
        let mut out = Vec::new();
        for entry in arr {
            if let Some(inner) = entry.get("hooks").and_then(|h| h.as_array()) {
                for leaf in inner {
                    if let Some(c) = leaf.get("command").and_then(|c| c.as_str()) {
                        out.push(c.to_string());
                    }
                }
            }
        }
        out
    }

    /// `setup --apply` must wire exactly the canonical hook list.
    #[test]
    fn rememora_hooks_includes_user_prompt_submit() {
        // Issue #101 regression guard — UserPromptSubmit (FTS5 injection)
        // shipped in PR #87 but was missing from `setup --apply` for several
        // releases.
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
        // Setup intentionally excludes the manifest's `Setup` hook
        // (marketplace-only, requires `${CLAUDE_PLUGIN_ROOT}`).
        assert_eq!(
            events,
            vec!["SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"],
        );
    }

    /// Drift guard: every event in `rememora_hooks()` must also appear in the
    /// embedded plugin manifest. The manifest may include additional events
    /// (currently `Setup`) that we deliberately skip.
    #[test]
    fn setup_hooks_match_manifest_subset() {
        let parsed: serde_json::Value =
            serde_json::from_str(PLUGIN_HOOKS_MANIFEST).expect("manifest parses");
        let manifest_events: std::collections::HashSet<String> = parsed
            .get("hooks")
            .and_then(|h| h.as_object())
            .expect("manifest has hooks object")
            .keys()
            .cloned()
            .collect();

        for spec in rememora_hooks() {
            assert!(
                manifest_events.contains(spec.event),
                "rememora_hooks() lists {} but it is absent from plugin/hooks/hooks.json (drift)",
                spec.event,
            );
        }
    }

    /// `write_hooks` must emit canonical envelope shape — Claude Code rejects
    /// flat `{type, command}` entries silently, which is the root cause of #107.
    #[test]
    fn write_hooks_emits_canonical_envelope_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_hooks(&path).expect("write_hooks");

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = parsed
            .get("hooks")
            .and_then(|v| v.as_object())
            .expect("hooks object present");

        // Top-level keys must be exactly the four hook events setup writes.
        let mut keys: Vec<&str> = hooks.keys().map(|s| s.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"],
        );

        for spec in rememora_hooks() {
            let arr = hooks
                .get(spec.event)
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("{} array missing", spec.event));
            assert_eq!(arr.len(), 1, "{} should have a single envelope", spec.event);

            let entry = &arr[0];
            // Every envelope entry must have a `hooks` array of `{type: command, command: ...}`.
            let inner = entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .unwrap_or_else(|| panic!("{} envelope missing inner hooks array", spec.event));
            assert_eq!(inner.len(), 1, "{} should have one inner leaf", spec.event);
            let leaf = &inner[0];
            assert_eq!(
                leaf.get("type").and_then(|t| t.as_str()),
                Some("command"),
                "{} leaf must be type=command",
                spec.event,
            );
            let cmd = leaf
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or_else(|| panic!("{} leaf missing command string", spec.event));
            assert!(
                cmd.contains(spec.marker),
                "{} leaf command does not contain marker {:?}: {}",
                spec.event,
                spec.marker,
                cmd,
            );

            // Flat-shape fields must NOT appear at envelope level.
            assert!(
                entry.get("type").is_none(),
                "{} envelope must not have top-level `type` (flat shape leaked)",
                spec.event,
            );
            assert!(
                entry.get("command").is_none(),
                "{} envelope must not have top-level `command` (flat shape leaked)",
                spec.event,
            );
        }
    }

    /// SessionStart in the plugin manifest carries a `matcher` value Claude
    /// Code uses to scope the hook. Setup must propagate it.
    #[test]
    fn write_hooks_propagates_session_start_matcher() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_hooks(&path).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = &parsed
            .get("hooks")
            .and_then(|h| h.get("SessionStart"))
            .and_then(|v| v.as_array())
            .unwrap()[0];

        assert_eq!(
            entry.get("matcher").and_then(|m| m.as_str()),
            Some("startup|clear|compact|resume"),
        );

        // Other events (SessionEnd, Stop, UserPromptSubmit) have no matcher in
        // the manifest — the field must be absent on those envelopes.
        for ev in ["SessionEnd", "Stop", "UserPromptSubmit"] {
            let e = &parsed
                .get("hooks")
                .and_then(|h| h.get(ev))
                .and_then(|v| v.as_array())
                .unwrap()[0];
            assert!(
                e.get("matcher").is_none(),
                "{ev} envelope should not have a matcher",
            );
        }
    }

    /// Re-running `write_hooks` must be idempotent: each event ends up with
    /// exactly one envelope entry, no duplicate rememora commands.
    #[test]
    fn write_hooks_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_hooks(&path).unwrap();
        write_hooks(&path).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let hooks = parsed.get("hooks").and_then(|v| v.as_object()).unwrap();

        for spec in rememora_hooks() {
            let arr = hooks.get(spec.event).and_then(|v| v.as_array()).unwrap();
            assert_eq!(arr.len(), 1, "{} duplicated after re-run", spec.event);
            let cmds = collect_envelope_commands(arr);
            let count = cmds.iter().filter(|c| c.contains(spec.marker)).count();
            assert_eq!(count, 1, "{} marker duplicated after re-run", spec.event);
        }
    }

    /// Migration: a settings.json written by the broken pre-#107 setup —
    /// flat-shape `{type, command}` entries directly under each event — must
    /// be healed in place on the next `setup --apply`. User-managed
    /// non-rememora entries in the same arrays must be preserved.
    #[test]
    fn write_hooks_migrates_flat_shape_to_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        // Seed the file with the exact flat shape the broken setup produced,
        // plus a couple of non-rememora user-managed entries we must preserve.
        let broken = serde_json::json!({
            "permissions": { "allow": ["Bash(ls:*)"] },
            "hooks": {
                "SessionStart": [
                    { "type": "command", "command": "rememora context --auto 2>/dev/null || true" },
                    { "type": "command", "command": "echo user-hook-please-keep" }
                ],
                "UserPromptSubmit": [
                    { "type": "command", "command": "bash -c 'rememora search --limit 3 --format context blah'" }
                ],
                "SessionEnd": [
                    { "type": "command", "command": "rememora session end-active --auto-summary 2>/dev/null || true" }
                ],
                "Stop": [
                    { "type": "command", "command": "bash -c '(rememora curate --auto 2>/dev/null || true) &'" }
                ]
            }
        });
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&broken).unwrap()).unwrap();

        write_hooks(&path).expect("migration write_hooks");

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
            "non-hooks settings clobbered by migration",
        );

        let hooks = parsed.get("hooks").and_then(|v| v.as_object()).unwrap();

        // Each event has exactly one rememora envelope entry now…
        for spec in rememora_hooks() {
            let arr = hooks.get(spec.event).and_then(|v| v.as_array()).unwrap();
            let envelopes: Vec<_> = arr
                .iter()
                .filter(|e| e.get("hooks").is_some())
                .collect();
            assert_eq!(
                envelopes.len(),
                1,
                "{} should have exactly one envelope entry post-migration",
                spec.event,
            );
            // …and it must be canonical-shape (no top-level type/command).
            let env = envelopes[0];
            assert!(env.get("type").is_none(), "{} migrated entry still flat", spec.event);
            assert!(
                env.get("command").is_none(),
                "{} migrated entry still flat",
                spec.event,
            );
            // The flat-shape rememora entry must be gone (no entry has a
            // top-level `command` containing rememora tokens).
            for entry in arr {
                if let Some(cmd) = entry.get("command").and_then(|c| c.as_str()) {
                    assert!(
                        !is_rememora_command(cmd),
                        "{} still has flat-shape rememora entry: {}",
                        spec.event,
                        cmd,
                    );
                }
            }
        }

        // Non-rememora user hook on SessionStart must be preserved.
        let ss = hooks.get("SessionStart").and_then(|v| v.as_array()).unwrap();
        let preserved = ss.iter().any(|e| {
            e.get("command")
                .and_then(|c| c.as_str())
                .map(|s| s.contains("user-hook-please-keep"))
                .unwrap_or(false)
        });
        assert!(preserved, "user-managed non-rememora hook was clobbered");
    }

    /// Issue #111: bundled plugin scripts must land on disk under the hooks
    /// dir with mode 0755 and contain the recursion-gate telemetry calls
    /// (`rememora debug record-hook-event`) that populate `hook_invocations`.
    /// Without this, Homebrew/cargo installs get zero observability — the
    /// inline `(rememora curate --auto) &` form never emitted hook events.
    #[test]
    fn deploy_hook_scripts_writes_executable_files_with_telemetry() {
        let dir = tempfile::tempdir().unwrap();
        deploy_hook_scripts(dir.path()).expect("deploy_hook_scripts");

        let expected = [
            "session-start.sh",
            "session-end.sh",
            "stop-curate.sh",
            "prompt-search.sh",
        ];
        for name in expected {
            let p = dir.path().join(name);
            assert!(p.exists(), "{} must be deployed", name);

            // Mode 0755 on Unix.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o755, "{} must be 0755, got 0o{:o}", name, mode);
            }

            // Sanity: file is non-empty and has a bash shebang.
            let body = std::fs::read_to_string(&p).unwrap();
            assert!(
                body.starts_with("#!/usr/bin/env bash"),
                "{} missing bash shebang",
                name,
            );
        }

        // The Stop-hook recursion gate is the entire reason #111 exists:
        // verify the deployed copy still contains the `record-hook-event`
        // telemetry call so `hook_invocations` will populate when it runs.
        let stop_curate = std::fs::read_to_string(dir.path().join("stop-curate.sh")).unwrap();
        assert!(
            stop_curate.contains("rememora debug record-hook-event"),
            "stop-curate.sh must call `rememora debug record-hook-event` so \
             hook_invocations populates for Homebrew/cargo installs (#111)",
        );
    }

    /// `deploy_hook_scripts` must be idempotent — running it twice is a
    /// no-error overwrite (the issue spec calls for re-deploy on every
    /// `setup --apply` so scripts upgrade in lockstep with the CLI binary).
    #[test]
    fn deploy_hook_scripts_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        deploy_hook_scripts(dir.path()).expect("first deploy");
        // Tamper with one script to simulate a stale version on disk.
        let stop = dir.path().join("stop-curate.sh");
        std::fs::write(&stop, "#!/usr/bin/env bash\n# stale\n").unwrap();
        // Second deploy must overwrite it.
        deploy_hook_scripts(dir.path()).expect("second deploy");
        let body = std::fs::read_to_string(&stop).unwrap();
        assert!(
            body.contains("rememora debug record-hook-event"),
            "second deploy did not overwrite stale stop-curate.sh",
        );
    }

    /// Issue #111: settings.json's inline commands must reference the
    /// deployed-script paths under `~/.rememora/hooks/`. Without this, the
    /// `_emit` recursion-gate telemetry inside the scripts never runs and
    /// `hook_invocations` stays empty for CLI installs.
    #[test]
    fn write_hooks_references_deployed_script_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_hooks(&path).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let hooks = parsed.get("hooks").and_then(|v| v.as_object()).unwrap();

        let cases = [
            ("SessionStart", "~/.rememora/hooks/session-start.sh"),
            ("SessionEnd", "~/.rememora/hooks/session-end.sh"),
            ("Stop", "~/.rememora/hooks/stop-curate.sh"),
            ("UserPromptSubmit", "~/.rememora/hooks/prompt-search.sh"),
        ];
        for (event, expected_path) in cases {
            let arr = hooks.get(event).and_then(|v| v.as_array()).unwrap();
            let cmds = collect_envelope_commands(arr);
            assert!(
                cmds.iter().any(|c| c.contains(expected_path)),
                "{} command must reference {} (got: {:?})",
                event,
                expected_path,
                cmds,
            );
        }
    }
}
