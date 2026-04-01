use anyhow::Result;
use std::path::PathBuf;

const REMEMORA_MARKER: &str = "## Rememora";
const REMEMORA_HOOK_MARKER: &str = "rememora context --auto";

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
    if let Ok(content) = std::fs::read_to_string(path) {
        content.contains(REMEMORA_HOOK_MARKER)
    } else {
        false
    }
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

    // SessionStart hook
    let session_start = hooks_obj
        .entry("SessionStart")
        .or_insert_with(|| serde_json::json!([]));
    if let Some(arr) = session_start.as_array_mut() {
        let already = arr.iter().any(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|s| s.contains("rememora"))
                .unwrap_or(false)
        });
        if !already {
            arr.push(serde_json::json!({
                "type": "command",
                "command": "rememora context --auto 2>/dev/null || true"
            }));
        }
    }

    // SessionEnd hook
    let session_end = hooks_obj
        .entry("SessionEnd")
        .or_insert_with(|| serde_json::json!([]));
    if let Some(arr) = session_end.as_array_mut() {
        let already = arr.iter().any(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|s| s.contains("rememora"))
                .unwrap_or(false)
        });
        if !already {
            arr.push(serde_json::json!({
                "type": "command",
                "command": "rememora session end-active --auto-summary 2>/dev/null || true"
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

enum Action {
    AlreadyConfigured,
    NeedsWork {
        instructions: bool,
        hooks: bool,
    },
}
