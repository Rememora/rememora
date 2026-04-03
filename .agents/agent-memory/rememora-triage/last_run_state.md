---
timestamp: 2026-04-03T00:00:00Z
run_by: rememora-triage agent
---

# Last Triage Run State

## Board Summary (Project #3)

**Total items**: 20

### Done (12)
#1 Setup CLI polish with cliclack
#2 rememora eval: DB compliance metrics
#3 Eval benchmark harness (TypeScript, multi-model)
#4 Vector search: candle + sqlite-vec
#6 Memory evolution: LLM-based consolidation
#8 Local agent loop: auto-dispatch issues to Claude CLI
#18 Investigate claude-cli:// deep links and cross-agent protocol
#20 Add `session end-active` command for hook integration
#21 Add SessionStart/SessionEnd hooks to `rememora setup`
#22 Redesign instruction snippets with behavioral triggers
#23 Extend eval bench for multi-session task sequences
#24 Experiment: Instruction mode comparison (P0)

### Ready-For-Dev (1 confirmed + 4 pending user approval)
#19 Adopt BDD-style test structure with Given-When-Then scenarios — confirmed correct, NOT_STARTED

**Pending approval to move to Ready-For-Dev:**
- #25 Experiment: Knowledge transfer A/B — deps (#23, #24) Done, infra exists
- #26 Experiment: Knowledge base growth curve — dep (#23) Done, infra exists
- #27 Experiment: Discovery agent pre-indexing — dep (#23) Done, agent-driven option available
- #28 Experiment: Category value ranking — dep (#23) Done, conditions matrix ready

### Todo (staying, with reasons)
#5 Hierarchical retrieval with score propagation — scope underspecified (4 bullet AC), NOT_STARTED, no infra blocker. Flagged for human call.
#7 TUI dashboard — P3/XL, no urgency, NOT_STARTED
#29 Experiment: Cross-agent knowledge transfer — Codex runner exists, Gemini runner ABSENT. Partially blocked.

## Project Board Field IDs (for GraphQL mutations)
- Project ID: PVT_kwDOCB405M4BSdN1
- Status Field ID: PVTSSF_lADOCB405M4BSdN1zg__B7M
- Status Options:
  - Todo: f75ad846
  - Ready-For-Dev: eafe2cca
  - In Progress: 47fc9ee4
  - Ready-For-Review: 7e86c92f
  - Cherry-Picked: 4e5b3b65
  - Done: 98236657

## PVTI Item IDs (for board mutations)
- #3: PVTI_lADOCB405M4BSdN1zgoBbXY
- #4: PVTI_lADOCB405M4BSdN1zgoBbXk
- #1: PVTI_lADOCB405M4BSdN1zgoBbW8
- #2: PVTI_lADOCB405M4BSdN1zgoBbXM
- #5: PVTI_lADOCB405M4BSdN1zgoBbXw
- #6: PVTI_lADOCB405M4BSdN1zgoBbX4
- #7: PVTI_lADOCB405M4BSdN1zgoBbYE
- #8: PVTI_lADOCB405M4BSdN1zgoBros
- #18: PVTI_lADOCB405M4BSdN1zgo4jZs
- #19: PVTI_lADOCB405M4BSdN1zgo4yQI
- #20: PVTI_lADOCB405M4BSdN1zgo6ed0
- #21: PVTI_lADOCB405M4BSdN1zgo6eeA
- #22: PVTI_lADOCB405M4BSdN1zgo6eeY
- #23: PVTI_lADOCB405M4BSdN1zgo6eeg
- #24: PVTI_lADOCB405M4BSdN1zgo6efA
- #25: PVTI_lADOCB405M4BSdN1zgo6efs
- #26: PVTI_lADOCB405M4BSdN1zgo6egU
- #27: PVTI_lADOCB405M4BSdN1zgo6eg0
- #28: PVTI_lADOCB405M4BSdN1zgo6ehc
- #29: PVTI_lADOCB405M4BSdN1zgo6eh4

## Key Findings from This Run
- Multi-session eval infra (#23) is complete: `task-sequence.ts`, `behavioral-logger.ts`, `long-run.ts`, `long-run.eval.ts`, `compare-conditions.ts`, `run-long.ts` all exist in `bench/src/`
- Conditions matrix populated: `bench/conditions/` has `behavioral-triggers.json`, `full-hybrid.json`, `full-hybrid-tmux.json`, `hooks-only.json`, `none.json`, `none-tmux.json`, `reference-card.json`, `instruction-mode-eval.json`
- Sample task sequence exists: `bench/tasks/sample-sequence.json` and `instruction-mode-eval.json`
- Runners available: `claude-code.ts`, `claude-tmux.ts`, `codex.ts` — NO Gemini runner
- `hierarchy.rs` has L0/L1/L2 tiered loading but NO score propagation logic (parent/child boost, recursive URI traversal)
- `tests/` uses old `test_*` naming — no `scenarios/` dir, no `SCENARIOS.md`, no BDD structure
- No `ratatui` in `Cargo.toml`, no TUI code anywhere

## Pending User Decisions
1. Approve moving #25, #26, #27, #28 to Ready-For-Dev
2. Decision on #5: move to Ready-For-Dev (scope-thin) or add AC first
3. Decision on #29: Codex-only run (move to Ready-For-Dev) or wait for Gemini runner
