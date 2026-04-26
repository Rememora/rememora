---
name: rememora
description: Manage rememora persistent memory — save knowledge, search prior context, or check status.
argument-hint: "[save|search|status] [text]"
allowed-tools: Bash
---

# Rememora Memory Management

The user invoked `/rememora`. Parse the argument and act accordingly.

## Commands

### `/rememora save <text>`
Save the specified text as a memory. Determine the best category automatically:
- If it describes a choice/decision → `--category decision`
- If it describes a bug fix → `--category case`
- If it describes a pattern → `--category pattern`
- If it describes a service/API/config → `--category entity`
- If it describes a preference → `--category preference`

```bash
rememora save "<text>" --category <auto-detected> --project <project-name>
```

### `/rememora search <query>`
Search for prior knowledge matching the query.

```bash
rememora search "<query>" --project <project-name>
```

### `/rememora status`
Show current rememora status — project, session, memory count.

```bash
rememora status --json
```

### `/rememora` (no args)
Show project context — memory map, last session, key context.

```bash
rememora context --auto
```

Report the results concisely.
