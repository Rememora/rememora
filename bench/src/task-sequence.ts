import { readFileSync } from "node:fs";

// ---------------------------------------------------------------------------
// Task Sequence — ordered list of tasks that build on each other
// ---------------------------------------------------------------------------

/** A single task within a multi-session sequence. */
export interface SequentialTask {
  /** Unique task identifier within the sequence. */
  id: string;
  /** Human-readable description of what the agent should do. */
  description: string;
  /** The user message sent to the CLI agent for this task. */
  userMessage: string;
  /** Expected outcome for scoring (optional — used by human reviewers or LLM judges). */
  groundTruth?: string;
  /** IDs of prior tasks this one builds on (informational, not enforced). */
  dependsOn?: string[];
}

/** A complete task sequence for multi-session evaluation. */
export interface TaskSequence {
  /** Unique sequence identifier. */
  id: string;
  /** The sample project this sequence targets (e.g., "express-api"). */
  project: string;
  /** Human-readable description of what the sequence evaluates. */
  description?: string;
  /** Ordered list of tasks. */
  tasks: SequentialTask[];
}

// ---------------------------------------------------------------------------
// Experiment Conditions — control how rememora is configured during a run
// ---------------------------------------------------------------------------

/** Instruction delivery mode for the agent. */
export type InstructionMode =
  | "none"
  | "reference-card"
  | "behavioral-triggers"
  | "hooks-only"
  | "full-hybrid";

/** An experiment condition that controls how rememora is presented to the agent. */
export interface ExperimentCondition {
  /** Unique condition identifier. */
  id: string;
  /** How rememora instructions are delivered to the agent. */
  instructionMode: InstructionMode;
  /** Which memory categories the agent is allowed to use (empty = all). */
  categoriesEnabled?: string[];
  /** Whether the KB is pre-seeded with context before the first task. */
  preIndexed?: boolean;
  /** Which CLI agent to use. */
  agent: string;
}

// ---------------------------------------------------------------------------
// Long-run scores — metrics collected during multi-session evaluation
// ---------------------------------------------------------------------------

/** Extended scores for a single task within a long run. */
export interface LongRunScores {
  /** Did the agent complete the task? 0 or 1. */
  task_completion: number;
  /** Quality score for the task output (0-1, placeholder for LLM judge). */
  task_quality: number;
  /** Number of autonomous rememora save calls (not prompted by user message). */
  autonomous_saves: number;
  /** Number of autonomous rememora search calls (not prompted by user message). */
  autonomous_searches: number;
  /** Total tokens consumed for this task (from CLI output if available). */
  tokens_consumed: number;
  /** Number of KB entries when this task started. */
  kb_size_at_start: number;
  /** Number of KB entries when this task ended. */
  kb_size_at_end: number;
  /** Number of new contexts detected in DB after this task (ground truth). */
  db_saves?: number;
  /** Categories of new DB contexts (e.g. {"decision": 1, "case": 1}). */
  db_categories?: Record<string, number>;
}

// ---------------------------------------------------------------------------
// Long-run result — extends EvalRow for multi-session output
// ---------------------------------------------------------------------------

/** A single JSONL row for a task within a long run. */
export interface LongRunEvalRow {
  /** Composite ID: experiment/condition/task/timestamp. */
  id: string;
  input: {
    task: string;
    task_id: string;
    task_index: number;
    sequence_id: string;
    kb_size: number;
    mode: InstructionMode;
  };
  output: {
    commands: string[];
    saves: string[];
    searches: string[];
    raw_output?: string;
    /** Contexts found in DB after this task (ground truth, not parsed). */
    db_new_contexts?: Array<{ category: string | null; name: string; abstract: string }>;
  };
  expected: {
    ground_truth?: string;
    depends_on?: string[];
  };
  scores: LongRunScores;
  metadata: {
    experiment: string;
    condition: string;
    task_index: number;
    sequence_id: string;
    cli: string;
    latency_ms: number;
    timestamp: string;
  };
}

// ---------------------------------------------------------------------------
// Loaders
// ---------------------------------------------------------------------------

/** Load a task sequence from a JSON file. */
export function loadTaskSequence(path: string): TaskSequence {
  const raw = readFileSync(path, "utf-8");
  const data = JSON.parse(raw) as TaskSequence;

  if (!data.id || !data.project || !Array.isArray(data.tasks)) {
    throw new Error(
      `Invalid task sequence file: ${path}. Must have id, project, and tasks array.`,
    );
  }

  if (data.tasks.length === 0) {
    throw new Error(`Task sequence ${path} has no tasks.`);
  }

  for (const task of data.tasks) {
    if (!task.id || !task.description || !task.userMessage) {
      throw new Error(
        `Invalid task in ${path}: each task must have id, description, and userMessage.`,
      );
    }
  }

  return data;
}

/** Load an experiment condition from a JSON file. */
export function loadCondition(path: string): ExperimentCondition {
  const raw = readFileSync(path, "utf-8");
  const data = JSON.parse(raw) as ExperimentCondition;

  if (!data.id || !data.instructionMode || !data.agent) {
    throw new Error(
      `Invalid condition file: ${path}. Must have id, instructionMode, and agent.`,
    );
  }

  const validModes: InstructionMode[] = [
    "none",
    "reference-card",
    "behavioral-triggers",
    "hooks-only",
    "full-hybrid",
  ];
  if (!validModes.includes(data.instructionMode)) {
    throw new Error(
      `Invalid instructionMode "${data.instructionMode}" in ${path}. Must be one of: ${validModes.join(", ")}`,
    );
  }

  return data;
}
