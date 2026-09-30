import { readdirSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

import type { LongRunEvalRow } from "./task-sequence.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = join(__dirname, "..", "results");

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Per-condition aggregate metrics. */
export interface ConditionSummary {
  condition: string;
  runCount: number;
  totalTasks: number;
  tasksCompleted: number;
  autonomousSaves: number;
  autonomousSearches: number;
  promptedSaves: number;
  promptedSearches: number;
  avgKbGrowthPerTask: number;
  taskCompletionRate: number;
  /** Average task quality score from LLM judge (0-1). */
  avgTaskQuality: number;
  /** Total DB-inferred saves (ground truth, when available). */
  dbSaves: number;
}

/** Full comparison result across conditions. */
export interface ComparisonReport {
  sequencePattern: string;
  conditions: ConditionSummary[];
  generatedAt: string;
}

// ---------------------------------------------------------------------------
// JSONL reader
// ---------------------------------------------------------------------------

/** Read all rows from a JSONL file. */
function readJsonlFile(path: string): LongRunEvalRow[] {
  const content = readFileSync(path, "utf-8");
  return content
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line) as LongRunEvalRow);
}

// ---------------------------------------------------------------------------
// Core comparison logic
// ---------------------------------------------------------------------------

/**
 * Build a comparison report from JSONL result files matching a pattern.
 *
 * @param sequenceId - Match files like `longrun_<sequenceId>_*.jsonl`
 * @param resultsDir - Directory containing JSONL files (defaults to bench/results/)
 */
export function compareConditions(
  sequenceId: string,
  resultsDir: string = RESULTS_DIR,
): ComparisonReport {
  const prefix = `longrun_${sequenceId}_`;

  const files = readdirSync(resultsDir)
    .filter((f) => f.startsWith(prefix) && f.endsWith(".jsonl"))
    .sort();

  if (files.length === 0) {
    throw new Error(
      `No result files found matching '${prefix}*.jsonl' in ${resultsDir}`,
    );
  }

  // Group all rows by condition
  const byCondition = new Map<string, LongRunEvalRow[]>();

  for (const file of files) {
    const rows = readJsonlFile(join(resultsDir, file));
    for (const row of rows) {
      const cond = row.metadata.condition;
      const existing = byCondition.get(cond) ?? [];
      existing.push(row);
      byCondition.set(cond, existing);
    }
  }

  // Build per-condition summaries
  const conditions: ConditionSummary[] = [];

  for (const [condition, rows] of byCondition) {
    const totalTasks = rows.length;
    const tasksCompleted = rows.filter(
      (r) => r.scores.task_completion > 0,
    ).length;

    const autonomousSaves = rows.reduce(
      (sum, r) => sum + r.scores.autonomous_saves,
      0,
    );
    const autonomousSearches = rows.reduce(
      (sum, r) => sum + r.scores.autonomous_searches,
      0,
    );

    // Total saves/searches = length of output arrays
    const totalSaves = rows.reduce(
      (sum, r) => sum + r.output.saves.length,
      0,
    );
    const totalSearches = rows.reduce(
      (sum, r) => sum + r.output.searches.length,
      0,
    );

    const promptedSaves = totalSaves - autonomousSaves;
    const promptedSearches = totalSearches - autonomousSearches;

    // KB growth per task: average of (kb_end - kb_start) across tasks
    const kbGrowths = rows.map(
      (r) => r.scores.kb_size_at_end - r.scores.kb_size_at_start,
    );
    const avgKbGrowthPerTask =
      kbGrowths.length > 0
        ? kbGrowths.reduce((a, b) => a + b, 0) / kbGrowths.length
        : 0;

    // DB-inferred saves (ground truth when available)
    const dbSaves = rows.reduce(
      (sum, r) => sum + (r.scores.db_saves ?? 0),
      0,
    );

    // Average task quality from LLM judge
    const avgTaskQuality =
      totalTasks > 0
        ? rows.reduce((sum, r) => sum + r.scores.task_quality, 0) / totalTasks
        : 0;

    // Determine run count from unique timestamps
    const uniqueTimestamps = new Set(rows.map((r) => r.metadata.timestamp));

    conditions.push({
      condition,
      runCount: uniqueTimestamps.size,
      totalTasks,
      tasksCompleted,
      autonomousSaves,
      autonomousSearches,
      promptedSaves,
      promptedSearches,
      avgKbGrowthPerTask: Math.round(avgKbGrowthPerTask * 100) / 100,
      taskCompletionRate:
        totalTasks > 0
          ? Math.round((tasksCompleted / totalTasks) * 100) / 100
          : 0,
      avgTaskQuality: Math.round(avgTaskQuality * 100) / 100,
      dbSaves,
    });
  }

  // Sort by condition name for stable output
  conditions.sort((a, b) => a.condition.localeCompare(b.condition));

  return {
    sequencePattern: sequenceId,
    conditions,
    generatedAt: new Date().toISOString(),
  };
}

// ---------------------------------------------------------------------------
// Pretty-print
// ---------------------------------------------------------------------------

/** Print a comparison report as a formatted table to the console. */
export function printComparisonReport(report: ComparisonReport): void {
  console.log("\n" + "=".repeat(100));
  console.log(`  Instruction Mode Comparison — ${report.sequencePattern}`);
  console.log(`  Generated: ${report.generatedAt}`);
  console.log("=".repeat(100));

  // Header
  const hdr = [
    "Condition".padEnd(22),
    "Runs".padEnd(6),
    "Done".padEnd(8),
    "Rate".padEnd(8),
    "Quality".padEnd(9),
    "A-Saves".padEnd(10),
    "A-Search".padEnd(10),
    "P-Saves".padEnd(10),
    "P-Search".padEnd(10),
    "KB/task".padEnd(8),
  ].join("");
  console.log(hdr);
  console.log("-".repeat(109));

  for (const c of report.conditions) {
    const row = [
      c.condition.padEnd(22),
      String(c.runCount).padEnd(6),
      `${c.tasksCompleted}/${c.totalTasks}`.padEnd(8),
      `${(c.taskCompletionRate * 100).toFixed(0)}%`.padEnd(8),
      c.avgTaskQuality.toFixed(2).padEnd(9),
      String(c.autonomousSaves).padEnd(10),
      String(c.autonomousSearches).padEnd(10),
      String(c.promptedSaves).padEnd(10),
      String(c.promptedSearches).padEnd(10),
      c.avgKbGrowthPerTask.toFixed(2).padEnd(8),
    ].join("");
    console.log(row);
  }

  console.log("=".repeat(109));
  console.log(
    "  Quality = avg LLM judge score (0-1) | A-Saves = autonomous saves | A-Search = autonomous searches",
  );
  console.log(
    "  P-Saves = prompted saves   | P-Search = prompted searches | KB/task = avg KB entries grown per task",
  );
  console.log("=".repeat(109));
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

async function main(): Promise<void> {
  const sequenceId = process.argv[2];
  const resultsDir = process.argv[3];

  if (!sequenceId) {
    console.error(
      "Usage: tsx src/compare-conditions.ts <sequence-id> [results-dir]",
    );
    console.error(
      "Example: tsx src/compare-conditions.ts instruction-mode-eval",
    );
    process.exit(1);
  }

  const report = compareConditions(sequenceId, resultsDir);
  printComparisonReport(report);

  // Also write JSON report
  const jsonPath = join(
    resultsDir ?? RESULTS_DIR,
    `comparison_${sequenceId}_${new Date().toISOString().replace(/[:.]/g, "-")}.json`,
  );
  const { writeFileSync } = await import("node:fs");
  writeFileSync(jsonPath, JSON.stringify(report, null, 2) + "\n");
  console.log(`\n  JSON report: ${jsonPath}`);
}

// Only run main if this file is the entry point
const isMain =
  process.argv[1]?.endsWith("compare-conditions.ts") ||
  process.argv[1]?.endsWith("compare-conditions.js");
if (isMain) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
