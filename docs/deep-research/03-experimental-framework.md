# Experimental Framework: Measuring Rememora's Value at Scale

*Research date: 2026-04-01 | Issue: [#18](https://github.com/Rememora/rememora/issues/18)*

---

## Core Questions

1. **Does persistent memory help?** — Does an agent with accumulated knowledge perform better than one starting fresh?
2. **When does memory hurt?** — At what point does stored knowledge become noise?
3. **What's worth storing?** — Which categories provide the most retrieval value?
4. **Pre-indexed vs on-demand?** — Does a discovery agent help or pollute?
5. **Which instruction mode works best?** — Reference card vs behavioral triggers vs hooks vs hybrid?

---

## State of the Art

### Relevant Benchmarks

| Benchmark | What It Tests | Multi-Session? | Source |
|---|---|---|---|
| **LongMemEval** (ICLR 2025) | 5 memory abilities across 30-500 sessions | Yes (synthetic) | [arxiv.org/abs/2410.10813](https://arxiv.org/abs/2410.10813) |
| **ToM-SWE** (Oct 2025) | User preference modeling across coding sessions | Yes (real) | [arxiv.org/abs/2510.21903](https://arxiv.org/html/2510.21903v1) |
| **AWM** (ICML 2025) | Knowledge reuse across task episodes (51% improvement) | Yes (within agent) | [arxiv.org/abs/2409.07429](https://arxiv.org/html/2409.07429v1) |
| **tau-bench** (Sierra) | Multi-turn tool use with pass^k reliability | No (single session) | [arxiv.org/abs/2406.12045](https://arxiv.org/abs/2406.12045) |
| **Context-Bench** (Letta) | Long-horizon with cost efficiency | No | [letta.com/blog/context-bench](https://www.letta.com/blog/context-bench) |
| **MemTrack** (NeurIPS 2025 WS) | Enterprise multi-platform memory | Yes | [arxiv.org/pdf/2510.01353](https://arxiv.org/pdf/2510.01353) |

### The Gap

**No benchmark tests**: "Agent A stores knowledge, then Agent B retrieves it to solve a task better." This is Rememora's exact value proposition and an open evaluation frontier.

### Critical Research Findings

1. **Context poisoning** (ICLR 2025): Increasing retrieved passages does NOT consistently improve performance. Performance rises then FALLS.
2. **Lost in the middle** (Stanford/UW, TACL 2024): Middle-position information suffers >30% accuracy drops.
3. **Context rot** (Chroma, 2025): Every model tested degrades with longer context. "More context" != "better."
4. **Selective deletion beats hoarding** (Memory survey, Dec 2025): Utility-based deletion yields ~10% gains over naive storage.
5. **Agent context becomes noise** (JetBrains, NeurIPS 2025): Observation masking matched LLM summarization in both cost and quality.

**Implication: Rememora's value is about precision, not recall.**

---

## Proposed Experiments

### Experiment 1: Knowledge Transfer A/B ("Does memory help?")

**Setup:**
- Real project with 10-20 sequential, related issues
- **Control**: Fresh session per task, no rememora
- **Treatment**: Rememora persists between sessions

**Metrics:**
- Task completion rate
- Task quality (LLM-as-judge)
- Tokens to completion
- Redundant exploration (re-discovered vs searched)
- Error propagation (bad memory -> worse outcome)

**Borrowed from:** AWM's episode-accumulation methodology.

### Experiment 2: Knowledge Base Growth Curve ("When does memory hurt?")

**Setup:**
- Single project, 30-50 task sequence
- Rememora accumulates, no pruning

**Measurements at 5-task intervals:**
- Retrieval precision@5
- Task completion quality
- Tokens per task
- Retrieval latency

**Expected:** Performance improves then plateaus/degrades. Finding the inflection point tells us when consolidation is needed.

### Experiment 3: Category Value Ranking ("What's worth storing?")

**Setup:**
- Same task sequence, 6 treatment groups (one category each) + control + all

**Metrics:**
- Per-category retrieval rate
- Per-category utilization rate
- Task completion delta vs control

**Borrowed from:** OpenAI's memory write quality / read quality decomposition.

### Experiment 4: Instruction Mode Comparison ("Which mode works best?")

**Setup:**
- 5 instruction modes:
  1. No instructions (rememora installed but unmentioned)
  2. Current instructions (reference card)
  3. Behavioral triggers
  4. Hooks + minimal instructions
  5. Full hybrid (hooks + triggers + urgency)

**Metrics:**
- Autonomous save/search frequency
- Save quality (fraction later retrieved/useful)
- Search effectiveness (influenced response?)
- Task completion quality

### Experiment 5: Discovery Agent Pre-Indexing ("Does pre-knowledge help?")

**Setup:**
- Medium project (~50-100 files)
- Phase 1: Discovery agent indexes codebase
- Phase 2: Task sequence under three conditions:
  1. Cold start (no rememora)
  2. Discovery-primed only
  3. Discovery + continued accumulation

**Metrics:**
- Time-to-first-correct-action
- Retrieval relevance (pre-indexed vs session-accumulated)
- Context pollution rate
- Staleness after N code-modifying tasks

### Experiment 6: Cross-Agent Transfer ("Does shared memory work?")

**Setup:**
- Task sequence split: Agent A (tasks 1-5) -> Agent B (tasks 6-10)
- Session transfer via rememora

**Metrics:**
- Agent B's utilization of Agent A's knowledge
- Task quality delta vs fresh start
- Knowledge format compatibility across agents

---

## Measurement Framework

### Metrics Taxonomy

**Retrieval Quality:**
- Context Precision@k
- Context Recall
- MRR (Mean Reciprocal Rank)
- Latency at DB sizes: 10, 100, 1K, 10K

**Storage Quality:**
- Save precision (fraction later useful)
- Save recall (fraction of "should-saves" captured)
- Category accuracy
- Noise ratio (never retrieved / total)

**Task Performance:**
- Completion rate (binary)
- Quality score (LLM-as-judge, 1-5)
- Token efficiency
- pass^k reliability (tau-bench metric)

**Behavioral:**
- Autonomous save frequency
- Autonomous search frequency
- Save timing (right moment?)
- Search timing (before acting, not after?)

### JSONL Output Format

```jsonl
{
  "id": "exp1/task-7/run-3",
  "input": {"task": "...", "knowledge_base_size": 42, "mode": "behavioral-triggers"},
  "output": {"patch": "...", "saves": [...], "searches": [...]},
  "expected": {"patch_correct": true},
  "scores": {
    "task_completion": 1,
    "task_quality": 4,
    "retrieval_precision": 0.8,
    "save_precision": 0.7,
    "autonomous_saves": 3,
    "autonomous_searches": 2,
    "tokens_consumed": 45000
  },
  "metadata": {
    "experiment": "knowledge_transfer_ab",
    "condition": "with_memory",
    "task_index": 7,
    "kb_size_at_start": 42,
    "agent": "claude-code"
  }
}
```

---

## The Discovery Agent Concept

### Option A: `rememora discover` command

- Tree-sitter + glob for structural extraction
- LLM for semantic summarization
- Stores with categories and importance scores
- Runs periodically or on-demand

### Option B: Agent-driven discovery

- Purpose-built agent instruction: "explore this codebase and store everything you learn"
- Uses native code understanding (grep, file read, AST)
- More expensive, captures higher-level semantics

### Option C: Hybrid

- Structural pass: tree-sitter -> entity entries
- Semantic pass: LLM summarizes key modules -> pattern + decision entries
- Differential updates: on git diff, re-index only changed files

### Risks

- **Staleness**: Pre-indexed knowledge rots as code changes
- **Noise**: Indiscriminate indexing creates low-signal DB
- **Cost**: LLM extraction is expensive at scale
- **Duplication**: May duplicate what agents discover on-demand

---

## Extending the Eval Infrastructure

Current `bench/` handles single-turn scenarios. For long-running evals:

```typescript
interface TaskSequence {
  id: string;
  project: string;
  tasks: SequentialTask[];
}

interface SequentialTask {
  id: string;
  description: string;
  groundTruth?: string;
  dependsOn?: string[];
}

interface ExperimentCondition {
  id: string;
  instructionMode: "none" | "reference-card" | "behavioral-triggers" | "hooks-only" | "full-hybrid";
  categoriesEnabled: string[];
  preIndexed: boolean;
  agent: "claude-code" | "codex" | "gemini";
}
```

New infrastructure needed:
1. **Persistent DB mode** — knowledge accumulates across task sequences
2. **Task sequences** — ordered lists where later tasks benefit from earlier knowledge
3. **Condition matrix** — same sequence under different conditions
4. **Behavioral logging** — capture every save/search call
5. **Baseline comparison** — always run a "no memory" control

---

## Implementation Priority

| Priority | Experiment | Effort | Value |
|---|---|---|---|
| **P0** | Exp 4: Instruction mode comparison | Low | Answers "which mode is best" |
| **P0** | Exp 1: Knowledge transfer A/B | Medium | Validates fundamental value proposition |
| **P1** | Exp 2: Growth curve | Medium | Determines consolidation needs |
| **P1** | Exp 5: Discovery agent | Medium | Tests pre-indexing value |
| **P2** | Exp 3: Category ranking | High | Optimizes storage strategy |
| **P2** | Exp 6: Cross-agent transfer | High | Validates cross-agent differentiator |

---

## References

- [LongMemEval](https://github.com/xiaowu0162/LongMemEval) — Multi-session memory benchmark (ICLR 2025)
- [AWM](https://arxiv.org/html/2409.07429v1) — Agent Workflow Memory, 51% improvement (ICML 2025)
- [tau-bench](https://arxiv.org/abs/2406.12045) — pass^k reliability metric (Sierra)
- [Context poisoning](https://proceedings.iclr.cc/paper_files/paper/2025/file/5df5b1f121c915d8bdd00db6aac20827-Paper-Conference.pdf) — ICLR 2025
- [Context rot](https://research.trychroma.com/context-rot) — Chroma
- [Lost in the middle](https://cs.stanford.edu/~nfliu/papers/lost-in-the-middle.arxiv2023.pdf) — Stanford
- [JetBrains agent context](https://blog.jetbrains.com/research/2025/12/efficient-context-management/) — NeurIPS 2025
- [Memory in AI Agents survey](https://arxiv.org/abs/2512.13564) — Dec 2025
- [OpenAI memory eval](https://developers.openai.com/cookbook/examples/agents_sdk/context_personalization)
- [METR time horizons](https://metr.org/blog/2025-03-19-measuring-ai-ability-to-complete-long-tasks/)
- [Zep/Graphiti](https://arxiv.org/abs/2501.13956) — Temporal knowledge graphs (Jan 2025)
- [A-MEM](https://arxiv.org/abs/2502.12110) — Agentic memory (NeurIPS 2025)
- [Anthropic: Context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
