# Instruction Mode Experiment — Run 1 Report

**Date**: 2026-04-02
**Sequence**: `instruction-mode-eval` (8 tasks, Express API buildup)
**Agent**: Claude Code (claude-haiku via `claude -p`)
**Conditions**: 5 (behavioral-triggers, full-hybrid, hooks-only, none, reference-card*)

*reference-card still running at time of report

---

## Executive Summary

**Result: Inconclusive — runner configuration prevented meaningful measurement.**

All 4 completed conditions produced **zero autonomous saves and zero autonomous searches**. This is not because the instructions failed — it's because the eval runner was misconfigured:

1. **Tool restriction too narrow**: `--allowedTools "Bash(rememora:*)"` meant the agent could ONLY call rememora commands. It couldn't write code, read files, or do any actual development work. With no coding context, the agent had no motivation to save decisions or search for patterns.

2. **No real project**: Tasks ran in `/tmp` with no actual codebase. The agent was asked to "set up an Express API" but couldn't create files.

3. **Max turns too low**: `--max-turns 5` limited the agent to at most 5 tool calls, barely enough to write code let alone also use rememora.

4. **Task completion near zero**: Only 1/8 tasks "completed" across all conditions. The agent couldn't do the work, so it couldn't produce the side-effects we wanted to measure.

---

## Raw Results (4/5 conditions)

| Condition | Completed | Auto Saves | Auto Searches | KB End | Latency |
|---|---|---|---|---|---|
| behavioral-triggers | 1/8 | 0 | 0 | 0 | 286s |
| full-hybrid | 0/8 | 0 | 0 | 0 | 1534s |
| hooks-only | 0/8 | 0 | 0 | 0 | 2185s |
| none | 0/8 | 0 | 0 | 0 | 19790s |
| reference-card | pending | — | — | — | — |

---

## Root Cause Analysis

The experiment infrastructure works correctly — JSONL output, behavioral logging, condition matrix, comparison reports all function as designed. The issue is purely in **how the CLI runner invokes Claude Code**:

### Problem 1: Tool Allowlist

```
--allowedTools "Bash(rememora:*)"
```

This restricts the agent to ONLY running `rememora` subcommands. For the experiment to work, the agent needs to do actual coding work (where it encounters decisions, patterns, bugs) AND have rememora available as an additional tool.

**Fix**: Change to `--allowedTools "Bash,Read,Write,Edit,Glob,Grep"` or remove the restriction entirely. The experiment should measure what the agent does naturally, not constrain it.

### Problem 2: Working Directory

Running in `/tmp` with no codebase means:
- No project to register with rememora
- No code context to discover entities/patterns
- No prior decisions to search for

**Fix**: Create or clone a sample project for each eval run, or use a persistent fixture directory.

### Problem 3: Turn Limit

`--max-turns 5` gives the agent very few opportunities to both write code AND use rememora.

**Fix**: Increase to `--max-turns 20` or higher for long-run evals.

---

## Recommendations for Run 2

1. **Broaden tool access**: Allow full tool set so the agent can actually code
2. **Provide a real project**: Clone a starter Express project or create a fixture
3. **Increase turn limit**: At least 20 turns per task
4. **Consider cost**: With broader tools and more turns, each run will cost more. Start with Haiku.
5. **Add raw output capture**: Save full agent output for debugging, not just parsed commands

---

## What Worked

- **Eval infrastructure**: Task sequences, condition matrix, behavioral logger, comparison reports all functioned correctly
- **JSONL output**: Braintrust-compatible format generated for all conditions
- **Condition isolation**: Each condition got its own fresh rememora DB as designed
- **Automated comparison**: `compare:conditions` script produced clear tabular output

---

## Cost

Estimated ~$15-25 across all conditions (exact cost not captured by runner).

---

## Files

- Results: `bench/results/longrun_instruction-mode-eval_*.jsonl`
- Comparison: `bench/results/comparison_instruction-mode-eval_*.json`
- This report: `bench/results/REPORT-instruction-mode-eval-run1.md`
