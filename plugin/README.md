# Rememora — Claude Code Plugin

Persistent cross-agent memory for Claude Code. Automatically saves decisions, bug fixes, patterns, and entity knowledge across sessions. Semantically retrieves relevant context when you need it.

## What it does

- **SessionStart hook**: Loads project context from rememora automatically
- **Model-invoked save skill**: Claude autonomously saves knowledge when it makes decisions, fixes bugs, or discovers patterns
- **Model-invoked search skill**: Claude autonomously searches memory before implementing or when encountering unfamiliar code
- **`/rememora` command**: Manually trigger memory save or search

## Install

```bash
# Via Claude Code marketplace (when published)
claude plugin install rememora

# Manual: copy to your plugins directory
cp -r plugin/ ~/.claude/plugins/rememora/
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
4. On **session end**, the hook closes the active rememora session

## Plugin structure

```
plugin/
├── hooks/
│   └── hooks.json              # SessionStart + SessionEnd hooks
├── scripts/
│   ├── session-start.sh        # Load context + start session
│   └── session-end.sh          # End active session
├── skills/
│   ├── rememora-save/
│   │   └── SKILL.md            # Model-invoked: autonomous save
│   ├── rememora-search/
│   │   └── SKILL.md            # Model-invoked: autonomous search
│   └── rememora-init/
│       └── SKILL.md            # User-invoked: /rememora
└── README.md
```
