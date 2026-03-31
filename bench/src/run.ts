import { existsSync, mkdirSync, writeFileSync, readdirSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { tmpdir } from "node:os";

import type { CliRunner } from "./runners/types.js";
import { commandsToToolCalls } from "./runners/types.js";
import { ClaudeCodeRunner } from "./runners/claude-code.js";
import { CodexRunner } from "./runners/codex.js";
import { SCENARIOS } from "./scenarios.js";
import {
  scoreScenario,
  printResult,
  printSummary,
  type ScenarioResult,
} from "./scorer.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = join(__dirname, "..", "results");

const ALL_RUNNERS: CliRunner[] = [
  new ClaudeCodeRunner(),
  new CodexRunner(),
];

function getRunner(name: string): CliRunner {
  const runner = ALL_RUNNERS.find((r) => r.name === name);
  if (!runner) {
    const names = ALL_RUNNERS.map((r) => r.name).join(", ");
    throw new Error(`Unknown CLI: ${name}. Must be one of: ${names}, all`);
  }
  return runner;
}

async function runEval(
  runner: CliRunner,
  scenarioFilter?: string,
  timeoutMs = 120_000,
): Promise<ScenarioResult[]> {
  const scenarios = scenarioFilter
    ? SCENARIOS.filter((s) => s.id === scenarioFilter)
    : SCENARIOS;

  if (scenarios.length === 0) {
    console.error(`No scenarios matching '${scenarioFilter}'`);
    process.exit(1);
  }

  // Check CLI availability
  const isAvailable = await runner.available();
  if (!isAvailable) {
    console.error(`CLI '${runner.name}' not found on PATH. Is it installed?`);
    process.exit(1);
  }

  console.log(`\n  Rememora Eval Benchmark`);
  console.log(`  CLI: ${runner.name}`);
  console.log(`  Scenarios: ${scenarios.length}`);
  console.log(`  Timeout: ${timeoutMs}ms per scenario`);
  console.log("─".repeat(60));

  const results: ScenarioResult[] = [];

  for (const scenario of scenarios) {
    try {
      const runResult = await runner.run(scenario.userMessage, {
        cwd: tmpdir(),
        env: {},
        timeoutMs,
      });

      const toolCalls = commandsToToolCalls(runResult.commands);
      const result = scoreScenario(
        scenario,
        runner.name,
        toolCalls,
        runResult.latencyMs,
      );

      results.push(result);
      printResult(result);
    } catch (err) {
      console.error(`\n  \x1b[31mERROR\x1b[0m  ${scenario.name}: ${err}`);
      results.push({
        scenario: { id: scenario.id, name: scenario.name, description: scenario.description },
        cli: runner.name,
        passed: false,
        score: 0,
        expectationResults: [],
        latencyMs: 0,
      });
    }
  }

  printSummary(results);

  // Write results to JSON
  if (!existsSync(RESULTS_DIR)) {
    mkdirSync(RESULTS_DIR, { recursive: true });
  }

  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const filename = `${runner.name}_${timestamp}.json`;
  const outPath = join(RESULTS_DIR, filename);

  const output = {
    cli: runner.name,
    timestamp: new Date().toISOString(),
    summary: {
      total: results.length,
      passed: results.filter((r) => r.passed).length,
      averageScore:
        results.length > 0
          ? results.reduce((sum, r) => sum + r.score, 0) / results.length
          : 0,
      totalLatencyMs: results.reduce((sum, r) => sum + r.latencyMs, 0),
    },
    results: results.map((r) => ({
      ...r,
      expectationResults: r.expectationResults.map((er) => ({
        passed: er.passed,
        description: er.expectation.description,
        reason: er.reason,
        matchedCommand: er.matchedToolCall
          ? (er.matchedToolCall.input.command as string)
          : undefined,
      })),
    })),
  };

  writeFileSync(outPath, JSON.stringify(output, null, 2) + "\n");
  console.log(`\n  Results written to: ${outPath}`);

  return results;
}

function runCompare(): void {
  if (!existsSync(RESULTS_DIR)) {
    console.error("No results directory found. Run eval first.");
    process.exit(1);
  }

  const files = readdirSync(RESULTS_DIR)
    .filter((f) => f.endsWith(".json"))
    .sort();

  if (files.length === 0) {
    console.error("No result files found. Run eval first.");
    process.exit(1);
  }

  console.log(`\n  Rememora Eval — CLI Comparison`);
  console.log("─".repeat(60));
  console.log(
    `  ${"CLI".padEnd(20)} ${"Score".padEnd(10)} ${"Pass".padEnd(8)} Latency`,
  );
  console.log("─".repeat(60));

  for (const file of files) {
    const data = JSON.parse(readFileSync(join(RESULTS_DIR, file), "utf-8"));
    const { cli, summary } = data;
    const scoreStr = `${(summary.averageScore * 100).toFixed(1)}%`;
    const passStr = `${summary.passed}/${summary.total}`;
    const latencyStr = `${Math.round(summary.totalLatencyMs)}ms`;

    console.log(
      `  ${(cli as string).padEnd(20)} ${scoreStr.padEnd(10)} ${passStr.padEnd(8)} ${latencyStr}`,
    );
  }

  console.log("─".repeat(60));
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    options: {
      cli: { type: "string", short: "c" },
      scenario: { type: "string", short: "s" },
      timeout: { type: "string", short: "t" },
      compare: { type: "boolean", default: false },
    },
    strict: true,
  });

  if (values.compare) {
    runCompare();
    return;
  }

  if (!values.cli) {
    const cliNames = ALL_RUNNERS.map((r) => r.name).join(", ");
    console.error(
      "Usage: pnpm --prefix bench run eval -- --cli <cli> [--scenario <id>] [--timeout <ms>]",
    );
    console.error("       pnpm --prefix bench run eval -- --compare");
    console.error(`\nCLIs: ${cliNames}, all`);
    console.error(
      `Scenarios: ${SCENARIOS.map((s) => s.id).join(", ")}`,
    );
    process.exit(1);
  }

  const timeoutMs = values.timeout ? parseInt(values.timeout, 10) : 120_000;

  if (values.cli === "all") {
    for (const runner of ALL_RUNNERS) {
      const isAvailable = await runner.available();
      if (isAvailable) {
        await runEval(runner, values.scenario, timeoutMs);
      } else {
        console.log(`\n  Skipping ${runner.name} (not found on PATH)`);
      }
    }
  } else {
    const runner = getRunner(values.cli);
    await runEval(runner, values.scenario, timeoutMs);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
