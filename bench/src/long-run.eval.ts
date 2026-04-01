import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { randomBytes } from "node:crypto";

import { loadTaskSequence, loadCondition } from "./task-sequence.js";
import type {
  TaskSequence,
  ExperimentCondition,
  LongRunEvalRow,
  LongRunScores,
} from "./task-sequence.js";
import { extractRemoraCalls } from "./behavioral-logger.js";
import type { BehaviorSummary } from "./behavioral-logger.js";
import { loadInstructionText } from "./long-run.js";
import { compareConditions } from "./compare-conditions.js";
import type { ComparisonReport, ConditionSummary } from "./compare-conditions.js";

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
    expect(cond.preIndexed).toBe(false);
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

// ---------------------------------------------------------------------------
// Instruction-mode-eval task sequence
// ---------------------------------------------------------------------------

describe("instruction-mode-eval task sequence", () => {
  const seq = loadTaskSequence(
    join(BENCH_DIR, "tasks/instruction-mode-eval.json"),
  );

  it("loads with correct id and project", () => {
    expect(seq.id).toBe("instruction-mode-eval");
    expect(seq.project).toBe("express-api");
  });

  it("has 8 tasks", () => {
    expect(seq.tasks).toHaveLength(8);
  });

  it("every task has id, description, userMessage, and groundTruth", () => {
    for (const task of seq.tasks) {
      expect(task.id).toBeTruthy();
      expect(task.description).toBeTruthy();
      expect(task.userMessage).toBeTruthy();
      expect(task.groundTruth).toBeTruthy();
    }
  });

  it("no task userMessage mentions rememora, save to memory, or search memory", () => {
    const forbidden = [
      /\brememora\b/i,
      /\bsave\s+(this\s+)?to\s+memory\b/i,
      /\bsearch\s+memory\b/i,
      /\brecall\b/i,
      /\blook\s*up\s+in\s+memory\b/i,
      /\bremember\s+this\b/i,
    ];
    for (const task of seq.tasks) {
      for (const pattern of forbidden) {
        expect(
          pattern.test(task.userMessage),
          `Task "${task.id}" userMessage matches forbidden pattern ${pattern}: "${task.userMessage.slice(0, 80)}..."`,
        ).toBe(false);
      }
    }
  });

  it("later tasks have dependsOn referencing earlier task IDs", () => {
    const taskIds = new Set(seq.tasks.map((t) => t.id));
    for (const task of seq.tasks) {
      if (task.dependsOn) {
        for (const dep of task.dependsOn) {
          expect(
            taskIds.has(dep),
            `Task "${task.id}" depends on unknown task "${dep}"`,
          ).toBe(true);
        }
      }
    }
  });

  it("first task has no dependencies", () => {
    expect(seq.tasks[0].dependsOn).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// New condition configs
// ---------------------------------------------------------------------------

describe("new condition configs", () => {
  it("loads behavioral-triggers.json", () => {
    const cond = loadCondition(
      join(BENCH_DIR, "conditions/behavioral-triggers.json"),
    );
    expect(cond.id).toBe("behavioral-triggers");
    expect(cond.instructionMode).toBe("behavioral-triggers");
    expect(cond.agent).toBe("claude-code");
    expect(cond.preIndexed).toBe(false);
  });

  it("loads hooks-only.json", () => {
    const cond = loadCondition(
      join(BENCH_DIR, "conditions/hooks-only.json"),
    );
    expect(cond.id).toBe("hooks-only");
    expect(cond.instructionMode).toBe("hooks-only");
    expect(cond.agent).toBe("claude-code");
  });

  it("all 5 conditions load successfully", () => {
    const modes = [
      "none",
      "reference-card",
      "behavioral-triggers",
      "hooks-only",
      "full-hybrid",
    ];
    for (const mode of modes) {
      const cond = loadCondition(
        join(BENCH_DIR, `conditions/${mode}.json`),
      );
      expect(cond.instructionMode).toBe(mode);
      expect(cond.agent).toBe("claude-code");
    }
  });
});

// ---------------------------------------------------------------------------
// Instruction text loader
// ---------------------------------------------------------------------------

describe("loadInstructionText", () => {
  it("returns empty string for 'none' mode", () => {
    const text = loadInstructionText("none");
    expect(text.trim()).toBe("");
  });

  it("returns non-empty text for 'reference-card' mode", () => {
    const text = loadInstructionText("reference-card");
    expect(text.length).toBeGreaterThan(100);
    expect(text).toContain("Rememora");
    expect(text).toContain("On session start");
  });

  it("returns non-empty text for 'behavioral-triggers' mode", () => {
    const text = loadInstructionText("behavioral-triggers");
    expect(text.length).toBeGreaterThan(100);
    expect(text).toContain("When to SEARCH");
    expect(text).toContain("When to SAVE");
  });

  it("returns non-empty text for 'hooks-only' mode", () => {
    const text = loadInstructionText("hooks-only");
    expect(text.length).toBeGreaterThan(50);
    expect(text).toContain("gone forever");
    // hooks-only should be shorter than behavioral-triggers
    const btText = loadInstructionText("behavioral-triggers");
    expect(text.length).toBeLessThan(btText.length);
  });

  it("returns non-empty text for 'full-hybrid' mode", () => {
    const text = loadInstructionText("full-hybrid");
    expect(text.length).toBeGreaterThan(100);
    expect(text).toContain("CRITICAL");
    expect(text).toContain("When to SEARCH");
    expect(text).toContain("When to SAVE");
    // full-hybrid should be the longest
    const btText = loadInstructionText("behavioral-triggers");
    expect(text.length).toBeGreaterThan(btText.length);
  });

  it("returns empty string for unknown mode", () => {
    const text = loadInstructionText("nonexistent-mode");
    expect(text).toBe("");
  });
});

// ---------------------------------------------------------------------------
// Comparison report
// ---------------------------------------------------------------------------

describe("compareConditions", () => {
  const tmpDir = join(
    tmpdir(),
    `rememora-compare-test-${randomBytes(4).toString("hex")}`,
  );

  const TS1 = "2026-04-01T12:00:00.000Z";
  const TS2 = "2026-04-01T13:00:00.000Z";

  function makeRow(
    condition: string,
    taskId: string,
    taskIndex: number,
    autonomousSaves: number,
    autonomousSearches: number,
    kbStart: number,
    kbEnd: number,
    timestamp: string,
  ): LongRunEvalRow {
    return {
      id: `test-seq/${condition}/${taskId}/${timestamp}`,
      input: {
        task: `Task ${taskId}`,
        task_id: taskId,
        task_index: taskIndex,
        sequence_id: "test-seq",
        kb_size: kbStart,
        mode: condition as LongRunEvalRow["input"]["mode"],
      },
      output: {
        commands: [],
        saves: Array(autonomousSaves).fill("rememora save ..."),
        searches: Array(autonomousSearches).fill("rememora search ..."),
      },
      expected: { ground_truth: "test" },
      scores: {
        task_completion: 1,
        task_quality: 0,
        autonomous_saves: autonomousSaves,
        autonomous_searches: autonomousSearches,
        tokens_consumed: 0,
        kb_size_at_start: kbStart,
        kb_size_at_end: kbEnd,
      },
      metadata: {
        experiment: `test-seq_${condition}`,
        condition,
        task_index: taskIndex,
        sequence_id: "test-seq",
        cli: "claude-code",
        latency_ms: 1000,
        timestamp,
      },
    };
  }

  beforeAll(() => {
    mkdirSync(tmpDir, { recursive: true });

    // Condition A: 2 runs, 2 tasks each
    const condARows = [
      makeRow("cond-a", "t1", 0, 2, 1, 0, 2, TS1),
      makeRow("cond-a", "t2", 1, 1, 2, 2, 3, TS1),
      makeRow("cond-a", "t1", 0, 3, 0, 0, 3, TS2),
      makeRow("cond-a", "t2", 1, 0, 1, 3, 3, TS2),
    ];
    writeFileSync(
      join(tmpDir, `longrun_test-seq_cond-a_run1.jsonl`),
      condARows.slice(0, 2).map((r) => JSON.stringify(r)).join("\n") + "\n",
    );
    writeFileSync(
      join(tmpDir, `longrun_test-seq_cond-a_run2.jsonl`),
      condARows.slice(2).map((r) => JSON.stringify(r)).join("\n") + "\n",
    );

    // Condition B: 1 run, 2 tasks
    const condBRows = [
      makeRow("cond-b", "t1", 0, 0, 0, 0, 0, TS1),
      makeRow("cond-b", "t2", 1, 0, 0, 0, 0, TS1),
    ];
    writeFileSync(
      join(tmpDir, `longrun_test-seq_cond-b_run1.jsonl`),
      condBRows.map((r) => JSON.stringify(r)).join("\n") + "\n",
    );
  });

  afterAll(() => {
    rmSync(tmpDir, { recursive: true, force: true });
  });

  it("groups results by condition", () => {
    const report = compareConditions("test-seq", tmpDir);
    expect(report.conditions).toHaveLength(2);
    expect(report.conditions.map((c) => c.condition).sort()).toEqual([
      "cond-a",
      "cond-b",
    ]);
  });

  it("counts runs correctly", () => {
    const report = compareConditions("test-seq", tmpDir);
    const condA = report.conditions.find((c) => c.condition === "cond-a")!;
    const condB = report.conditions.find((c) => c.condition === "cond-b")!;
    expect(condA.runCount).toBe(2);
    expect(condB.runCount).toBe(1);
  });

  it("sums autonomous saves and searches", () => {
    const report = compareConditions("test-seq", tmpDir);
    const condA = report.conditions.find((c) => c.condition === "cond-a")!;
    expect(condA.autonomousSaves).toBe(6);
    expect(condA.autonomousSearches).toBe(4);
  });

  it("calculates task completion rate", () => {
    const report = compareConditions("test-seq", tmpDir);
    const condA = report.conditions.find((c) => c.condition === "cond-a")!;
    expect(condA.taskCompletionRate).toBe(1);
    expect(condA.totalTasks).toBe(4);
    expect(condA.tasksCompleted).toBe(4);
  });

  it("calculates average KB growth per task", () => {
    const report = compareConditions("test-seq", tmpDir);
    const condA = report.conditions.find((c) => c.condition === "cond-a")!;
    expect(condA.avgKbGrowthPerTask).toBe(1.5);

    const condB = report.conditions.find((c) => c.condition === "cond-b")!;
    expect(condB.avgKbGrowthPerTask).toBe(0);
  });

  it("condition with no rememora usage shows zeros", () => {
    const report = compareConditions("test-seq", tmpDir);
    const condB = report.conditions.find((c) => c.condition === "cond-b")!;
    expect(condB.autonomousSaves).toBe(0);
    expect(condB.autonomousSearches).toBe(0);
    expect(condB.promptedSaves).toBe(0);
    expect(condB.promptedSearches).toBe(0);
  });

  it("throws on missing sequence", () => {
    expect(() => compareConditions("nonexistent-seq", tmpDir)).toThrow(
      /No result files found/,
    );
  });

  it("report has generatedAt timestamp", () => {
    const report = compareConditions("test-seq", tmpDir);
    expect(report.generatedAt).toBeTruthy();
    expect(() => new Date(report.generatedAt)).not.toThrow();
  });
});
