# Instruction Design: Autonomous Model Behavior for Rememora

*Research date: 2026-04-01 | Issue: [#18](https://github.com/Rememora/rememora/issues/18)*

---

## The Core Problem

Rememora's current setup instructions are a **command reference card** — they tell agents WHAT commands exist, but not WHEN to autonomously use them. The eval scenarios confirm this: they all require imperative user prompts to trigger rememora usage.

**Goal: Make the model autonomously decide when to read and write, without user prompting.**

---

## How Existing Memory Systems Solve This

### Claude Code's Auto-Memory (most sophisticated)

Uses structured behavioral triggers per memory type:

```xml
<type>
  <name>feedback</name>
  <when_to_save>Any time the user corrects your approach ("no not that", "don't") 
    OR confirms a non-obvious approach worked ("yes exactly", "perfect")</when_to_save>
  <how_to_use>Let these memories guide behavior so the user doesn't repeat themselves</how_to_use>
</type>
```

Key mechanisms:
- **5 memory types** each with explicit `when_to_save` and `how_to_use` triggers
- **Urgency framing**: Information NOT saved will be lost across conversations
- **"Auto Dream" consolidation**: Periodic LLM-powered merge (dual-gate: enough sessions + enough time)
- **First 200 lines of MEMORY.md auto-loaded** every session
- **Negative constraints**: Explicit "What NOT to save"
- **Example pairs**: Concrete trigger -> action examples per type

Sources:
- [Claude Code Memory docs](https://code.claude.com/docs/en/memory)
- [Piebald-AI: Claude Code System Prompts](https://github.com/Piebald-AI/claude-code-system-prompts)
- [ClaudeFast: Auto Memory](https://claudefa.st/blog/guide/mechanics/auto-memory)
- [ClaudeFast: Auto Dream](https://claudefa.st/blog/guide/mechanics/auto-dream)

### Gemini CLI's save_memory (simplest)

**Primarily reactive** — triggers on user requests or clear user-stated facts:

> "If unsure whether to save something, you can ask the user."

No proactive saving. No consolidation. Appends bullet points to GEMINI.md.

Sources:
- [memoryTool.ts source](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/memoryTool.ts)

### Codex CLI's Memory Pipeline (most automated)

Two-phase extraction requiring zero model judgment:
1. **Phase 1**: Mini-model extracts memories from session transcript post-hoc
2. **Phase 2**: "Memory Writing Agent" consolidates into `memory_summary.md` (truncated to 5,000 tokens)
3. **Citation tracking**: `<oai-mem-citation>` blocks track actual usage, informing pruning

Key insight: Codex doesn't ask the model to DECIDE what to save — it saves EVERYTHING and prunes later.

Sources:
- [DeepWiki: Codex Memory System](https://deepwiki.com/openai/codex/3.7-memory-system)
- [Codex Memory Deep Dive](https://mer.vin/2025/12/openai-codex-cli-memory-deep-dive/)

### Claude API Memory Tool (best urgency pattern)

```
ASSUME INTERRUPTION: Your context window might be reset at any moment, 
so you risk losing any progress that is not recorded in memory.
```

Creates intrinsic motivation for proactive saving without enumerating every trigger.

Source: [Claude API Memory Tool docs](https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool)

---

## Instruction Design Research

### Capacity Constraints

- Frontier LLMs follow **~150-200 instructions** with reasonable consistency
- Claude Code's system prompt uses ~50 of those
- Quality degrades **uniformly across ALL instructions** as count increases
- Instructions at **beginning and end** of context are followed more reliably (primacy/recency bias)

### What Gets Followed vs Ignored

| Reliably Followed | Reliably Ignored |
|---|---|
| Specific, verifiable: "Use 2-space indent" | Vague: "Keep files organized" |
| Concrete commands: "Run `npm test` before committing" | Task-specific rules in universal context |
| Negative constraints: "Do NOT use class components" | Contradictory instructions |
| Conditional triggers: "When [X], then [Y]" | Instructions in the middle of long sections |
| Urgency-framed: "You will lose this if..." | "Hotfix" instructions for one-off problems |

### The Key Pattern: Conditional Behavioral Triggers

Most effective format for autonomous behavior:

```
When [observable condition the model can detect], then [specific action with exact command].
```

Sources:
- [Anthropic: Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
- [HumanLayer: Writing a good CLAUDE.md](https://www.humanlayer.dev/blog/writing-a-good-claude-md)
- [Builder.io: Improve your AI code output with AGENTS.md](https://www.builder.io/blog/agents-md)

---

## Proposed Three-Layer Architecture

### Layer 1: Hooks (Deterministic)

These ALWAYS fire regardless of model behavior:

```json
{
  "hooks": {
    "SessionStart": [{
      "type": "command",
      "command": "rememora context --auto --format json 2>/dev/null || true"
    }],
    "SessionEnd": [{
      "type": "command", 
      "command": "rememora session end-active --auto-summary 2>/dev/null || true"
    }]
  }
}
```

### Layer 2: Behavioral Triggers (Instructions)

**SAVE triggers (when to write):**

| When you... | Category | Why |
|---|---|---|
| Make or witness a design/architectural decision | `decision` | Other agents need to know what was decided and why |
| Solve a non-trivial bug or workaround | `case` | Future agents hitting the same issue can find the solution |
| Discover a codebase pattern, convention, or idiom | `pattern` | Prevents agents from reinventing or violating conventions |
| Learn a user preference or get corrected | `preference` | All agents should respect user preferences |
| Discover an important entity (service, API, config) | `entity` | Reduces re-exploration time for future sessions |
| Observe a notable event (deploy, incident, migration) | `event` | Provides timeline context for debugging |

**SEARCH triggers (when to read):**

| When you... | Search for |
|---|---|
| Are about to implement something non-trivial | Prior decisions, patterns, and cases in this area |
| Encounter unfamiliar code or architecture | Entity knowledge about this part of the codebase |
| Hear the user reference past work or decisions | The specific context they're referencing |
| Are about to make a decision that could conflict with a past one | Prior decisions on the same topic |
| Are stuck or blocked on a problem | Related cases and patterns |
| Switch between projects or contexts | Full project context |

### Layer 3: Meta-Instruction (Urgency + Identity)

```markdown
ASSUME INTERRUPTION: Your context will be lost when this session ends. 
Anything not saved to rememora is gone forever — for you AND for every 
other agent that works on this project after you.

You are not the only agent working here. Save knowledge that others will need.
Search before you assume — someone may have already solved this.
```

---

## Draft: Redesigned Instruction Snippet

```markdown
## Rememora — Cross-Agent Persistent Memory

You have access to `rememora`, a persistent memory system shared across agents and sessions.
Your context will be lost when this session ends. Anything not saved to rememora is gone forever.
You are not the only agent on this project — save what others will need, search before you assume.

### When to SEARCH (before acting)
- Before implementing anything non-trivial -> `rememora search "relevant topic" --project <name>`
- When encountering unfamiliar code/architecture -> search for entity knowledge
- When the user references past work or decisions -> search for that context
- When stuck or blocked -> search for related cases and patterns

### When to SAVE (as you work)
Save immediately when any of these happen — do not batch or defer:
- **Decision made**: team chose an approach, trade-off, or technology -> `rememora save "..." --category decision --importance 0.8 --project <name>`
- **Bug solved**: non-trivial fix, workaround, or gotcha discovered -> `rememora save "..." --category case --project <name>`
- **Pattern found**: convention, idiom, or reusable approach -> `rememora save "..." --category pattern --project <name>`
- **User corrected you** or stated a preference -> `rememora save "..." --category preference`
- **Entity discovered**: service, API, config, key integration point -> `rememora save "..." --category entity --project <name>`

### What NOT to save
- Code that can be read from files (use file paths instead)
- Git history (use `git log`)
- Anything already in the project README or docs
- Temporary debugging state

### Sessions
- Start: `rememora session start --agent claude-code --project <name> --intent "..."`
- End: `rememora session end <id> --summary "..." --working-state "..."`
- Transfer: `rememora session end <id> --status transferred --summary "..." --working-state "..."`
```

### Key Differences from Current Instructions

| Current | Proposed |
|---|---|
| "During work -- save knowledge as you discover it" (vague) | Specific conditional triggers with category mappings |
| No search triggers | 4 specific "when to search" conditions |
| No urgency framing | "ASSUME INTERRUPTION" + multi-agent motivation |
| No negative constraints | "What NOT to save" prevents noise |
| Session start in instructions | Session start moved to hooks (deterministic) |
| Reference card style | Behavioral specification style |
