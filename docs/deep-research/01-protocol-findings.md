# Protocol Findings: `claude-cli://` Deep Links & Cross-Agent Mechanisms

*Research date: 2026-04-01 | Issue: [#18](https://github.com/Rememora/rememora/issues/18)*

---

## 1. `claude-cli://` Deep Link Protocol

**Status: Real, shipping, limited scope.**

`claude-cli://` is a registered macOS/Linux custom URL protocol handler in Claude Code. It installs a handler app at `~/Applications/Claude Code URL Handler.app` (macOS) or via `.desktop` files (Linux, respects `XDG_DATA_HOME`).

### URL Format

```
claude-cli://open?cwd=/path/to/project&q=your+prompt+here
```

### Parameters

| Parameter | Description |
|-----------|-------------|
| `cwd` | Working directory for the session |
| `q` | Pre-filled prompt text (up to 5,000 chars as of v2.1.72) |

### Behavior

Opens a new terminal window, `cd`s to `cwd`, launches `claude`, and pre-fills the prompt. Does **NOT** auto-submit — user must press Enter.

### Limitations

- Only `cwd` and `q` are supported. **No way to pass CLI flags** (`--append-system-prompt`, `--model`, etc.) via the URL
- No `--resume` or `--session-id` parameter (open feature request: [anthropics/claude-code#10366](https://github.com/anthropics/claude-code/issues/10366))
- Handler app recreates itself every ~24h (known pain point: [anthropics/claude-code#41015](https://github.com/anthropics/claude-code/issues/41015))
- VS Code has a separate handler: `vscode://anthropic.claude-code/open?prompt=...&session=...`

### Changelog

| Version | Date | Change |
|---------|------|--------|
| v2.1.69 | 2026-03-05 | Added `disableDeepLinkRegistration` setting |
| v2.1.72 | 2026-03-10 | `q` param expanded to 5,000 chars |
| v2.1.84 | 2026-03-26 | Opens in preferred terminal |
| v2.1.89 | 2026-04-01 | Fixed deep links not opening on macOS |

### Sources

- [CLI reference](https://code.claude.com/docs/en/cli-reference)
- [Changelog](https://code.claude.com/docs/en/changelog)
- [anthropics/claude-code#29145](https://github.com/anthropics/claude-code/issues/29145) — Original feature request (closed, implemented)
- [anthropics/claude-code#10366](https://github.com/anthropics/claude-code/issues/10366) — Deep linking to specific chats (open)
- [anthropics/claude-code#41015](https://github.com/anthropics/claude-code/issues/41015) — URL Handler install path issue
- [anthropics/claude-code#26197](https://github.com/anthropics/claude-code/issues/26197) — `&` escaping bug on Windows

---

## 2. Claude Code Hooks & Configuration

Claude Code's **hook system** is the primary integration mechanism, far more powerful than deep links.

### 12 Hook Events

| Event | When | Rememora Use Case |
|-------|------|-------------------|
| **`SessionStart`** | Every session start/resume | **Run `rememora context --auto` -> inject context** |
| **`UserPromptSubmit`** | User submits prompt | Inject per-prompt context |
| `PreToolUse` | Before tool execution | Observe tool calls |
| `PostToolUse` | After tool execution | Observe results |
| `Stop` | Claude finishes responding | Auto-save session state |
| **`SessionEnd`** | Session terminates | **Run `rememora session end`** |
| `Setup` | With `--init`/`--init-only` | One-time project setup |
| `PostToolUseFailure` | After tool fails | Recovery context |
| `PermissionRequest` | Permission dialog | Auto-allow/deny |
| `SubagentStop` | Subagent finishes | Can block stopping |
| `Elicitation` | MCP server requests input | Auto-respond |
| `ElicitationResult` | After user responds | Modify response |

Hooks communicate via: stdin (JSON event data) -> shell command -> stdout/stderr (`additionalContext` or `result` field injected into Claude's context).

### CLI Flags for Context Injection

| Flag | Effect |
|------|--------|
| `--append-system-prompt "text"` | Appends to system prompt |
| `--append-system-prompt-file path` | Appends file to system prompt |
| `--system-prompt "text"` | Replaces entire system prompt |
| `-c` / `--continue` | Resume most recent session |
| `-r` / `--resume <id>` | Resume specific session |

### Configuration Hierarchy

CLI flags > `~/.claude/config.json` > `.claude/settings.json` > `.claude/settings.local.json` > `~/.claude/settings.json` > `CLAUDE.md` files.

### Sources

- [Hooks guide](https://code.claude.com/docs/en/hooks-guide)
- [Hooks reference](https://code.claude.com/docs/en/hooks)
- [Session lifecycle hooks tutorial](https://claudefa.st/blog/tools/hooks/session-lifecycle-hooks)

---

## 3. Cross-Agent Protocol Survey

| Capability | Claude Code | Codex CLI | Gemini CLI |
|---|---|---|---|
| **Custom URI scheme** | `claude-cli://` (cwd + q only) | None | None |
| **Instruction file** | `CLAUDE.md` | `AGENTS.md` (+ `.override.md`) | `GEMINI.md` (customizable name) |
| **Global instructions** | `~/.claude/CLAUDE.md` | `~/.codex/AGENTS.md` | `~/.gemini/GEMINI.md` |
| **Config file** | `.claude/settings.json` (JSON) | `.codex/config.toml` (TOML) | `.gemini/settings.json` (JSON) |
| **System prompt override** | `--append-system-prompt` flag | `model_instructions_file` config | `GEMINI_SYSTEM_MD` env var |
| **Startup hooks** | `SessionStart` (production) | `hooks.json` (feature-gated) | `SessionStart` (production) |
| **Hook env vars** | Via `CLAUDE_ENV_FILE` | Not documented | `GEMINI_PROJECT_DIR`, `GEMINI_SESSION_ID`, `GEMINI_CWD` |
| **.env loading** | No | No | Yes (CWD upward + `~/.env`) |
| **MCP servers** | `.claude/settings.json` | `[mcp_servers]` in config.toml | `mcpServers` in settings.json |
| **Non-interactive mode** | `claude -p "prompt"` | `codex exec` | `gemini -p "prompt"` |
| **Config home env var** | -- | `CODEX_HOME` | `GEMINI_CLI_HOME` |

### Codex-Specific Notes

- Hooks are behind feature flag (`codex_hooks = true` in config.toml)
- `developer_instructions` config field allows inline instruction injection
- Plugins system bundles skills + apps + MCP servers
- Two-phase memory pipeline: post-session extraction -> global consolidation into `memory_summary.md`
- Source: https://github.com/openai/codex

### Gemini-Specific Notes

- Most extensive hooks system with 11 lifecycle events
- `GEMINI_SYSTEM_MD` env var can point to a Rememora-generated system prompt file
- Built-in `save_memory` tool writes to GEMINI.md (potential integration point)
- Import syntax: `@file.md` for modular includes in GEMINI.md
- Source: https://github.com/google-gemini/gemini-cli

---

## 4. Integration Options: Today vs. Needs Upstream

### Viable TODAY

**A. SessionStart hooks for automatic context loading (Claude Code + Gemini CLI)**

```json
{
  "hooks": {
    "SessionStart": [{
      "type": "command",
      "command": "rememora context --auto --format json 2>/dev/null || true"
    }]
  }
}
```

**B. SessionEnd hooks for automatic session closure**

**C. Deep link as "memory-aware launcher"** — Limited by 5,000 char `q` param and no auto-submit.

**D. Fix `rememora setup` for Codex** — Currently writes to `~/.codex/instructions.md` but Codex reads `~/.codex/AGENTS.md`.

**E. Gemini CLI `.env` injection** — Write env vars to `.gemini/.env`.

### Needs Upstream Changes

**F. `claude-cli://` with CLI flag passthrough** — Would need a feature request.

**G. Codex CLI hooks (production)** — Behind feature flag, waiting for stabilization.

**H. MCP server mode for Rememora** — High value but significant implementation effort.
