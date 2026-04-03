---
name: rememora-triage
description: Use this skill when triaging the Rememora project board, assessing issue readiness, checking dependencies, categorizing work, and proposing or applying board status updates. This is the Codex-facing equivalent of the Claude Code rememora-triage subagent.
---

# Rememora Triage

Use this skill for project board triage and backlog grooming on Rememora.

## Scope

- Review the GitHub project board
- Assess implementation evidence in the codebase
- Check blockers and dependencies
- Decide whether items belong in `Todo`, `Ready-For-Dev`, or need human review
- Persist triage continuity through Rememora context and sessions

## Runtime Paths

- Shared repo instructions: `AGENTS.md`
- Shared runtime artifacts: `.agents/`
- Claude-native subagent reference: `.claude/agents/rememora-triage.md`

## Workflow

1. Resume from Rememora first.
   - Run `rememora context --project rememora`
   - Inspect the latest session summary and `working_state`
   - If needed, run `rememora search "triage blockers ready-for-dev project board" --project rememora`

2. Start a triage session.
   - Run `rememora session start --agent rememora-triage --project rememora --intent "Triage Rememora project board"`

3. Inspect the project board.
   - Use `gh project item-list 3 --owner Rememora --format json --limit 500`
   - Capture issue number, status, labels, dependencies, and acceptance criteria

4. Scout the codebase before moving items.
   - Look for implementation evidence
   - Classify items as `NOT_STARTED`, `PARTIALLY_IMPLEMENTED`, `FULLY_IMPLEMENTED`, or `UNKNOWN`
   - Respect existing `In Progress` work

5. Be conservative about mutations.
   - Present intended board changes before applying them unless explicitly authorized
   - Flag ambiguous items for human review rather than guessing

6. End the session with resumable state.
   - Use `rememora session end-active --project rememora --summary "..." --working-state "..."`
   - Save only durable insights with `rememora save`

## Durable Memory Examples

- project board field IDs
- recurring blocker patterns
- stable priority overrides
- board mutation gotchas
- durable product decisions affecting triage

## When To Defer To Claude-Specific Logic

If you need the full Claude-native triage prompt, detailed board heuristics, or the exact reporting structure, read `.claude/agents/rememora-triage.md`.
