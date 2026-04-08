# Rememora

Persistent, cross-agent memory for AI coding assistants. One SQLite database, shared by every agent you use.

**The problem:** Claude Code, Codex, and Gemini CLI each lose context between sessions. Switch agents mid-task and you start from scratch. Come back to a project after a week and the agent has forgotten everything.

**Rememora fixes this.** A fast Rust CLI that any agent can call via Bash to save and retrieve memories, transfer working context between agents, and build up project knowledge over time — with autonomous curation that extracts memories from session transcripts without manual intervention.

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
- **Vector search** — optional cosine similarity via sqlite-vec + sentence-transformers (feature-gated)
- **Hybrid search** — reciprocal rank fusion (RRF) merging BM25 + vector results
- **Autonomous curation** — LLM-powered memory extraction from Claude Code session transcripts
- **Memory consolidation** — smart dedup, merge, and pruning of stale memories via LLM
- **Agent orchestration** — dispatch GitHub issues to Claude CLI with quality gates and retry loops
- **Eval benchmark** — multi-scenario harness measuring instruction compliance and autonomous behavior
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

## Autonomous Curation

Rememora can automatically extract memories from Claude Code sessions without manual intervention:

```bash
# Auto-discover and curate all Claude Code session transcripts
rememora curate --auto

# Curate a specific session file
rememora curate --file ~/.claude/projects/.../session.jsonl --project myapp

# Preview what would be extracted (dry-run)
rememora curate --auto --dry-run
```

**How it works:**
1. **JSONL parsing** — reads Claude Code session transcripts incrementally (watermark-based, never re-processes old content)
2. **Signal gate** — fast Haiku classification: does this transcript contain memorable knowledge? (YES/NO)
3. **AUDN curation** — Sonnet subagent with Bash access runs the full Add/Update/Delete/Noop cycle via `rememora save/search/supersede`
4. **Consolidation** — smart dedup via BM25 clustering + LLM-powered merge/prune

Integrates with Claude Code hooks for fully autonomous operation — memories are extracted after every conversation turn.

## Memory Consolidation

Over time, memories accumulate duplicates and stale entries. Rememora consolidates them:

```bash
# Find and merge similar memories using LLM
rememora evolve --project myapp

# Preview clusters without applying changes
rememora evolve --project myapp --dry-run

# Check if consolidation gate is met (24h + 5 new memories)
rememora consolidate --project myapp --check-only
```

The consolidation system uses BM25 cross-search to find similar memory clusters, then an LLM decides whether to merge, supersede, prune, or keep each cluster.

## Agent Orchestration

Dispatch GitHub issues to Claude CLI agents with quality gates:

```bash
# Run a single issue
rememora agent-run --repo owner/repo --issue 42 --retries 3

# Watch project board and auto-dispatch Ready-For-Dev issues
rememora agent-loop --repo owner/repo --poll 300

# One-shot: process current Ready-For-Dev items and exit
rememora agent-loop --repo owner/repo --once
```

**`agent-run` workflow:**
1. Fetch issue from GitHub → move to "In Progress"
2. Create isolated git worktree
3. Run Claude CLI with issue context
4. Quality gate: run tests, retry on failure (configurable retries)
5. Open PR → move to "Ready for Review"

**`agent-loop`** polls the GitHub project board continuously, dispatching Ready-For-Dev issues and merging Cherry-Picked PRs.

## Agent Setup

### Claude Code (Recommended: Skill)

The fastest way to add Rememora to Claude Code is as a skill:

```bash
# Copy the plugin to your Claude Code plugins directory
cp -r plugin/ ~/.claude/plugins/rememora/
```

This gives you **fully autonomous operation** — no manual commands needed:

| Component | What it does |
|-----------|-------------|
| **SessionStart hook** | Loads project context + starts rememora session |
| **SessionEnd hook** | Closes the active session |
| **Stop hook** | Curates memories from the session transcript after each turn |
| **rememora-save skill** | Claude autonomously saves decisions, bug fixes, patterns |
| **rememora-search skill** | Claude autonomously searches before implementations |
| **`/rememora` command** | Manual save, search, or status check |

After copying, restart Claude Code. The plugin auto-detects your project from the working directory.

### Claude Code (Alternative: CLAUDE.md)

If you prefer manual control, add to `~/.claude/CLAUDE.md`:

```markdown
## Rememora Memory System
On session start:
1. `rememora context --auto` — load prior context
2. `rememora session start --agent claude-code --project <name> --intent "..."`

During work, save important discoveries:
- `rememora save "..." --category decision --project <name>`

Before ending: `rememora session end <id> --summary "..." --working-state "..."`
```

### Codex

Add to `~/.codex/config.toml`:

```toml
system_prompt = """
On session start: run `rememora context --auto` and `rememora session start --agent codex ...`
Save important discoveries with `rememora save ...`
Before ending: `rememora session end <id> --summary "..." --working-state "..."`
"""
```

### Gemini CLI

Add to `~/.gemini/GEMINI.md` using the same pattern as the Claude Code CLAUDE.md approach.

### Auto-Setup (All Agents)

```bash
# Detect installed agents and show what would be configured
rememora setup

# Apply the configuration
rememora setup --apply
```

Auto-detects Claude Code, Codex, and Gemini CLI, then patches their config files with rememora instructions.

## Commands

| Command | Description |
|---------|-------------|
| `rememora save "..." --category <cat>` | Save a memory |
| `rememora search "query"` | Search memories (BM25 + optional vector) |
| `rememora context --project <name>` | Load full project context (L0 + L1) |
| `rememora context --auto` | Auto-detect project from cwd |
| `rememora context --cheatsheet` | Compact top-5 summary |
| `rememora get <uri>` | Get specific context by URI |
| `rememora session start` | Start a tracked session |
| `rememora session end <id>` | End session with summary |
| `rememora session end-active` | End active session (hook-friendly) |
| `rememora session resume --project <name>` | Show last session state |
| `rememora session list` | List recent sessions |
| `rememora project add <name>` | Register a project |
| `rememora project list` | List all projects |
| `rememora project show <name>` | Show project details |
| `rememora supersede <old-id> --by <new-id>` | Replace outdated memory |
| `rememora relate <uri-a> <uri-b>` | Link two contexts |
| `rememora extract` | Extract memories from text via LLM |
| `rememora curate --auto` | Curate memories from session transcripts |
| `rememora evolve --project <name>` | LLM-driven memory consolidation |
| `rememora consolidate --project <name>` | Smart dedup with dual gate |
| `rememora agent-run --repo X --issue N` | Dispatch issue to Claude CLI |
| `rememora agent-loop --repo X` | Watch board + auto-dispatch |
| `rememora setup` | Configure agents to use rememora |
| `rememora eval` | DB compliance metrics |
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

### Core Storage

- **Single SQLite database** at `~/.rememora/rememora.db` with WAL mode for concurrent access
- **URI-based hierarchy**: `rememora://projects/{name}/memories/{category}/{slug}`
- **Unified contexts table** — memories, projects, resources all in one table, differentiated by type
- **Tiered loading** — each context has L0 (abstract), L1 (overview), L2 (content) fields
- **Hotness scoring**: `sigmoid(log1p(access_count)) * exp(-age/half_life)` blended 30/70 with importance
- **FTS5 full-text search** with auto-synced triggers on insert/update/delete
- **Soft deletion** via `superseded_by` pointers (audit trail, no data loss)

### Search

- **BM25** — FTS5-based search across name, abstract, overview, content, tags, category
- **Vector search** — optional cosine similarity via sqlite-vec + `all-MiniLM-L6-v2` (384-dim, feature-gated)
- **Hybrid RRF** — reciprocal rank fusion merges BM25 + vector results: `RRF(d) = Σ 1/(k+rank)` with k=60
- **Pluggable embedding backend** — `EmbedBackend` trait with Candle implementation (Metal GPU + CPU fallback)

### Autonomous Curation Pipeline

```
Session JSONL → Watermark (incremental) → Signal Gate (Haiku) → AUDN Curator (Sonnet) → rememora save/search/supersede
```

- **Watermark tracking** — byte offset per session file, never re-processes old content
- **Signal gate** — fast Haiku YES/NO classification (min 500 chars, max 32KB transcript)
- **AUDN cycle** — Sonnet subagent with Bash access runs Add/Update/Delete/Noop
- **Consolidation** — BM25 clustering + LLM merge/supersede/prune with dual gate (24h + 5 new memories)
- **Audit trail** — curator log tracks every action with model, reason, and timestamp

### Three-Layer Integration

```
┌─────────────────────────────────────────────┐
│ Layer 3: Multi-Agent Orchestration          │
│ agent-run, agent-loop, developer/triage     │
│ agents, atomic locking, git worktrees       │
├─────────────────────────────────────────────┤
│ Layer 2: Claude Code Plugin                 │
│ Hooks (SessionStart, SessionEnd, Stop)      │
│ Skills (save, search, init)                 │
├─────────────────────────────────────────────┤
│ Layer 1: CLI Core                           │
│ save, search, context, session, curate,     │
│ evolve, extract, agent-run, eval, export    │
└─────────────────────────────────────────────┘
```

### Database Schema (3 migrations)

| Table | Purpose |
|-------|---------|
| `contexts` | Unified memory storage (18 columns, ULID PKs, URI hierarchy, L0/L1/L2 layers) |
| `contexts_fts` | FTS5 virtual table (auto-synced via triggers) |
| `sessions` | Agent session tracking with parent chains for transfer |
| `relations` | Bidirectional inter-context links (related, depends_on, derived_from, supersedes) |
| `context_embeddings` | Vector storage (f32 BLOB, feature-gated) |
| `vec_contexts` | sqlite-vec KNN index (feature-gated) |
| `watermarks` | Incremental curation byte offsets per session file |
| `curator_log` | Audit trail of curation actions (add/update/delete/noop) |
| `consolidation_runs` | Memory consolidation run history |

## Eval Benchmark

A TypeScript harness for measuring rememora instruction compliance and autonomous agent behavior.

```bash
cd bench

# Quick scenario eval (6 scenarios)
pnpm run eval -- --cli claude-code

# Multi-task sequence with experiment condition
pnpm run eval:long -- --sequence tasks/instruction-mode-eval.json --condition conditions/full-hybrid.json

# Run all conditions in matrix mode
pnpm run eval:matrix -- --sequence tasks/instruction-mode-eval.json

# Compare results across conditions
pnpm run compare:conditions
```

**Quick scenarios** test isolated rememora CLI compliance: session start, save decision, save case, search, transfer handoff, session end.

**Long-run sequences** measure autonomous behavior across multi-task workflows (8 tasks simulating real project development). Five instruction delivery modes are compared:

| Condition | Description |
|-----------|-------------|
| `none` | No rememora instructions (baseline) |
| `reference-card` | Quick command reference |
| `behavioral-triggers` | "When to SEARCH", "When to SAVE" guidance |
| `hooks-only` | Minimal reminders |
| `full-hybrid` | Comprehensive MANDATORY protocol |

Results are exported as **Braintrust-aligned JSONL** (`input/output/expected/scores/metadata`), importable into AI Foundry, Langfuse, LangSmith, and OpenAI Evals with thin adapters.

**Runners:** Claude Code, Codex, Claude Tmux (interactive).

## Development

```bash
cargo test          # 124 tests (121 pass, 3 ignored)
cargo build         # Debug build
cargo clippy        # Lint
```

### Source Structure

| Module | Purpose |
|--------|---------|
| `main.rs` | CLI entry point (clap, 19 commands) |
| `db.rs` | SQLite connection, WAL, 3 migrations |
| `uri.rs` | `rememora://` URI parsing & building |
| `models/context.rs` | Context CRUD + FTS5 |
| `models/session.rs` | Session lifecycle + transfer chains |
| `models/project.rs` | Project metadata + CWD detection |
| `models/relation.rs` | Bidirectional context links |
| `models/watermark.rs` | Curation watermarks + curator log + consolidation runs |
| `hierarchy.rs` | L0/L1 context assembly |
| `hotness.rs` | Scoring: sigmoid(log1p(access)) * exp(-age/7) |
| `search.rs` | BM25 + vector + reciprocal rank fusion |
| `format.rs` | Markdown/JSON output formatting |
| `curator.rs` | Signal gate + AUDN subagent curation |
| `jsonl.rs` | Claude Code session JSONL parser + noise filtering |
| `evolve.rs` | BM25 clustering for memory consolidation |
| `embed/mod.rs` | EmbedBackend trait |
| `embed/candle.rs` | Candle implementation (all-MiniLM-L6-v2, 384-dim) |
| `commands/*` | Individual command implementations |

### Dependencies

**Core:** rusqlite (bundled), clap 4, serde, ulid, chrono, dirs, anyhow, cliclack, ureq

**Embedding (feature-gated):** candle-core/nn/transformers, hf-hub, tokenizers, sqlite-vec

**Feature flags:** `embed-candle` (vector search via Candle), `embed-llamacpp` (stub), `metal` (Apple GPU)

## Roadmap

- [x] Auto-extraction of memories from text via LLM
- [x] Homebrew formula (`brew install Rememora/tap/rememora`)
- [x] Claude Code hooks for automatic session tracking
- [x] Autonomous curation from session transcripts
- [x] Memory consolidation (evolve + consolidate)
- [x] Agent orchestration (agent-run + agent-loop)
- [x] Agent auto-setup (detect + configure)
- [x] Eval benchmark harness (scenarios + long-run + conditions matrix)
- [x] Cheatsheet context mode (compact top-5 summary)
- [ ] Vector search via candle + sqlite-vec (hybrid BM25 + cosine similarity)
- [x] Hierarchical retrieval with score propagation
- [ ] Memory evolution — LLM-based consolidation of old memories
- [x] TUI dashboard for browsing memories

## Insights

Non-obvious gotchas and design decisions discovered while building rememora: **[Engineering Insights](docs/insights.md)**

## License

MIT
