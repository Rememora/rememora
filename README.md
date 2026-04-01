# Rememora

Persistent, cross-agent memory for AI coding assistants. One SQLite database, shared by every agent you use.

**The problem:** Claude Code, Codex, and Gemini CLI each lose context between sessions. Switch agents mid-task and you start from scratch. Come back to a project after a week and the agent has forgotten everything.

**Rememora fixes this.** A fast Rust CLI that any agent can call via Bash to save and retrieve memories, transfer working context between agents, and build up project knowledge over time.

```bash
# Agent A (Claude Code) saves a decision
rememora save "Chose Zustand over Redux for state management" \
  --category decision --project myapp --importance 0.9

# Agent B (Codex) picks up full context
rememora context --project myapp
# → Returns: project memories + last session state + working context
```

## Features

- **Cross-agent memory** — Claude Code, Codex, Gemini CLI, or any agent with Bash access
- **Session transfer** — hand off working state between agents with full continuity
- **6 memory categories** — preferences, entities, decisions, events, cases, patterns
- **Tiered loading** — L0 abstracts (~100 tok) → L1 overviews (~500 tok) → L2 full content
- **Hotness scoring** — frequently accessed + important memories surface first
- **Full-text search** — BM25 via SQLite FTS5, zero external dependencies
- **Auto-extract** — LLM-powered memory extraction from session transcripts
- **Fast** — ~3ms startup, 3.6MB binary, single SQLite database with WAL
- **Local-first** — everything stays on your machine

## Install

```bash
# Homebrew (macOS & Linux)
brew install Rememora/tap/rememora

# From source
cargo install --path .

# Or download from GitHub Releases
# https://github.com/Rememora/rememora/releases
```

## Quick Start

```bash
# Register a project
rememora project add myapp --path /Users/me/myapp --description "Mobile app" --stack react-native,typescript

# Start a tracked session
rememora session start --agent claude-code --project myapp --intent "implementing auth flow"
# → prints session ID

# Save memories as you work
rememora save "Uses expo-secure-store for token storage" --category decision --project myapp --importance 0.8
rememora save "Stripe API requires idempotency keys for charges" --category entity --project myapp
rememora save "iOS build fails with Hermes + RN 0.76 — disable new arch" --category case --project myapp

# Search memories
rememora search "authentication" --project myapp

# End session with summary
rememora session end <session-id> \
  --summary "Auth flow complete. Login, signup, token refresh all working." \
  --working-state "Need to add biometric auth. Files: src/auth/"
```

## Cross-Agent Transfer

The core use case — seamless handoff between agents:

```bash
# 1. Claude Code finishes work, hands off
rememora session end <id> --status transferred \
  --summary "Auth flow 80% done" \
  --working-state "Login UI done. Token refresh blocked on secure storage decision."

# 2. Switch to Codex — it loads full context
rememora context --project myapp
# Returns markdown with:
#   - All project memories (decisions, entities, cases, patterns)
#   - Last session summary + working state
#   - Transfer status

# 3. Codex continues where Claude Code left off
rememora session start --agent codex --project myapp \
  --intent "resolve secure storage and finish token refresh" \
  --parent <previous-session-id>
```

## Agent Setup

Add to your agent's system prompt or instructions file:

**Claude Code** (`~/.claude/CLAUDE.md`):
```markdown
## Rememora Memory System
On session start:
1. `rememora context --auto` — load prior context
2. `rememora session start --agent claude-code --project <name> --intent "..."`

During work, save important discoveries:
- `rememora save "..." --category decision --project <name>`

Before ending: `rememora session end <id> --summary "..." --working-state "..."`
```

**Codex** (`~/.codex/config.toml`):
```toml
system_prompt = """
On session start: run `rememora context --auto` and `rememora session start --agent codex ...`
Save important discoveries with `rememora save ...`
Before ending: `rememora session end <id> --summary "..." --working-state "..."`
"""
```

## Commands

| Command | Description |
|---------|-------------|
| `rememora save "..." --category <cat>` | Save a memory |
| `rememora search "query"` | Search memories (BM25) |
| `rememora context --project <name>` | Load full project context |
| `rememora context --auto` | Auto-detect project from cwd |
| `rememora get <uri>` | Get specific context by URI |
| `rememora session start` | Start a tracked session |
| `rememora session end <id>` | End session with summary |
| `rememora session resume --project <name>` | Show last session state |
| `rememora session list` | List recent sessions |
| `rememora project add <name>` | Register a project |
| `rememora project list` | List all projects |
| `rememora project show <name>` | Show project details |
| `rememora supersede <old-id> --by <new-id>` | Replace outdated memory |
| `rememora relate <uri-a> <uri-b>` | Link two contexts |
| `rememora extract` | Extract memories from text via LLM |
| `rememora status` | Show DB stats |
| `rememora export --project <name>` | Export as JSON or markdown |

All commands support `--json` for structured output.

## Auto-Extract Memories

Extract memories from session transcripts, notes, or any text using an LLM:

```bash
# Pipe text and preview what would be extracted
cat session_log.txt | rememora extract --project myapp

# Extract and save directly
cat session_log.txt | rememora extract --project myapp --save --agent claude-code

# From a file
rememora extract --file notes.md --project myapp --save

# JSON output for programmatic use
rememora extract --file notes.md --project myapp --json
```

Requires `ANTHROPIC_API_KEY` environment variable. Uses Claude Haiku for fast, cheap extraction.

## Memory Categories

| Category | Use for | Example |
|----------|---------|---------|
| `preference` | User/project preferences | "prefers Zustand over Redux" |
| `entity` | Key concepts, APIs, tools | "Stripe API uses idempotency keys" |
| `decision` | Architecture & design choices | "chose expo-router over React Navigation" |
| `event` | Milestones, releases, incidents | "v2.0 shipped 2026-03-01" |
| `case` | Specific problem + solution | "iOS build fails with Hermes + RN 0.76" |
| `pattern` | Reusable processes | "always run migrations before seeding" |

## Architecture

- **Single SQLite database** at `~/.rememora/rememora.db` with WAL mode for concurrent access
- **URI-based hierarchy**: `rememora://projects/{name}/memories/{category}/{slug}`
- **Unified contexts table** — memories, projects, resources all in one table, differentiated by type
- **Tiered loading** — each context has L0 (abstract), L1 (overview), L2 (content) fields
- **Hotness scoring**: `sigmoid(log1p(access_count)) * exp(-age/half_life)` blended with importance
- **Pluggable embedding backend** — `EmbedBackend` trait for future vector search (candle, llama.cpp)

## Development

```bash
cargo test          # 62 tests
cargo build         # Debug build
cargo clippy        # Lint
```

## Roadmap

- [x] Auto-extraction of memories from text via LLM
- [x] Homebrew formula (`brew install Rememora/tap/rememora`)
- [x] Claude Code hooks for automatic session tracking
- [ ] Vector search via candle + sqlite-vec (hybrid BM25 + cosine similarity)
- [ ] Hierarchical retrieval with score propagation
- [ ] Memory evolution — LLM-based consolidation of old memories
- [ ] TUI dashboard for browsing memories

## Insights

Non-obvious gotchas and design decisions discovered while building rememora: **[Engineering Insights](docs/insights.md)**

## License

MIT
