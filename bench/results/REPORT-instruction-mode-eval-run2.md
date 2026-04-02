# Instruction Mode Experiment — Run 2 Report

**Date**: 2026-04-02
**Sequence**: `instruction-mode-eval` (8 tasks, Express API buildup)
**Agent**: Claude Code via `claude -p` (single-turn mode)
**Conditions completed**: 4/5 (behavioral-triggers, full-hybrid, none, reference-card)
**Condition aborted**: hooks-only (killed to save cost)

---

## Executive Summary

**Finding: Instruction-based autonomy does not work in single-turn `claude -p` mode.**

After fixing the runner (broader tools, project fixture, 25 max turns), the agent successfully completed coding tasks (7/8 for full-hybrid) but produced **zero rememora calls across all conditions**. The model uses all 25 turns for coding work (Write, Edit, Read, Bash) and never invokes rememora, regardless of instruction style.

This is not a bug — it's a structural limitation of single-turn mode. The model is focused on completing the immediate prompt and has no concept of session persistence. Instructions in `--append-system-prompt` are deprioritized relative to the coding task.

**Implication for Rememora**: Hooks (Layer 1) are the only reliable mechanism for programmatic/automated usage. Behavioral trigger instructions (Layer 2) may only work in interactive sessions where the model has ongoing context and natural pause points.

---

## Results

| Condition | Tasks Done | Rememora Saves | Rememora Searches | Bash Cmds | Latency |
|---|---|---|---|---|---|
| behavioral-triggers | 4/8 | 0 | 0 | 6 | 286s |
| full-hybrid | 7/8 | 0 | 0 | 6 | 1678s |
| none | 0/8 | 0 | 0 | 0 | 19790s |
| reference-card | 1/8 | 0 | 0 | 0 | — |
| hooks-only | aborted | — | — | — | — |

### Observations

1. **full-hybrid completed 7/8 tasks** — the instructions helped with coding quality even though rememora wasn't used
2. **behavioral-triggers completed 4/8** — still better than none (0/8)
3. **none completed 0/8** — without any instructions the model floundered (used all latency budget)
4. **All Bash commands were `ls`/`find`** — the model used Write/Edit/Read for coding, Bash only for exploration

---

## Why Zero Rememora Usage

### Single-Turn Mode (`claude -p`)

Each task is a fresh `claude -p "prompt"` invocation. The model:
- Gets the prompt + appended system prompt
- Focuses entirely on completing the coding task
- Uses all available turns for file creation/editing
- Has no concept of "I might need this later" since it exits after responding
- Never encounters a natural pause point to think about memory management

### Instruction Priority

In single-turn mode, `--append-system-prompt` content competes with the primary task. The model allocates 100% of its reasoning to "write this Express endpoint" and 0% to "also save a decision to rememora." The urgency framing ("your context will be lost") is technically true but the model doesn't experience session boundaries in `-p` mode.

### Turn Budget Competition

With 25 max turns, the model uses them for:
1. Reading the project fixture (Glob, Read)
2. Writing implementation files (Write, Edit)
3. Verification (Bash: ls, find)

There are no "spare" turns for rememora calls. The model optimizes for task completion, not memory management.

---

## Strategic Implications

### What This Means for Rememora

| Integration Layer | Single-Turn (`-p`) | Interactive | Hooks |
|---|---|---|---|
| Instructions (Layer 2+3) | Does not work | Unknown (needs testing) | N/A |
| Hooks (Layer 1) | Deterministic | Deterministic | Deterministic |
| CLAUDE.md | Likely same as instructions | Likely works | N/A |

**Hooks are essential, not optional.** For any automated/programmatic rememora usage (agent-run, agent-loop, CI), only hooks guarantee execution. Instructions are insufficient.

### Revised Experiment Design

To test instruction effectiveness, we need:
1. **Interactive mode testing** — not `claude -p` but actual multi-turn conversations where the model has session context
2. **Explicit "also do memory" tasks** — prompts that include both a coding task AND memory management as part of the acceptance criteria
3. **Longer sessions** — multi-task within a single session rather than separate `-p` invocations per task

### What Still Needs Validation

- Do behavioral triggers work in interactive mode?
- Does the "ASSUME INTERRUPTION" framing work when the model experiences actual session boundaries?
- Is there a turn budget threshold where the model starts allocating turns to rememora?

---

## What Worked

- **Runner fix successful**: Task completion went from 0/8 (Run 1) to 7/8 (Run 2 full-hybrid)
- **Project fixture**: Agent successfully wrote code in the fixture directory
- **Instruction quality effect**: full-hybrid (7/8) >> behavioral-triggers (4/8) >> none (0/8) for task completion
- **Infrastructure**: JSONL output, behavioral logging, comparison all correct

---

## Files

- Run 2 results: `bench/results/longrun_instruction-mode-eval_*_2026-04-02T*.jsonl`
- Run 1 report: `bench/results/REPORT-instruction-mode-eval-run1.md`
- This report: `bench/results/REPORT-instruction-mode-eval-run2.md`
