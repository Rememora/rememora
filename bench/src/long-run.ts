import { existsSync, mkdirSync, writeFileSync, unlinkSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";

import type { CliRunner } from "./runners/types.js";
import { commandsToToolCalls } from "./runners/types.js";
import { ClaudeCodeRunner } from "./runners/claude-code.js";
import { CodexRunner } from "./runners/codex.js";
import type {
  TaskSequence,
  ExperimentCondition,
  LongRunEvalRow,
  LongRunScores,
} from "./task-sequence.js";
import { extractRemoraCalls } from "./behavioral-logger.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = join(__dirname, "..", "results");

// ---------------------------------------------------------------------------
// Runner registry (mirrors run.ts)
// ---------------------------------------------------------------------------

const ALL_RUNNERS: CliRunner[] = [
  new ClaudeCodeRunner(),
  new CodexRunner(),
];

function getRunner(name: string): CliRunner {
  const runner = ALL_RUNNERS.find((r) => r.name === name);
  if (!runner) {
    const names = ALL_RUNNERS.map((r) => r.name).join(", ");
    throw new Error(`Unknown CLI: ${name}. Must be one of: ${names}`);
  }
  return runner;
}

// ---------------------------------------------------------------------------
// DB helpers — query rememora for KB stats
// ---------------------------------------------------------------------------

/** Count entries in a rememora DB by shelling out to rememora status. */
function getKbSize(dbPath: string): number {
  try {
    const out = execFileSync("rememora", ["status", "--json"], {
      env: { ...process.env, REMEMORA_DB: dbPath },
      timeout: 5_000,
      encoding: "utf-8",
    });
    const data = JSON.parse(out);
    return (data.contexts_count as number) ?? 0;
  } catch {
    // If rememora isn't available or status fails, fall back to 0
    return 0;
  }
}

// ---------------------------------------------------------------------------
// Core: run a task sequence under a given condition
// ---------------------------------------------------------------------------

export interface LongRunOptions {
  /** Timeout per task in ms. */
  timeoutMs?: number;
  /** If true, keep the DB after the run for inspection. */
  keepDb?: boolean;
  /** Custom DB path (default: creates temp DB). */
  dbPath?: string;
  /** If true, suppress console output. */
  quiet?: boolean;
}

/** Result of a full long run. */
export interface LongRunResult {
  sequenceId: string;
  conditionId: string;
  cli: string;
  dbPath: string;
  rows: LongRunEvalRow[];
  summary: {
    tasksCompleted: number;
    totalTasks: number;
    totalSaves: number;
    totalSearches: number;
    autonomousSaves: number;
    autonomousSearches: number;
    totalLatencyMs: number;
    kbSizeStart: number;
    kbSizeEnd: number;
  };
}

/**
 * Run a task sequence under a given experiment condition.
 *
 * The key difference from single-scenario eval: the rememora DB persists
 * across all tasks in the sequence, allowing knowledge to accumulate.
 */
export async function runLongRun(
  sequence: TaskSequence,
  condition: ExperimentCondition,
  options: LongRunOptions = {},
): Promise<LongRunResult> {
  const timeoutMs = options.timeoutMs ?? 120_000;
  const runner = getRunner(condition.agent);
  const timestamp = new Date().toISOString();

  // Check CLI availability
  const isAvailable = await runner.available();
  if (!isAvailable) {
    throw new Error(`CLI '${runner.name}' not found on PATH. Is it installed?`);
  }

  // Create a persistent DB for the entire sequence
  const dbPath =
    options.dbPath ??
    join(
      tmpdir(),
      `rememora-longrun-${sequence.id}-${randomBytes(4).toString("hex")}.db`,
    );

  const log = options.quiet ? () => {} : console.log.bind(console);

  log(`\n  Rememora Long-Run Eval`);
  log(`  Sequence: ${sequence.id} (${sequence.tasks.length} tasks)`);
  log(`  Condition: ${condition.id} (mode: ${condition.instructionMode})`);
  log(`  CLI: ${runner.name}`);
  log(`  DB: ${dbPath} (persistent across tasks)`);
  log("─".repeat(60));

  const rows: LongRunEvalRow[] = [];
  let totalSaves = 0;
  let totalSearches = 0;
  let autonomousSaves = 0;
  let autonomousSearches = 0;
  let totalLatencyMs = 0;

  const kbSizeStart = getKbSize(dbPath);

  for (let i = 0; i < sequence.tasks.length; i++) {
    const task = sequence.tasks[i];
    const kbSizeAtStart = getKbSize(dbPath);

    log(`\n  Task ${i + 1}/${sequence.tasks.length}: ${task.id}`);
    log(`  ${task.description}`);

    // Build environment — the persistent DB is the critical piece
    const env: Record<string, string> = {
      REMEMORA_DB: dbPath,
    };

    // For "none" condition, we could unset REMEMORA_DB, but it's cleaner
    // to just not include rememora instructions. The agent may or may not
    // use rememora depending on its own configuration.

    try {
      const runResult = await runner.run(task.userMessage, {
        cwd: tmpdir(),
        env,
        timeoutMs,
      });

      const commands = runResult.commands.map((c) => c.command);
      const behavior = extractRemoraCalls(
        runResult.rawOutput,
        commands,
        task.userMessage,
      );

      const kbSizeAtEnd = getKbSize(dbPath);
      const latencyMs = runResult.latencyMs;
      totalLatencyMs += latencyMs;

      // Build scores
      const scores: LongRunScores = {
        task_completion: runResult.exitCode === 0 ? 1 : 0,
        task_quality: 0, // Placeholder — requires LLM judge or human review
        autonomous_saves: behavior.autonomousSaveCount,
        autonomous_searches: behavior.autonomousSearchCount,
        tokens_consumed: 0, // Placeholder — extract from CLI output if available
        kb_size_at_start: kbSizeAtStart,
        kb_size_at_end: kbSizeAtEnd,
      };

      totalSaves += behavior.saves.length;
      totalSearches += behavior.searches.length;
      autonomousSaves += behavior.autonomousSaveCount;
      autonomousSearches += behavior.autonomousSearchCount;

      const row: LongRunEvalRow = {
        id: `${sequence.id}/${condition.id}/${task.id}/${timestamp}`,
        input: {
          task: task.description,
          task_id: task.id,
          task_index: i,
          sequence_id: sequence.id,
          kb_size: kbSizeAtStart,
          mode: condition.instructionMode,
        },
        output: {
          commands,
          saves: behavior.saves.map((s) => s.fullCommand),
          searches: behavior.searches.map((s) => s.fullCommand),
        },
        expected: {
          ground_truth: task.groundTruth,
          depends_on: task.dependsOn,
        },
        scores,
        metadata: {
          experiment: `${sequence.id}_${condition.id}`,
          condition: condition.id,
          task_index: i,
          sequence_id: sequence.id,
          cli: runner.name,
          latency_ms: Math.round(latencyMs),
          timestamp,
        },
      };

      rows.push(row);

      const saveIcon =
        behavior.saves.length > 0
          ? `\x1b[32m${behavior.saves.length} saves\x1b[0m`
          : "0 saves";
      const searchIcon =
        behavior.searches.length > 0
          ? `\x1b[36m${behavior.searches.length} searches\x1b[0m`
          : "0 searches";

      log(
        `  Result: ${saveIcon}, ${searchIcon}, KB: ${kbSizeAtStart} -> ${kbSizeAtEnd}`,
      );
    } catch (err) {
      log(`  \x1b[31mERROR\x1b[0m  ${task.id}: ${err}`);

      const kbSizeAtEnd = getKbSize(dbPath);

      rows.push({
        id: `${sequence.id}/${condition.id}/${task.id}/${timestamp}`,
        input: {
          task: task.description,
          task_id: task.id,
          task_index: i,
          sequence_id: sequence.id,
          kb_size: kbSizeAtStart,
          mode: condition.instructionMode,
        },
        output: {
          commands: [],
          saves: [],
          searches: [],
        },
        expected: {
          ground_truth: task.groundTruth,
          depends_on: task.dependsOn,
        },
        scores: {
          task_completion: 0,
          task_quality: 0,
          autonomous_saves: 0,
          autonomous_searches: 0,
          tokens_consumed: 0,
          kb_size_at_start: kbSizeAtStart,
          kb_size_at_end: kbSizeAtEnd,
        },
        metadata: {
          experiment: `${sequence.id}_${condition.id}`,
          condition: condition.id,
          task_index: i,
          sequence_id: sequence.id,
          cli: runner.name,
          latency_ms: 0,
          timestamp,
        },
      });
    }
  }

  const kbSizeEnd = getKbSize(dbPath);

  // Print summary
  log("\n" + "─".repeat(60));
  log(`  Long-run complete: ${sequence.id} x ${condition.id}`);
  log(
    `  Tasks: ${rows.filter((r) => r.scores.task_completion > 0).length}/${sequence.tasks.length} completed`,
  );
  log(`  Saves: ${totalSaves} (${autonomousSaves} autonomous)`);
  log(`  Searches: ${totalSearches} (${autonomousSearches} autonomous)`);
  log(`  KB growth: ${kbSizeStart} -> ${kbSizeEnd} entries`);
  log(`  Total latency: ${Math.round(totalLatencyMs)}ms`);
  log("─".repeat(60));

  // Write JSONL results
  if (!existsSync(RESULTS_DIR)) {
    mkdirSync(RESULTS_DIR, { recursive: true });
  }

  const ts = new Date().toISOString().replace(/[:.]/g, "-");
  const jsonlPath = join(
    RESULTS_DIR,
    `longrun_${sequence.id}_${condition.id}_${ts}.jsonl`,
  );
  const jsonlContent =
    rows.map((row) => JSON.stringify(row)).join("\n") + "\n";
  writeFileSync(jsonlPath, jsonlContent);

  log(`\n  JSONL: ${jsonlPath}`);

  // Clean up DB unless requested to keep
  if (!options.keepDb && !options.dbPath) {
    try {
      unlinkSync(dbPath);
    } catch {
      /* already gone */
    }
    try {
      unlinkSync(`${dbPath}-wal`);
    } catch {
      /* WAL */
    }
    try {
      unlinkSync(`${dbPath}-shm`);
    } catch {
      /* SHM */
    }
  }

  return {
    sequenceId: sequence.id,
    conditionId: condition.id,
    cli: runner.name,
    dbPath,
    rows,
    summary: {
      tasksCompleted: rows.filter((r) => r.scores.task_completion > 0).length,
      totalTasks: sequence.tasks.length,
      totalSaves,
      totalSearches,
      autonomousSaves,
      autonomousSearches,
      totalLatencyMs,
      kbSizeStart,
      kbSizeEnd,
    },
  };
}

// ---------------------------------------------------------------------------
// Matrix runner — run same sequence across multiple conditions
// ---------------------------------------------------------------------------

export interface MatrixResult {
  sequenceId: string;
  results: LongRunResult[];
}

/**
 * Run a task sequence across all given conditions.
 *
 * Each condition gets its own fresh DB so results are independent.
 */
export async function runMatrix(
  sequence: TaskSequence,
  conditions: ExperimentCondition[],
  options: LongRunOptions = {},
): Promise<MatrixResult> {
  const results: LongRunResult[] = [];

  console.log(`\n  Rememora Eval Matrix`);
  console.log(`  Sequence: ${sequence.id} (${sequence.tasks.length} tasks)`);
  console.log(`  Conditions: ${conditions.length}`);
  console.log("═".repeat(60));

  for (const condition of conditions) {
    // Each condition gets its own DB
    const result = await runLongRun(sequence, condition, {
      ...options,
      dbPath: undefined, // Force fresh DB per condition
    });
    results.push(result);
  }

  // Print comparison matrix
  console.log("\n" + "═".repeat(60));
  console.log("  Matrix Comparison");
  console.log("─".repeat(60));
  console.log(
    `  ${"Condition".padEnd(20)} ${"Completed".padEnd(12)} ${"Saves".padEnd(10)} ${"Searches".padEnd(10)} ${"KB Growth".padEnd(12)} Latency`,
  );
  console.log("─".repeat(60));

  for (const r of results) {
    const completed = `${r.summary.tasksCompleted}/${r.summary.totalTasks}`;
    const saves = `${r.summary.totalSaves} (${r.summary.autonomousSaves}a)`;
    const searches = `${r.summary.totalSearches} (${r.summary.autonomousSearches}a)`;
    const growth = `${r.summary.kbSizeStart}->${r.summary.kbSizeEnd}`;
    const latency = `${Math.round(r.summary.totalLatencyMs)}ms`;

    console.log(
      `  ${r.conditionId.padEnd(20)} ${completed.padEnd(12)} ${saves.padEnd(10)} ${searches.padEnd(10)} ${growth.padEnd(12)} ${latency}`,
    );
  }

  console.log("═".repeat(60));

  return { sequenceId: sequence.id, results };
}
