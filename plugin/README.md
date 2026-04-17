# Rememora — Claude Code Plugin

Persistent cross-agent memory for Claude Code. Automatically saves decisions, bug fixes, patterns, and entity knowledge across sessions. Semantically retrieves relevant context when you need it.

## What it does

- **SessionStart hook**: Loads project context from rememora automatically
- **Stop hook**: Curates memories from the transcript after each agent turn (one in-flight curate per session, via a kernel-level concurrency gate)
- **SessionEnd hook**: Closes the active rememora session and runs a final curation pass
- **Model-invoked save skill**: Claude autonomously saves knowledge when it makes decisions, fixes bugs, or discovers patterns
- **Model-invoked search skill**: Claude autonomously searches memory before implementing or when encountering unfamiliar code
- **`/rememora` command**: Manually trigger memory save or search

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
- A registered project: `rememora project add <name> --path <cwd>`

## How it works

1. On **session start**, the hook runs `rememora context --auto` and injects prior knowledge
2. During work, Claude **autonomously saves** when it detects:
   - Architectural or design decisions
   - Non-trivial bug fixes
   - Codebase patterns or conventions
   - Important entities (services, APIs, configs)
3. Before implementation, Claude **autonomously searches** for relevant prior knowledge
4. After each agent turn, the **Stop hook** forks `rememora curate` against the session transcript to extract anything Claude missed. At most one curate runs in-flight per session (enforced by a `pgrep`-based concurrency gate); a secondary `REMEMORA_CURATE_COOLDOWN_SECS` (default `300`) frequency gate rate-limits consecutive runs
5. On **session end**, the hook closes the active rememora session and runs a final curation pass so the tail of the session is never lost

## Plugin structure

```
plugin/
├── hooks/
│   └── hooks.json              # SessionStart + Stop + SessionEnd hooks
├── scripts/
│   ├── session-start.sh        # Load context + start session
│   ├── stop-curate.sh          # Fork `rememora curate` per agent turn
│   └── session-end.sh          # End active session + final curation pass
├── skills/
│   ├── rememora-save/
│   │   └── SKILL.md            # Model-invoked: autonomous save
│   ├── rememora-search/
│   │   └── SKILL.md            # Model-invoked: autonomous search
│   └── rememora-init/
│       └── SKILL.md            # User-invoked: /rememora
└── README.md
```
