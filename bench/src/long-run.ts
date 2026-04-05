import { existsSync, mkdirSync, readFileSync, writeFileSync, unlinkSync, rmSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";

import type { CliRunner, RunResult } from "./runners/types.js";
import { commandsToToolCalls } from "./runners/types.js";
import { ClaudeCodeRunner } from "./runners/claude-code.js";
import { CodexRunner } from "./runners/codex.js";
import { ClaudeTmuxRunner } from "./runners/claude-tmux.js";
import type {
  TaskSequence,
  ExperimentCondition,
  LongRunEvalRow,
  LongRunScores,
} from "./task-sequence.js";
import { extractRemoraCalls } from "./behavioral-logger.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = join(__dirname, "..", "results");
const INSTRUCTIONS_DIR = join(__dirname, "..", "instructions");

// ---------------------------------------------------------------------------
// Runner registry
// ---------------------------------------------------------------------------

const ALL_RUNNERS: CliRunner[] = [
  new ClaudeCodeRunner(),
  new ClaudeTmuxRunner(),
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
// Instruction text loader
// ---------------------------------------------------------------------------

/**
 * Load instruction text for a given instruction mode.
 *
 * Resolves `<mode>.txt` from `bench/instructions/`. Returns an empty string
 * for the "none" mode or if no file is found.
 */
export function loadInstructionText(mode: string): string {
  const filePath = join(INSTRUCTIONS_DIR, `${mode}.txt`);
  try {
    return readFileSync(filePath, "utf-8");
  } catch {
    // No instruction file — treat as empty (equivalent to "none" mode)
    return "";
  }
}

// ---------------------------------------------------------------------------
// Project fixture — creates a minimal project for the agent to work in
// ---------------------------------------------------------------------------

/** Create a temporary project directory with basic structure so the agent has a real codebase. */
function createProjectFixture(sequenceProject: string): string {
  const dir = join(tmpdir(), `rememora-eval-${sequenceProject}-${randomBytes(4).toString("hex")}`);
  mkdirSync(join(dir, "src", "routes"), { recursive: true });
  mkdirSync(join(dir, "src", "middleware"), { recursive: true });
  mkdirSync(join(dir, "src", "models"), { recursive: true });
  mkdirSync(join(dir, "tests"), { recursive: true });

  writeFileSync(join(dir, "package.json"), JSON.stringify({
    name: sequenceProject,
    version: "1.0.0",
    scripts: { start: "tsx src/index.ts", test: "vitest run" },
    dependencies: { express: "^4.18.0", "@prisma/client": "^5.0.0" },
    devDependencies: { typescript: "^5.0.0", tsx: "^4.0.0", vitest: "^1.0.0", "@types/express": "^4.17.0" },
  }, null, 2));

  writeFileSync(join(dir, "tsconfig.json"), JSON.stringify({
    compilerOptions: { target: "ES2022", module: "Node16", outDir: "dist", strict: true },
    include: ["src/**/*.ts"],
  }, null, 2));

  writeFileSync(join(dir, "src", "index.ts"),
`import express from "express";

const app = express();
app.use(express.json());

// Routes will be added here

const PORT = process.env.PORT || 3000;
app.listen(PORT, () => console.log(\`Server on port \${PORT}\`));

export default app;
`);

  // Init git so the agent has a working repo context
  try {
    execFileSync("git", ["init"], { cwd: dir, timeout: 5_000, stdio: "ignore" });
    execFileSync("git", ["add", "."], { cwd: dir, timeout: 5_000, stdio: "ignore" });
    execFileSync("git", ["commit", "-m", "Initial project setup", "--no-gpg-sign"], { cwd: dir, timeout: 5_000, stdio: "ignore" });
  } catch {
    // git init is optional — some environments may not have git
  }

  return dir;
}

/** Clean up a project fixture directory. */
function cleanupFixture(dir: string): void {
  try {
    rmSync(dir, { recursive: true, force: true });
  } catch { /* already gone */ }
}

// ---------------------------------------------------------------------------
// DB helpers — query rememora for KB stats (DB as ground truth)
// ---------------------------------------------------------------------------

/** Shared env builder for rememora subprocess calls. */
function rememoraEnv(dbPath: string): NodeJS.ProcessEnv {
  return { ...process.env, REMEMORA_DB: dbPath };
}

/** Count memory entries in a rememora DB by shelling out to rememora status. */
function getKbSize(dbPath: string): number {
  try {
    const out = execFileSync("rememora", ["status", "--json"], {
      env: rememoraEnv(dbPath),
      timeout: 5_000,
      encoding: "utf-8",
    });
    const data = JSON.parse(out);
    return (data.memories as number) ?? 0;
  } catch {
    return 0;
  }
}

/** A context record as returned by `rememora export --json`. */
interface DbContext {
  id: string;
  uri: string;
  context_type: string;
  category: string | null;
  name: string;
  abstract: string;
  importance: number;
  created_at: string;
  source_agent: string | null;
}

/**
 * Export all contexts from the DB, optionally filtered to those created
 * after a given ISO timestamp. Returns memory-type contexts only.
 */
function getDbContexts(dbPath: string): DbContext[] {
  try {
    const out = execFileSync("rememora", ["export", "--json"], {
      env: rememoraEnv(dbPath),
      timeout: 10_000,
      encoding: "utf-8",
      maxBuffer: 5 * 1024 * 1024,
    });
    const all = JSON.parse(out) as DbContext[];
    return all.filter((c) => c.context_type === "memory");
  } catch {
    return [];
  }
}

/**
 * Find contexts created after a given ISO timestamp.
 * This is the primary mechanism for detecting saves — DB is ground truth.
 */
function getNewContextsSince(dbPath: string, since: string): DbContext[] {
  const all = getDbContexts(dbPath);
  return all.filter((c) => c.created_at > since);
}

/** Get category breakdown from DB. */
function getDbCategories(dbPath: string): Record<string, number> {
  try {
    const out = execFileSync("rememora", ["status", "--json"], {
      env: rememoraEnv(dbPath),
      timeout: 5_000,
      encoding: "utf-8",
    });
    const data = JSON.parse(out);
    return (data.categories as Record<string, number>) ?? {};
  } catch {
    return {};
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

  // Load instruction text for this condition's mode
  const instructionText = loadInstructionText(condition.instructionMode);

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

  // Create a project fixture so the agent has a real codebase to work in
  const projectDir = createProjectFixture(sequence.project);

  const log = options.quiet ? () => {} : console.log.bind(console);

  log(`\n  Rememora Long-Run Eval`);
  log(`  Sequence: ${sequence.id} (${sequence.tasks.length} tasks)`);
  log(`  Condition: ${condition.id} (mode: ${condition.instructionMode})`);
  log(`  CLI: ${runner.name}`);
  log(`  DB: ${dbPath} (persistent across tasks)`);
  log(`  Project: ${projectDir}`);
  log("─".repeat(60));

  const rows: LongRunEvalRow[] = [];
  let totalSaves = 0;
  let totalSearches = 0;
  let autonomousSaves = 0;
  let autonomousSearches = 0;
  let totalLatencyMs = 0;

  const kbSizeStart = getKbSize(dbPath);

  // Build shared options for all tasks
  const env: Record<string, string> = { REMEMORA_DB: dbPath };
  const runOptions: import("./runners/types.js").RunOptions = {
    cwd: projectDir,
    env,
    timeoutMs,
    instructionText,
  };

  // For tmux runner: start a persistent session and reuse it across tasks.
  // This gives the model session continuity — the key difference from -p mode.
  const isTmux = runner instanceof ClaudeTmuxRunner;
  const tmuxRunner = isTmux ? (runner as ClaudeTmuxRunner) : null;
  let tmuxSession: string | null = null;

  if (tmuxRunner) {
    log(`  Starting persistent Claude session via tmux...`);
    tmuxSession = tmuxRunner.startSession(runOptions);
    log(`  tmux session: ${tmuxSession}`);
  }

  for (let i = 0; i < sequence.tasks.length; i++) {
    const task = sequence.tasks[i];
    const kbSizeAtStart = getKbSize(dbPath);
    const taskStartTime = new Date().toISOString();

    log(`\n  Task ${i + 1}/${sequence.tasks.length}: ${task.id}`);
    log(`  ${task.description}`);

    try {
      // Tmux: send prompt to the existing session
      // Non-tmux: spawn a new process per task (original behavior)
      const runResult = tmuxRunner && tmuxSession
        ? await (async () => {
            const { output, latencyMs } = await tmuxRunner.sendPrompt(
              tmuxSession!,
              task.userMessage,
              timeoutMs,
            );
            // Parse rememora commands from terminal output
            const { parseTerminalCommands } = await import("./runners/claude-tmux.js");
            const commands = parseTerminalCommands(output);
            return {
              cli: runner.name,
              commands,
              rawOutput: output,
              exitCode: 0,
              latencyMs,
            } satisfies RunResult;
          })()
        : await runner.run(task.userMessage, runOptions);

      const commands = runResult.commands.map((c) => c.command);
      const behavior = extractRemoraCalls(
        runResult.rawOutput,
        commands,
        task.userMessage,
      );

      // DB-based inference: query what was actually saved (ground truth)
      const kbSizeAtEnd = getKbSize(dbPath);
      const newContexts = getNewContextsSince(dbPath, taskStartTime);
      const dbSaves = newContexts.length;
      const dbCategories: Record<string, number> = {};
      for (const ctx of newContexts) {
        const cat = ctx.category ?? "unknown";
        dbCategories[cat] = (dbCategories[cat] ?? 0) + 1;
      }

      const latencyMs = runResult.latencyMs;
      totalLatencyMs += latencyMs;

      // Use DB-inferred saves as primary metric, fall back to parsed commands
      const effectiveSaves = dbSaves > 0 ? dbSaves : behavior.saves.length;

      // Build scores
      const scores: LongRunScores = {
        task_completion: runResult.exitCode === 0 ? 1 : 0,
        task_quality: 0, // Placeholder — requires LLM judge or human review
        autonomous_saves: dbSaves > 0 ? dbSaves : behavior.autonomousSaveCount,
        autonomous_searches: behavior.autonomousSearchCount,
        tokens_consumed: 0, // Placeholder — extract from CLI output if available
        kb_size_at_start: kbSizeAtStart,
        kb_size_at_end: kbSizeAtEnd,
        db_saves: dbSaves,
        db_categories: Object.keys(dbCategories).length > 0 ? dbCategories : undefined,
      };

      totalSaves += effectiveSaves;
      totalSearches += behavior.searches.length;
      autonomousSaves += dbSaves > 0 ? dbSaves : behavior.autonomousSaveCount;
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
          db_new_contexts: newContexts.map((c) => ({
            category: c.category,
            name: c.name,
            abstract: c.abstract,
          })),
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
        effectiveSaves > 0
          ? `\x1b[32m${effectiveSaves} saves\x1b[0m`
          : "0 saves";
      const dbIcon = dbSaves > 0
        ? ` \x1b[33m(${dbSaves} in DB)\x1b[0m`
        : "";
      const searchIcon =
        behavior.searches.length > 0
          ? `\x1b[36m${behavior.searches.length} searches\x1b[0m`
          : "0 searches";

      log(
        `  Result: ${saveIcon}${dbIcon}, ${searchIcon}, KB: ${kbSizeAtStart} -> ${kbSizeAtEnd}`,
      );
    } catch (err) {
      log(`  \x1b[31mERROR\x1b[0m  ${task.id}: ${err}`);

      const kbSizeAtEnd = getKbSize(dbPath);
      const newContexts = getNewContextsSince(dbPath, taskStartTime);
      const dbSaves = newContexts.length;

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
          db_new_contexts: newContexts.map((c) => ({
            category: c.category,
            name: c.name,
            abstract: c.abstract,
          })),
        },
        expected: {
          ground_truth: task.groundTruth,
          depends_on: task.dependsOn,
        },
        scores: {
          task_completion: 0,
          task_quality: 0,
          autonomous_saves: dbSaves,
          autonomous_searches: 0,
          tokens_consumed: 0,
          kb_size_at_start: kbSizeAtStart,
          kb_size_at_end: kbSizeAtEnd,
          db_saves: dbSaves,
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

  // Clean up tmux session
  if (tmuxRunner && tmuxSession) {
    log(`\n  Ending tmux session: ${tmuxSession}`);
    tmuxRunner.endSession(tmuxSession);
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

  // Clean up project fixture
  if (!options.keepDb) {
    cleanupFixture(projectDir);
  }

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
