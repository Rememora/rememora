use anyhow::Result;
use std::path::PathBuf;

const REMEMORA_MARKER: &str = "## Rememora Memory System";

const CLAUDE_CODE_SNIPPET: &str = r#"
## Rememora Memory System

Rememora is a cross-agent persistent memory system. Use the `rememora` CLI (Rust binary) for memory across sessions, projects, and agents.

### On session start:
1. `rememora context --auto` (or `--project <name>`) — load prior context
2. If project not registered: `rememora project add <name> --path <cwd> --description "..."`
3. `rememora session start --agent claude-code --project <name> --intent "what you're doing"`

### During work — save knowledge as you discover it:
- Codebase facts: `rememora save "..." --category entity --project <name>`
- Decisions: `rememora save "..." --category decision --project <name> --importance 0.9`
- Problems solved: `rememora save "..." --category case --project <name>`
- Patterns: `rememora save "..." --category pattern --project <name>`
- User preferences: `rememora save "..." --category preference` (global scope)

### When you need to recall something:
`rememora search "query" --project <name>`

### Before ending session:
`rememora session end <id> --summary "what was accomplished" --working-state "current status, blockers, next steps"`

### When handing off to another agent:
`rememora session end <id> --status transferred --summary "..." --working-state "..."`
The next agent picks this up via `rememora context --project <name>`.
"#;

const CODEX_SNIPPET: &str = r#"
## Rememora Memory System

On session start:
1. Run `rememora context --auto` to load prior context for the current project
2. If project not registered: `rememora project add <name> --path <cwd> --description "..."`
3. `rememora session start --agent codex --project <name> --intent "what you're doing"`

During work — save knowledge as you discover it:
- Codebase facts: `rememora save "..." --category entity --project <name>`
- Decisions: `rememora save "..." --category decision --project <name> --importance 0.9`
- Problems solved: `rememora save "..." --category case --project <name>`

When you need to recall something:
`rememora search "query" --project <name>`

Before ending session:
`rememora session end <id> --summary "what was accomplished" --working-state "current status"`

When handing off to another agent:
`rememora session end <id> --status transferred --summary "..." --working-state "..."`
"#;

const GEMINI_SNIPPET: &str = r#"
## Rememora Memory System

On session start:
1. Run `rememora context --auto` to load prior context for the current project
2. If project not registered: `rememora project add <name> --path <cwd> --description "..."`
3. `rememora session start --agent gemini --project <name> --intent "what you're doing"`

During work — save knowledge as you discover it:
- Codebase facts: `rememora save "..." --category entity --project <name>`
- Decisions: `rememora save "..." --category decision --project <name> --importance 0.9`
- Problems solved: `rememora save "..." --category case --project <name>`

When you need to recall something:
`rememora search "query" --project <name>`

Before ending session:
`rememora session end <id> --summary "what was accomplished" --working-state "current status"`

When handing off to another agent:
`rememora session end <id> --status transferred --summary "..." --working-state "..."`
"#;

struct AgentConfig {
    name: &'static str,
    binary: &'static str,
    config_path: PathBuf,
    snippet: &'static str,
    inject_mode: InjectMode,
}

enum InjectMode {
    /// Append to markdown file
    AppendMarkdown,
    /// Inject into TOML system_prompt field
    TomlSystemPrompt,
    /// Append to GEMINI.md file (create if needed)
    GeminiMd,
}

fn home() -> PathBuf {
    dirs::home_dir().expect("Could not determine home directory")
}

fn detect_agents() -> Vec<AgentConfig> {
    let mut agents = Vec::new();

    // Claude Code — binary: claude, config: ~/.claude/CLAUDE.md
    if binary_exists("claude") {
        agents.push(AgentConfig {
            name: "Claude Code",
            binary: "claude",
            config_path: home().join(".claude").join("CLAUDE.md"),
            snippet: CLAUDE_CODE_SNIPPET,
            inject_mode: InjectMode::AppendMarkdown,
        });
    }

    // Codex — binary: codex, config: ~/.codex/instructions.md
    if binary_exists("codex") {
        agents.push(AgentConfig {
            name: "Codex",
            binary: "codex",
            config_path: home().join(".codex").join("instructions.md"),
            snippet: CODEX_SNIPPET,
            inject_mode: InjectMode::TomlSystemPrompt,
        });
    }

    // Gemini CLI — binary: gemini, config: ~/.gemini/GEMINI.md
    if binary_exists("gemini") {
        agents.push(AgentConfig {
            name: "Gemini CLI",
            binary: "gemini",
            config_path: home().join(".gemini").join("GEMINI.md"),
            snippet: GEMINI_SNIPPET,
            inject_mode: InjectMode::GeminiMd,
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

pub fn run(apply: bool) -> Result<()> {
    let agents = detect_agents();

    if agents.is_empty() {
        println!("No supported agents detected.");
        println!("Rememora works with: Claude Code (claude), Codex (codex), Gemini CLI (gemini)");
        return Ok(());
    }

    println!("Detected agents:\n");

    let mut actions: Vec<(&AgentConfig, Action)> = Vec::new();

    for agent in &agents {
        let action = if already_configured(&agent.config_path) {
            Action::AlreadyConfigured
        } else if agent.config_path.exists() {
            Action::WillAppend
        } else {
            Action::WillCreate
        };

        let status = match &action {
            Action::AlreadyConfigured => "already configured",
            Action::WillAppend => "will append rememora instructions",
            Action::WillCreate => "will create config file",
        };

        println!(
            "  {} ({})\n    Config: {}\n    Status: {}\n",
            agent.name,
            agent.binary,
            agent.config_path.display(),
            status,
        );

        actions.push((agent, action));
    }

    let pending: Vec<_> = actions
        .iter()
        .filter(|(_, a)| !matches!(a, Action::AlreadyConfigured))
        .collect();

    if pending.is_empty() {
        println!("All agents already configured. Nothing to do.");
        return Ok(());
    }

    if !apply {
        println!("Run `rememora setup --apply` to apply these changes.");
        println!("Existing files will be backed up with .bak extension.");
        return Ok(());
    }

    // Apply changes
    for (agent, action) in &actions {
        if matches!(action, Action::AlreadyConfigured) {
            continue;
        }

        // Ensure parent directory exists
        if let Some(parent) = agent.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Backup existing file
        if agent.config_path.exists() {
            let backup = agent.config_path.with_extension("md.bak");
            std::fs::copy(&agent.config_path, &backup)?;
            println!("  Backed up {} -> {}", agent.config_path.display(), backup.display());
        }

        match agent.inject_mode {
            InjectMode::AppendMarkdown | InjectMode::GeminiMd => {
                let mut content = if agent.config_path.exists() {
                    std::fs::read_to_string(&agent.config_path)?
                } else {
                    String::new()
                };
                content.push_str(agent.snippet);
                std::fs::write(&agent.config_path, content)?;
            }
            InjectMode::TomlSystemPrompt => {
                // For Codex, we use instructions.md which is simpler than modifying TOML
                let mut content = if agent.config_path.exists() {
                    std::fs::read_to_string(&agent.config_path)?
                } else {
                    String::new()
                };
                content.push_str(agent.snippet);
                std::fs::write(&agent.config_path, content)?;
            }
        }

        println!("  Configured {} at {}", agent.name, agent.config_path.display());
    }

    println!("\nSetup complete. Your agents will now use rememora for persistent memory.");

    Ok(())
}

enum Action {
    AlreadyConfigured,
    WillAppend,
    WillCreate,
}
