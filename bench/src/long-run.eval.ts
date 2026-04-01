import { describe, it, expect } from "vitest";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

import { loadTaskSequence, loadCondition } from "./task-sequence.js";
import type {
  TaskSequence,
  ExperimentCondition,
  LongRunEvalRow,
  LongRunScores,
} from "./task-sequence.js";
import { extractRemoraCalls } from "./behavioral-logger.js";
import type { BehaviorSummary } from "./behavioral-logger.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const BENCH_DIR = join(__dirname, "..");

// ---------------------------------------------------------------------------
// Task Sequence Loader
// ---------------------------------------------------------------------------

describe("loadTaskSequence", () => {
  it("loads sample-sequence.json with all required fields", () => {
    const seq = loadTaskSequence(join(BENCH_DIR, "tasks/sample-sequence.json"));

    expect(seq.id).toBe("express-api-buildup");
    expect(seq.project).toBe("express-api");
    expect(seq.tasks).toHaveLength(5);
  });

  it("each task has id, description, and userMessage", () => {
    const seq = loadTaskSequence(join(BENCH_DIR, "tasks/sample-sequence.json"));

    for (const task of seq.tasks) {
      expect(typeof task.id).toBe("string");
      expect(task.id.length).toBeGreaterThan(0);
      expect(typeof task.description).toBe("string");
      expect(task.description.length).toBeGreaterThan(0);
      expect(typeof task.userMessage).toBe("string");
      expect(task.userMessage.length).toBeGreaterThan(0);
    }
  });

  it("tasks have optional groundTruth and dependsOn", () => {
    const seq = loadTaskSequence(join(BENCH_DIR, "tasks/sample-sequence.json"));

    // First task has groundTruth but no dependsOn
    expect(seq.tasks[0].groundTruth).toBeDefined();
    expect(seq.tasks[0].dependsOn).toBeUndefined();

    // Last task has both
    const last = seq.tasks[seq.tasks.length - 1];
    expect(last.groundTruth).toBeDefined();
    expect(last.dependsOn).toBeDefined();
    expect(Array.isArray(last.dependsOn)).toBe(true);
  });

  it("throws on invalid file", () => {
    expect(() => loadTaskSequence("/nonexistent/file.json")).toThrow();
  });
});

// ---------------------------------------------------------------------------
// Condition Loader
// ---------------------------------------------------------------------------

describe("loadCondition", () => {
  it("loads none.json", () => {
    const cond = loadCondition(join(BENCH_DIR, "conditions/none.json"));
    expect(cond.id).toBe("none");
    expect(cond.instructionMode).toBe("none");
    expect(cond.agent).toBe("claude-code");
  });

  it("loads reference-card.json", () => {
    const cond = loadCondition(join(BENCH_DIR, "conditions/reference-card.json"));
    expect(cond.id).toBe("reference-card");
    expect(cond.instructionMode).toBe("reference-card");
    expect(cond.categoriesEnabled).toHaveLength(6);
  });

  it("loads full-hybrid.json", () => {
    const cond = loadCondition(join(BENCH_DIR, "conditions/full-hybrid.json"));
    expect(cond.id).toBe("full-hybrid");
    expect(cond.instructionMode).toBe("full-hybrid");
    expect(cond.preIndexed).toBe(true);
  });

  it("throws on invalid instructionMode", () => {
    expect(() =>
      loadCondition(join(BENCH_DIR, "conditions/nonexistent.json")),
    ).toThrow();
  });
});

// ---------------------------------------------------------------------------
// Behavioral Logger
// ---------------------------------------------------------------------------

describe("extractRemoraCalls", () => {
  it("extracts save calls from commands", () => {
    const commands = [
      'rememora save "PostgreSQL chosen for ACID" --category decision --project acme',
      "echo hello",
    ];

    const result = extractRemoraCalls("", commands, "save this decision");

    expect(result.saves).toHaveLength(1);
    expect(result.saves[0].subcommand).toBe("save");
    expect(result.saves[0].autonomous).toBe(false); // user said "save"
  });

  it("extracts search calls from commands", () => {
    const commands = [
      'rememora search "database decision" --project acme',
    ];

    const result = extractRemoraCalls("", commands, "what database did we pick?");

    expect(result.searches).toHaveLength(1);
    expect(result.searches[0].subcommand).toBe("search");
    // User didn't say "search", "recall", etc.
    expect(result.searches[0].autonomous).toBe(true);
  });

  it("classifies autonomous saves correctly", () => {
    const commands = [
      'rememora save "auth uses RS256" --category decision --project acme',
    ];

    // User message doesn't mention save/remember/store
    const result = extractRemoraCalls(
      "",
      commands,
      "Add JWT authentication middleware",
    );

    expect(result.saves).toHaveLength(1);
    expect(result.saves[0].autonomous).toBe(true);
    expect(result.autonomousSaveCount).toBe(1);
  });

  it("classifies prompted searches correctly", () => {
    const commands = [
      'rememora search "auth decision" --project acme',
    ];

    const result = extractRemoraCalls(
      "",
      commands,
      "Search memory for our auth decisions",
    );

    expect(result.searches).toHaveLength(1);
    expect(result.searches[0].autonomous).toBe(false);
    expect(result.autonomousSearchCount).toBe(0);
  });

  it("handles mixed commands", () => {
    const commands = [
      "rememora context --auto",
      "rememora session start --agent claude --project acme",
      'rememora search "database" --project acme',
      'rememora save "found the issue" --category case --project acme',
    ];

    const result = extractRemoraCalls(
      "",
      commands,
      "Fix the database connection bug",
    );

    expect(result.saves).toHaveLength(1);
    expect(result.searches).toHaveLength(1);
    expect(result.other).toHaveLength(2); // context + session
    expect(result.totalCalls).toBe(4);
    // "Fix" doesn't match save/search prompt patterns
    expect(result.autonomousSaveCount).toBe(1);
    expect(result.autonomousSearchCount).toBe(1);
  });

  it("returns empty summary for no rememora calls", () => {
    const result = extractRemoraCalls("", ["echo hello", "ls -la"], "do something");

    expect(result.saves).toHaveLength(0);
    expect(result.searches).toHaveLength(0);
    expect(result.other).toHaveLength(0);
    expect(result.totalCalls).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// LongRunEvalRow shape validation
// ---------------------------------------------------------------------------

describe("LongRunEvalRow shape", () => {
  const TIMESTAMP = "2026-04-01T12:00:00.000Z";

  function makeSampleRow(): LongRunEvalRow {
    return {
      id: `express-api-buildup/full-hybrid/init-project/${TIMESTAMP}`,
      input: {
        task: "Initialize an Express API project",
        task_id: "init-project",
        task_index: 0,
        sequence_id: "express-api-buildup",
        kb_size: 0,
        mode: "full-hybrid",
      },
      output: {
        commands: [
          'rememora save "PostgreSQL + Prisma for ACID" --category decision --project express-api',
        ],
        saves: [
          'rememora save "PostgreSQL + Prisma for ACID" --category decision --project express-api',
        ],
        searches: [],
      },
      expected: {
        ground_truth:
          "Agent should create project structure and save a decision memory.",
        depends_on: undefined,
      },
      scores: {
        task_completion: 1,
        task_quality: 0,
        autonomous_saves: 0,
        autonomous_searches: 0,
        tokens_consumed: 0,
        kb_size_at_start: 0,
        kb_size_at_end: 1,
      },
      metadata: {
        experiment: "express-api-buildup_full-hybrid",
        condition: "full-hybrid",
        task_index: 0,
        sequence_id: "express-api-buildup",
        cli: "claude-code",
        latency_ms: 5000,
        timestamp: TIMESTAMP,
      },
    };
  }

  it("has all required top-level keys", () => {
    const row = makeSampleRow();
    expect(row).toHaveProperty("id");
    expect(row).toHaveProperty("input");
    expect(row).toHaveProperty("output");
    expect(row).toHaveProperty("expected");
    expect(row).toHaveProperty("scores");
    expect(row).toHaveProperty("metadata");
  });

  it("input contains task context and experiment info", () => {
    const row = makeSampleRow();
    expect(typeof row.input.task).toBe("string");
    expect(typeof row.input.task_id).toBe("string");
    expect(typeof row.input.task_index).toBe("number");
    expect(typeof row.input.sequence_id).toBe("string");
    expect(typeof row.input.kb_size).toBe("number");
    expect(typeof row.input.mode).toBe("string");
  });

  it("output contains commands, saves, and searches arrays", () => {
    const row = makeSampleRow();
    expect(Array.isArray(row.output.commands)).toBe(true);
    expect(Array.isArray(row.output.saves)).toBe(true);
    expect(Array.isArray(row.output.searches)).toBe(true);
  });

  it("scores has all LongRunScores fields as numbers", () => {
    const row = makeSampleRow();
    const scoreKeys: (keyof LongRunScores)[] = [
      "task_completion",
      "task_quality",
      "autonomous_saves",
      "autonomous_searches",
      "tokens_consumed",
      "kb_size_at_start",
      "kb_size_at_end",
    ];

    for (const key of scoreKeys) {
      expect(typeof row.scores[key]).toBe("number");
    }
  });

  it("metadata contains experiment tracking fields", () => {
    const row = makeSampleRow();
    expect(typeof row.metadata.experiment).toBe("string");
    expect(typeof row.metadata.condition).toBe("string");
    expect(typeof row.metadata.task_index).toBe("number");
    expect(typeof row.metadata.sequence_id).toBe("string");
    expect(typeof row.metadata.cli).toBe("string");
    expect(typeof row.metadata.latency_ms).toBe("number");
    expect(typeof row.metadata.timestamp).toBe("string");
  });

  it("survives JSON round-trip", () => {
    const row = makeSampleRow();
    const parsed = JSON.parse(JSON.stringify(row));
    expect(parsed).toEqual(row);
  });

  it("id is a composite of sequence/condition/task/timestamp", () => {
    const row = makeSampleRow();
    expect(row.id).toContain("express-api-buildup");
    expect(row.id).toContain("full-hybrid");
    expect(row.id).toContain("init-project");
    expect(row.id).toContain(TIMESTAMP);
  });
});

// ---------------------------------------------------------------------------
// Braintrust compatibility — long-run rows should also be importable
// ---------------------------------------------------------------------------

describe("LongRunEvalRow platform compatibility", () => {
  const TIMESTAMP = "2026-04-01T12:00:00.000Z";

  const row: LongRunEvalRow = {
    id: `express-api-buildup/full-hybrid/add-payments/${TIMESTAMP}`,
    input: {
      task: "Create a /payments endpoint",
      task_id: "add-payments-endpoint",
      task_index: 4,
      sequence_id: "express-api-buildup",
      kb_size: 3,
      mode: "full-hybrid",
    },
    output: {
      commands: [
        'rememora search "payment" --project express-api',
        'rememora save "payments endpoint created" --category entity --project express-api',
      ],
      saves: ['rememora save "payments endpoint created" --category entity --project express-api'],
      searches: ['rememora search "payment" --project express-api'],
    },
    expected: {
      ground_truth: "Agent should search and retrieve prior decisions.",
      depends_on: ["init-project", "add-auth-middleware", "fix-auth-bug"],
    },
    scores: {
      task_completion: 1,
      task_quality: 0,
      autonomous_saves: 1,
      autonomous_searches: 1,
      tokens_consumed: 0,
      kb_size_at_start: 3,
      kb_size_at_end: 4,
    },
    metadata: {
      experiment: "express-api-buildup_full-hybrid",
      condition: "full-hybrid",
      task_index: 4,
      sequence_id: "express-api-buildup",
      cli: "claude-code",
      latency_ms: 8000,
      timestamp: TIMESTAMP,
    },
  };

  it("Braintrust: has id, input, output, expected, scores, metadata", () => {
    expect(Object.keys(row)).toEqual(
      expect.arrayContaining(["id", "input", "output", "expected", "scores", "metadata"]),
    );
  });

  it("scores is Record<string, number>", () => {
    for (const [key, value] of Object.entries(row.scores)) {
      expect(typeof key).toBe("string");
      expect(typeof value).toBe("number");
    }
  });

  it("metadata.experiment identifies the run", () => {
    expect(row.metadata.experiment).toBe("express-api-buildup_full-hybrid");
    expect(row.metadata.condition).toBe("full-hybrid");
    expect(row.metadata.task_index).toBe(4);
  });
});
