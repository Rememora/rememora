import Anthropic from "@anthropic-ai/sdk";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface JudgeInput {
  taskDescription: string;
  groundTruth: string | undefined;
  agentOutput: string;
  taskIndex: number;
  kbSizeAtStart: number;
  kbSizeAtEnd: number;
  dbNewContexts: Array<{ category: string | null; name: string; abstract: string }>;
  isControlCondition: boolean;
}

export interface JudgeResult {
  /** Quality score from 0 to 1. */
  score: number;
  /** Short explanation from the model. */
  reasoning: string;
}

// ---------------------------------------------------------------------------
// Rubric prompt
// ---------------------------------------------------------------------------

const RUBRIC_PROMPT = `You are evaluating an AI coding agent's performance on a development task.

## Task
{task_description}

## Expected Outcome
{ground_truth}

## Agent Output (last 3000 chars)
{agent_output}

## Context
- Task position in sequence: task {task_index} (0-indexed)
- Knowledge base entries at start: {kb_size_at_start}
- Knowledge base entries at end: {kb_size_at_end}
- New memories saved: {new_context_count}
- Condition: {condition_type}

## Scoring Rubric
Rate the agent's output on a scale of 0.0 to 1.0:

1. **Task Completion** (0-0.4): Did the agent produce code/output that addresses the task requirements?
   - 0.0: No relevant output
   - 0.2: Partial attempt, major gaps
   - 0.4: Complete and correct implementation

2. **Knowledge Utilization** (0-0.3): Did the agent appropriately use or build upon prior knowledge?
   - For treatment (persisted KB): Did the agent search for and apply prior decisions/patterns?
   - For control (wiped KB): Did the agent correctly re-derive or re-discover needed context?
   - 0.0: No evidence of knowledge use
   - 0.15: Some evidence
   - 0.3: Strong evidence

3. **Quality** (0-0.3): Is the output well-structured, following best practices?
   - 0.0: Poor quality
   - 0.15: Acceptable
   - 0.3: High quality

Respond with EXACTLY this JSON format (no other text):
{"score": <number 0.0-1.0>, "reasoning": "<1-2 sentence explanation>"}`;

// ---------------------------------------------------------------------------
// Client singleton
// ---------------------------------------------------------------------------

let client: Anthropic | null = null;

function getClient(): Anthropic | null {
  if (client) return client;
  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) return null;
  client = new Anthropic({ apiKey });
  return client;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

export function isJudgeAvailable(): boolean {
  return !!process.env.ANTHROPIC_API_KEY;
}

export async function judgeTask(input: JudgeInput): Promise<JudgeResult> {
  const anthropic = getClient();
  if (!anthropic) {
    return { score: 0, reasoning: "No ANTHROPIC_API_KEY — skipped LLM judge" };
  }

  const prompt = RUBRIC_PROMPT
    .replace("{task_description}", input.taskDescription)
    .replace("{ground_truth}", input.groundTruth ?? "No ground truth provided")
    .replace("{agent_output}", input.agentOutput.slice(-3000))
    .replace("{task_index}", String(input.taskIndex))
    .replace("{kb_size_at_start}", String(input.kbSizeAtStart))
    .replace("{kb_size_at_end}", String(input.kbSizeAtEnd))
    .replace("{new_context_count}", String(input.dbNewContexts.length))
    .replace(
      "{condition_type}",
      input.isControlCondition
        ? "control (DB wiped between tasks)"
        : "treatment (DB persists across tasks)",
    );

  try {
    const response = await anthropic.messages.create({
      model: "claude-haiku-4-5-20251001",
      max_tokens: 256,
      messages: [{ role: "user", content: prompt }],
    });

    const text =
      response.content[0].type === "text" ? response.content[0].text : "";
    const parsed = JSON.parse(text);
    return {
      score: Math.max(0, Math.min(1, Number(parsed.score) || 0)),
      reasoning: String(parsed.reasoning || ""),
    };
  } catch (err) {
    return { score: 0, reasoning: `LLM judge error: ${err}` };
  }
}
