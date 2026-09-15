# Rememora — Claude Code Plugin

Persistent cross-agent memory for Claude Code. Saves decisions, bug fixes, patterns, and entity
knowledge across sessions. Semantically retrieves relevant context when you need it.

## What it does

Rememora fires nothing automatically — no hook captures memory on your behalf. Everything below is
either the model deciding to act, or you running a command yourself:

- **Model-invoked save skill**: Claude autonomously saves knowledge when it makes decisions, fixes bugs, or discovers patterns
- **Model-invoked search skill**: Claude autonomously searches memory before implementing or when encountering unfamiliar code
- **`/rememora` command**: Manually trigger memory save or search
- **`rememora dream`**: Run memory upkeep end-to-end on demand — curates any session transcripts that piled up, then evolves (dedup/prune/merge, applying the decisions). Run it by hand, or wire it to your own cron/launchd job; the plugin installs no schedule.

## Install

```bash
# 1. Add the Rememora marketplace
claude plugin marketplace add Rememora/rememora

# 2. Install the plugin
claude plugin install rememora@rememora

# For project-wide install (shared via git):
claude plugin install rememora@rememora --scope project
```

## Requirements

- `rememora` CLI installed and on PATH (`cargo install rememora` or via Homebrew)
- A registered project pointed at the **main checkout**, not a worktree: `rememora project add <name> --path /path/to/main/checkout`. Work done in a linked git worktree resolves back to that project automatically — see [Project resolution](#project-resolution).

## How it works

1. `claude plugin install` (or `rememora setup --apply`) injects a `## Rememora` instructions block into the agent's instructions file (`CLAUDE.md`/`AGENTS.md`/`GEMINI.md`), telling it when to search, save, and start/end sessions.
2. During work, Claude **autonomously saves** when it detects:
   - Architectural or design decisions
   - Non-trivial bug fixes
   - Codebase patterns or conventions
   - Important entities (services, APIs, configs)
3. Before implementation, Claude **autonomously searches** for relevant prior knowledge.
4. Whenever you want rememora to catch up on anything it missed, run `rememora dream` — it curates pending session transcripts and evolves the resulting memories (applying the decisions) in one pass.

## Project resolution

Every write path — the save/search skills, `/rememora`, `rememora dream` — goes through the CLI's own `project::resolve_write_target` ladder, which folds a worktree directory name, an encoded transcript path, or a case variant (`Ana` vs `ana`) onto the registered project's canonical name. Working from a linked git worktree resolves back to its main checkout automatically; see [Project Resolution](../README.md#project-resolution) in the main README for the full ladder.

This matters because the project filter is a hard `uri LIKE 'rememora://projects/<name>/%'` prefix match — a name matching no registered project does not rank memories lower, it excludes all of them, and the command still exits 0 with the global memories. So **register the main checkout, not a worktree.**

If memories were already saved under a fabricated name, `rememora project reconcile` reports where each one belongs (dry run by default) and `rememora project reconcile --apply` re-homes them.

## Plugin structure

```
plugin/
├── hooks/
│   └── hooks.json              # Setup hook only — one-time "is the CLI on PATH" check
├── scripts/
│   └── setup-check.sh          # Setup hook body
├── skills/
│   ├── rememora-save/
│   │   └── SKILL.md            # Model-invoked: autonomous save
│   ├── rememora-search/
│   │   └── SKILL.md            # Model-invoked: autonomous search
│   └── rememora-init/
│       └── SKILL.md            # User-invoked: /rememora
└── README.md
```
