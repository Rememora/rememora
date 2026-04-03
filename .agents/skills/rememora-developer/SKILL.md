---
name: rememora-developer
description: Use this skill when working on Rememora development tasks such as picking approved tickets, planning implementations, writing code in isolated worktrees, running validation, and preparing pull requests. This is the Codex-facing equivalent of the Claude Code rememora-developer subagent.
---

# Rememora Developer

Use this skill for implementation work on Rememora.

## Scope

- Pick up `Ready-For-Dev` or approved work
- Create or resume issue-specific worktrees under `.agents/worktrees/`
- Plan before implementing non-trivial ticket work
- Validate changes with `cargo test` and `cargo clippy`
- Use Rememora itself for durable memory and session continuity

## Runtime Paths

- Locks: `.agents/locks/`
- Worktrees: `.agents/worktrees/issue-<issue-number>`
- File-based memory leftovers: `.agents/agent-memory/`
- Shared repo instructions: `AGENTS.md`
- Claude-native subagent reference: `.claude/agents/rememora-developer.md`

## Workflow

1. Load context first.
   - Run `rememora context --project rememora`
   - If needed, run targeted recall with `rememora search "<query>" --project rememora`

2. Claim work atomically.
   - Use `.agents/locks/issue-<issue-number>` and `tty-$(basename $(tty))`
   - Do not start work on a ticket that is actively locked by another live terminal

3. Work in an isolated git worktree.
   - Create or resume `.agents/worktrees/issue-<issue-number>`
   - Do not modify the main checkout while doing ticket implementation work

4. Stay in plan mode until approval for non-trivial ticket work.
   - Write the plan in the issue worktree
   - Only implement after approval

5. Validate before completion.
   - Run `cargo test`
   - Run `cargo clippy`

6. Persist durable knowledge.
   - Use `rememora save` for durable decisions, cases, patterns, entities, and preferences
   - Use session `working_state` for ephemeral in-progress state

## Category Guide

- `preference`: user collaboration preferences
- `decision`: architecture or implementation tradeoffs
- `case`: tricky bugs, gotchas, fixes
- `pattern`: reusable conventions or test patterns
- `entity`: services, APIs, dashboards, integrations
- `event`: significant project events

## When To Defer To Claude-Specific Logic

If you need the full Claude-native ticket workflow, detailed lock recovery behavior, or the exact project board protocol, read `.claude/agents/rememora-developer.md`.
