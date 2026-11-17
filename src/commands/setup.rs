use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

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
// Instruction snippet: the only capture guidance rememora ships. No hook
// fires on the agent's behalf, so this block is what tells it to call
// `rememora context`/`search`/`save`/`session` itself. One source for every
// agent, parameterised by agent and project name, wrapped in markers so
// re-running setup updates it in place.
// ---------------------------------------------------------------------------

pub const BLOCK_BEGIN: &str = "<!-- rememora:begin (managed by `rememora setup`, edits inside are overwritten) -->";
pub const BLOCK_END: &str = "<!-- rememora:end -->";

const SNIPPET_TEMPLATE: &str = r#"## Rememora: shared memory

`rememora` is a memory database shared by every coding agent on this machine.
Nothing runs automatically: you call it yourself, as you work.

1. Start of work: `rememora context --project {project} --query "<task>"`
   (memory lines end with an id; `rememora get <id>` shows one in full), then
   `rememora session start --agent {agent} --project {project} --intent "<goal>"`.
2. Before non-trivial work, when stuck, or when the user mentions past work:
   `rememora search "<topic>" --project {project}`.
3. Save as it happens, one fact per memory, written so another agent can act
   on it without this conversation:
   `rememora save "<fact>" --category <category> --importance <0-1> --project {project} --agent {agent}`
   - decision: a choice and its reason (0.8)
   - case: a bug's root cause and its fix (0.7)
   - preference: a correction or preference from the user (0.8; omit
     `--project` when it applies everywhere)
   - pattern: a convention or quirk of the code or environment (0.6)
   - entity: where something lives, a service, a key config (0.5)
   Use 0.9 or more only when forgetting it would cause real damage.
4. When a saved fact changes, replace it in one step:
   `rememora supersede <old id> --text "<new fact>"`. Never keep two versions.
5. End of work: `rememora session end-active --agent {agent} --project {project} --summary "<what was done>" --working-state "<blockers, next steps>"`.

Do not save secrets, tokens, passwords or personal data, anything the code,
README or git history already records, or state that only matters today."#;

/// Render the instructions block for `agent` (the `--agent` value, e.g.
/// `codex`) and `project`. `None` leaves a placeholder for the agent to fill.
pub fn render_snippet(agent: Option<&str>, project: Option<&str>) -> String {
    let body = SNIPPET_TEMPLATE
        .replace("{agent}", agent.unwrap_or("<your agent name>"))
        .replace("{project}", project.unwrap_or("<project>"));
    format!("{BLOCK_BEGIN}\n{body}\n{BLOCK_END}\n")
}

/// Insert or replace the managed block in `existing`, preserving everything
/// else. Order of preference: replace between markers; replace an unmarked
/// `## Rememora` section written by older versions (up to the next `## `
/// heading); otherwise append.
pub fn upsert_block(existing: &str, block: &str) -> String {
    if let (Some(start), Some(end_rel)) = (
        existing.find(BLOCK_BEGIN),
        existing.find(BLOCK_BEGIN).and_then(|s| existing[s..].find(BLOCK_END)),
    ) {
        let mut end = start + end_rel + BLOCK_END.len();
        if existing[end..].starts_with('\n') {
            end += 1;
        }
        return format!("{}{}{}", &existing[..start], block, &existing[end..]);
    }
    if let Some(start) = legacy_section_start(existing) {
        let after = start + REMEMORA_MARKER.len();
        let end = existing[after..]
            .find("\n## ")
            .map(|i| after + i + 1)
            .unwrap_or(existing.len());
        let tail = &existing[end..];
        let sep = if tail.is_empty() { "" } else { "\n" };
        return format!("{}{}{}{}", &existing[..start], block, sep, tail);
    }
    let mut out = existing.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(block);
    out
}

fn legacy_section_start(s: &str) -> Option<usize> {
    if s.starts_with(REMEMORA_MARKER) {
        return Some(0);
    }
    s.find(&format!("\n{REMEMORA_MARKER}")).map(|i| i + 1)
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

/// How an instruction file is shaped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// Plain markdown, block upserted between markers.
    Markdown,
    /// Cursor `.mdc` rule: YAML front matter then markdown. Owned by rememora
    /// (a dedicated file), so it is rewritten whole.
    CursorMdc,
}

/// One row of the agent table.
pub struct AgentSpec {
    pub name: &'static str,
    /// `--agent` value written into the snippet.
    pub slug: &'static str,
    /// Any of these on PATH means the agent is installed.
    pub binaries: &'static [&'static str],
    /// Global instruction file, if the agent has one on disk.
    pub global: Option<fn() -> PathBuf>,
    /// Agent-specific project file (relative to the repo root), beyond AGENTS.md.
    pub project_file: Option<(&'static str, Format)>,
    /// Whether the agent reads a project-root AGENTS.md.
    pub reads_agents_md: bool,
    /// Legacy settings.json that may hold rememora hooks from older setups.
    pub hooks: Option<fn() -> PathBuf>,
    /// How to register `rememora mcp` with this agent.
    pub mcp: &'static str,
}

fn env_dir_or(var: &str, fallback: &[&str]) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => fallback.iter().fold(home(), |p, s| p.join(s)),
    }
}

/// The agent table. Sources checked 2026-11-17.
pub fn agent_table() -> Vec<AgentSpec> {
    vec![
        // Claude Code: ~/.claude/CLAUDE.md global, CLAUDE.md in the project.
        // Does not read AGENTS.md. https://docs.anthropic.com/en/docs/claude-code/memory
        // (not re-fetched in this change; unchanged from the previous setup).
        AgentSpec {
            name: "Claude Code",
            slug: "claude-code",
            binaries: &["claude"],
            global: Some(|| home().join(".claude").join("CLAUDE.md")),
            project_file: Some(("CLAUDE.md", Format::Markdown)),
            reads_agents_md: false,
            hooks: Some(|| home().join(".claude").join("settings.json")),
            mcp: "claude mcp add --scope user rememora -- rememora mcp",
        },
        // Codex CLI: $CODEX_HOME/AGENTS.md (default ~/.codex), project AGENTS.md
        // from the git root down. https://learn.chatgpt.com/docs/agent-configuration/agents-md
        // MCP: ~/.codex/config.toml [mcp_servers.<name>] command/args.
        // https://developers.openai.com/codex/mcp
        AgentSpec {
            name: "Codex",
            slug: "codex",
            binaries: &["codex"],
            global: Some(|| env_dir_or("CODEX_HOME", &[".codex"]).join("AGENTS.md")),
            project_file: None,
            reads_agents_md: true,
            hooks: None,
            mcp: "codex mcp add rememora -- rememora mcp\n  or in ~/.codex/config.toml:\n  [mcp_servers.rememora]\n  command = \"rememora\"\n  args = [\"mcp\"]",
        },
        // Gemini CLI: ~/.gemini/GEMINI.md global, GEMINI.md in workspace dirs
        // and parents. https://geminicli.com/docs/cli/gemini-md/
        // Does not read AGENTS.md by default (configurable via context.fileName).
        // MCP: `mcpServers` in ~/.gemini/settings.json (format unverified in this change).
        AgentSpec {
            name: "Gemini CLI",
            slug: "gemini",
            binaries: &["gemini"],
            global: Some(|| home().join(".gemini").join("GEMINI.md")),
            project_file: Some(("GEMINI.md", Format::Markdown)),
            reads_agents_md: false,
            hooks: Some(|| home().join(".gemini").join("settings.json")),
            mcp: "~/.gemini/settings.json: {\"mcpServers\": {\"rememora\": {\"command\": \"rememora\", \"args\": [\"mcp\"]}}}",
        },
        // Cursor: no global rules file on disk (User Rules live in the settings
        // UI), project rules are `.cursor/rules/*.mdc` with front matter
        // (description, globs, alwaysApply), and AGENTS.md is read.
        // https://cursor.com/docs/context/rules
        // MCP: ~/.cursor/mcp.json `mcpServers` (format unverified in this change).
        AgentSpec {
            name: "Cursor",
            slug: "cursor",
            binaries: &["cursor", "cursor-agent"],
            global: None,
            project_file: Some((".cursor/rules/rememora.mdc", Format::CursorMdc)),
            reads_agents_md: true,
            hooks: None,
            mcp: "~/.cursor/mcp.json: {\"mcpServers\": {\"rememora\": {\"command\": \"rememora\", \"args\": [\"mcp\"]}}}",
        },
        // opencode: ~/.config/opencode/AGENTS.md global (falls back to
        // ~/.claude/CLAUDE.md), project AGENTS.md. https://opencode.ai/docs/rules/
        // MCP: `mcp` key in opencode.json with type local (format unverified in this change).
        AgentSpec {
            name: "opencode",
            slug: "opencode",
            binaries: &["opencode"],
            global: Some(|| home().join(".config").join("opencode").join("AGENTS.md")),
            project_file: None,
            reads_agents_md: true,
            hooks: None,
            mcp: "opencode.json: {\"mcp\": {\"rememora\": {\"type\": \"local\", \"command\": [\"rememora\", \"mcp\"]}}}",
        },
        // GitHub Copilot CLI: $COPILOT_HOME/copilot-instructions.md (default
        // ~/.copilot), project .github/copilot-instructions.md and AGENTS.md.
        // https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-custom-instructions
        // MCP: ~/.copilot/mcp-config.json `mcpServers`, or `/mcp add` (unverified in this change).
        AgentSpec {
            name: "Copilot CLI",
            slug: "copilot",
            binaries: &["copilot"],
            global: Some(|| env_dir_or("COPILOT_HOME", &[".copilot"]).join("copilot-instructions.md")),
            project_file: Some((".github/copilot-instructions.md", Format::Markdown)),
            reads_agents_md: true,
            hooks: None,
            mcp: "~/.copilot/mcp-config.json: {\"mcpServers\": {\"rememora\": {\"type\": \"local\", \"command\": \"rememora\", \"args\": [\"mcp\"], \"tools\": [\"*\"]}}}",
        },
    ]
}

fn detected(spec: &AgentSpec) -> bool {
    spec.binaries.iter().any(|b| binary_exists(b))
}

/// File content for a rememora-owned Cursor rule.
fn cursor_mdc(block: &str) -> String {
    format!(
        "---\ndescription: Rememora shared memory, how to load, search and save memories\nalwaysApply: true\n---\n{block}"
    )
}

/// New content for `path` with the block applied, and whether it changed.
fn planned_content(path: &Path, format: Format, block: &str) -> Result<(String, bool)> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let new = match format {
        Format::Markdown => upsert_block(&existing, block),
        Format::CursorMdc => cursor_mdc(block),
    };
    let changed = new != existing;
    Ok((new, changed))
}

fn write_with_backup(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".bak");
        std::fs::copy(path, PathBuf::from(backup))?;
    }
    std::fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))
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

/// One instruction file setup would create or update.
struct Plan {
    label: String,
    path: PathBuf,
    content: String,
    changed: bool,
    hooks_cleanup: Option<PathBuf>,
}

impl Plan {
    fn pending(&self) -> bool {
        self.changed || self.hooks_cleanup.is_some()
    }
}

fn describe(plan: &Plan) -> String {
    let path = tilde_path(&plan.path);
    if !plan.pending() {
        return format!("{:<13} {path}: up to date", plan.label);
    }
    let mut parts = Vec::new();
    if plan.changed {
        parts.push(if plan.path.exists() { "instructions (update block)" } else { "instructions (create)" });
    }
    if plan.hooks_cleanup.is_some() {
        parts.push("hooks (remove stale rememora entries)");
    }
    format!("{:<13} {path}: will write {}", plan.label, parts.join(", "))
}

fn print_mcp(specs: &[&AgentSpec]) -> Result<()> {
    if specs.is_empty() {
        return Ok(());
    }
    let mut msg = String::from(
        "MCP (optional, for agents or clients without shell access). Register `rememora mcp`:",
    );
    for s in specs {
        msg.push_str(&format!("\n{}:\n  {}", s.name, s.mcp));
    }
    msg.push_str("\nClaude Desktop: claude_desktop_config.json, same `mcpServers` shape as Cursor.");
    msg.push_str("\nsetup prints these only; it does not edit MCP config files.");
    cliclack::log::info(msg)?;
    Ok(())
}

fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Decide whether to write: `--apply` always, a TTY asks, anything else is a dry run.
fn confirm_apply(apply: bool, count: usize) -> Result<bool> {
    if apply {
        return Ok(true);
    }
    if !interactive() {
        cliclack::outro("Dry run: nothing written. Re-run with --apply to write these changes.")?;
        return Ok(false);
    }
    let ok = cliclack::confirm(format!("Write {count} file{}?", if count == 1 { "" } else { "s" }))
        .interact()?;
    if !ok {
        cliclack::outro("Setup cancelled.")?;
    }
    Ok(ok)
}

fn write_plans(plans: &[Plan]) -> Result<()> {
    for plan in plans.iter().filter(|p| p.pending()) {
        if plan.changed {
            write_with_backup(&plan.path, &plan.content)?;
            cliclack::log::success(format!("{}: wrote {}", plan.label, tilde_path(&plan.path)))?;
        }
        if let Some(hooks_path) = &plan.hooks_cleanup {
            let backup = hooks_path.with_extension("json.bak");
            std::fs::copy(hooks_path, &backup)?;
            strip_rememora_hooks(hooks_path)?;
            cliclack::log::success(format!(
                "{}: removed automatic rememora hooks ({})",
                plan.label,
                tilde_path(hooks_path),
            ))?;
        }
    }
    Ok(())
}

pub fn run(apply: bool, project_dir: Option<&Path>, agent_files: bool) -> Result<()> {
    cliclack::intro("rememora setup")?;
    let table = agent_table();
    let found: Vec<&AgentSpec> = table.iter().filter(|s| detected(s)).collect();
    match project_dir {
        Some(dir) => run_project(apply, dir, agent_files, &found),
        None => run_global(apply, &found),
    }
}

fn run_global(apply: bool, found: &[&AgentSpec]) -> Result<()> {
    // Encryption writes a key and the DB, so a non-interactive dry run skips it.
    if apply || interactive() {
        setup_encryption()?;
    } else {
        cliclack::log::info("Encryption: skipped in dry run")?;
    }

    if found.is_empty() {
        let names: Vec<_> = agent_table().iter().map(|s| s.name).collect();
        cliclack::log::warning(format!(
            "No supported agents detected. Rememora works with: {}.\n\
             For any other agent, run `rememora setup --project-dir <repo>` to write AGENTS.md.",
            names.join(", "),
        ))?;
        cliclack::outro("Nothing to configure.")?;
        return Ok(());
    }

    let mut plans = Vec::new();
    for spec in found {
        let Some(global) = spec.global else {
            cliclack::log::warning(format!(
                "{:<13} has no global instruction file on disk. Use `rememora setup --project-dir <repo>` \
                 (writes AGENTS.md, which {} reads).",
                spec.name, spec.name,
            ))?;
            continue;
        };
        let path = global();
        let (content, changed) =
            planned_content(&path, Format::Markdown, &render_snippet(Some(spec.slug), None))?;
        let hooks_cleanup = spec.hooks.map(|h| h()).filter(hooks_need_cleanup);
        plans.push(Plan { label: spec.name.to_string(), path, content, changed, hooks_cleanup });
    }

    for plan in &plans {
        if plan.pending() {
            cliclack::log::info(describe(plan))?;
        } else {
            cliclack::log::success(describe(plan))?;
        }
    }
    print_mcp(found)?;

    // One-time cleanup: remove any hook scripts an older `setup --apply`
    // deployed to `~/.rememora/hooks/`. Rememora no longer fires anything
    // automatically, so nothing reads these anymore. Idempotent.
    let hooks_dir = default_hooks_dir();
    if apply && hooks_dir.exists() {
        std::fs::remove_dir_all(&hooks_dir)
            .with_context(|| format!("Failed to remove {}", hooks_dir.display()))?;
        cliclack::log::success(format!(
            "Removed deployed hook scripts at {}",
            tilde_path(&hooks_dir),
        ))?;
    }

    let pending = plans.iter().filter(|p| p.pending()).count();
    if pending == 0 {
        maybe_print_update_hint();
        cliclack::outro("All agents already configured.")?;
        return Ok(());
    }
    if !confirm_apply(apply, pending)? {
        return Ok(());
    }
    write_plans(&plans)?;
    maybe_print_update_hint();
    cliclack::outro("All agents configured.")?;
    Ok(())
}

fn run_project(apply: bool, dir: &Path, agent_files: bool, found: &[&AgentSpec]) -> Result<()> {
    if !dir.is_dir() {
        anyhow::bail!("--project-dir {} is not a directory", dir.display());
    }
    let project = dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let project = project.as_deref();

    let mut plans = Vec::new();
    let path = dir.join("AGENTS.md");
    let (content, changed) = planned_content(&path, Format::Markdown, &render_snippet(None, project))?;
    plans.push(Plan { label: "AGENTS.md".into(), path, content, changed, hooks_cleanup: None });

    let mut offered = Vec::new();
    for spec in found {
        let Some((rel, format)) = spec.project_file else { continue };
        let path = dir.join(rel);
        let (content, changed) = planned_content(&path, format, &render_snippet(Some(spec.slug), project))?;
        if agent_files {
            plans.push(Plan { label: spec.name.to_string(), path, content, changed, hooks_cleanup: None });
        } else if changed {
            offered.push(format!("{} ({})", rel, spec.name));
        }
    }

    for plan in &plans {
        cliclack::log::info(describe(plan))?;
    }
    let readers: Vec<_> = agent_table().into_iter().filter(|s| s.reads_agents_md).map(|s| s.name).collect();
    cliclack::log::info(format!("AGENTS.md is read by {}.", readers.join(", ")))?;
    if !offered.is_empty() {
        cliclack::log::info(format!(
            "Also available, add --agent-files to write: {}",
            offered.join(", "),
        ))?;
    }
    print_mcp(found)?;

    let pending = plans.iter().filter(|p| p.pending()).count();
    if pending == 0 {
        cliclack::outro("Project instructions already up to date.")?;
        return Ok(());
    }
    if !confirm_apply(apply, pending)? {
        return Ok(());
    }
    write_plans(&plans)?;
    cliclack::outro("Project instructions written.")?;
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

    #[test]
    fn snippet_is_short_parameterised_and_has_no_em_dashes() {
        let s = render_snippet(Some("codex"), Some("web"));
        assert!(s.lines().count() <= 35, "snippet is {} lines", s.lines().count());
        assert!(!s.contains('\u{2014}'), "em dash in snippet");
        assert!(s.contains("--agent codex --project web"));
        assert!(s.contains("session end-active --agent codex --project web"));
        assert!(s.contains("supersede <old id> --text \"<new fact>\""));
        assert!(s.contains("context --project web --query \"<task>\""));
        assert!(!s.contains("{agent}") && !s.contains("{project}"));
        assert!(s.starts_with(BLOCK_BEGIN) && s.trim_end().ends_with(BLOCK_END));
        let generic = render_snippet(None, None);
        assert!(generic.contains("--agent <your agent name>"));
    }

    #[test]
    fn upsert_appends_then_replaces_in_place_without_duplicating() {
        let a = render_snippet(Some("codex"), None);
        let b = render_snippet(Some("gemini"), None);
        let original = "# Repo\n\nKeep me.\n";
        let once = upsert_block(original, &a);
        assert!(once.starts_with(original));
        assert_eq!(upsert_block(&once, &a), once, "re-running must be a no-op");

        let with_tail = format!("{once}\n## Later section\nalso keep\n");
        let updated = upsert_block(&with_tail, &b);
        assert_eq!(updated.matches(BLOCK_BEGIN).count(), 1);
        assert!(updated.contains("--agent gemini") && !updated.contains("--agent codex"));
        assert!(updated.contains("Keep me.") && updated.contains("## Later section\nalso keep"));
    }

    #[test]
    fn upsert_replaces_legacy_unmarked_section() {
        let legacy = "intro\n\n## Rememora \u{2014} Cross-Agent Persistent Memory\nold body\n### Sessions\nold\n\n## Mine\nkeep\n";
        let out = upsert_block(legacy, &render_snippet(Some("codex"), None));
        assert!(!out.contains("old body") && !out.contains("Cross-Agent"));
        assert!(out.starts_with("intro\n\n"));
        assert!(out.ends_with("## Mine\nkeep\n"));
        assert_eq!(out.matches("## Rememora").count(), 1);
    }

    #[test]
    fn agent_table_rows_are_complete() {
        for spec in agent_table() {
            assert!(spec.global.is_some() || spec.reads_agents_md, "{} unreachable", spec.name);
            assert!(!spec.mcp.contains('\u{2014}'));
        }
        let mdc = cursor_mdc(&render_snippet(Some("cursor"), None));
        assert!(mdc.starts_with("---\ndescription: ") && mdc.contains("alwaysApply: true\n---\n"));
    }
}
