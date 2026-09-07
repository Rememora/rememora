# Rememora — Claude Code Plugin

Persistent cross-agent memory for Claude Code. Automatically saves decisions, bug fixes, patterns, and entity knowledge across sessions. Semantically retrieves relevant context when you need it.

## What it does

- **SessionStart hook**: Loads project context from rememora automatically
- **UserPromptSubmit hook**: Injects the top FTS5 hits for your prompt, scoped via `rememora search --cwd` so a git worktree resolves to its main checkout
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
- A registered project pointed at the **main checkout**, not a worktree: `rememora project add <name> --path /path/to/main/checkout`. Work done in a linked git worktree resolves back to that project automatically — see [Project resolution](#project-resolution).

## Escape hatches

If a Rememora hook misbehaves and you need to turn everything off quickly, set the kill-switch in the shell that launches Claude Code:

```bash
export REMEMORA_DISABLE_HOOKS=1
```

All four hook scripts (`session-start.sh`, `session-end.sh`, `stop-curate.sh`, `prompt-search.sh`) check this env var at the top and early-exit 0 when set. `unset REMEMORA_DISABLE_HOOKS` to re-enable.

Other tunables:

- `REMEMORA_CURATE_COOLDOWN_SECS` (default `300`): minimum seconds between curate runs per session. Set to `0` to disable the frequency gate. The kernel-level `pgrep` concurrency gate is independent and always active.
- `REMEMORA_CURATE_CHILD`: set internally by `rememora curate` on its `claude -p` subprocesses so the entire hook chain is a no-op inside curator children (no spurious session rows, no context injection, no recursive curate). Not intended for user override.

## How it works

1. On **session start**, the hook runs `rememora context --auto` and injects prior knowledge
2. During work, Claude **autonomously saves** when it detects:
   - Architectural or design decisions
   - Non-trivial bug fixes
   - Codebase patterns or conventions
   - Important entities (services, APIs, configs)
3. Before implementation, Claude **autonomously searches** for relevant prior knowledge
4. After each agent turn, the **Stop hook** forks `rememora curate` against the session transcript to extract anything Claude missed. The curate process is fully detached — launched in its own session via `setsid` (or `nohup` + `disown` on stock macOS) with stdin/stdout/stderr redirected to `/dev/null`, so it cannot hold the hook's pipe and block Claude Code waiting for EOF. At most one curate runs in-flight per session (enforced by a `pgrep`-based concurrency gate); a secondary `REMEMORA_CURATE_COOLDOWN_SECS` (default `300`) frequency gate rate-limits consecutive runs
5. On **session end**, the hook closes the active rememora session and runs a final curation pass so the tail of the session is never lost

## Project resolution

The hooks do not decide which project you are in. They pass along the raw facts they have and let the CLI resolve them:

- `prompt-search.sh` passes the session cwd verbatim: `rememora search --cwd "$CWD"`
- `session-start.sh` passes `basename "$PWD"`, and `session-end.sh` / `stop-curate.sh` pass `basename "$CWD"` from the hook payload, as `--project`
- `stop-curate.sh` additionally derives Claude Code's encoded transcript directory (`-Users-me-Projects-myapp`) to locate the session JSONL

Every one of those goes through `project::resolve_write_target` inside the CLI, which folds a worktree directory name, an encoded path, or a case variant (`Ana` vs `ana`) onto the registered project's canonical name. That is why a hook can safely send a bare basename: inside a linked worktree it resolves to the main checkout's project, and outside one it was already correct.

This matters because the project filter is a hard `uri LIKE 'rememora://projects/<name>/%'` prefix match — a name matching no registered project does not rank memories lower, it excludes all of them, and the command still exits 0 with the global memories. So **register the main checkout, not a worktree.**

If memories were already saved under a fabricated name, `rememora project reconcile` reports where each one belongs (dry run by default) and `rememora project reconcile --apply` re-homes them.

## Plugin structure

```
plugin/
├── hooks/
│   └── hooks.json              # Setup + SessionStart + UserPromptSubmit + Stop + SessionEnd hooks
├── scripts/
│   ├── setup-check.sh          # First-run check that the CLI is installed
│   ├── session-start.sh        # Load context + start session
│   ├── prompt-search.sh        # Inject top FTS5 hits (`rememora search --cwd`)
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
