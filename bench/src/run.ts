import { existsSync, mkdirSync, writeFileSync, readdirSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import type { Provider } from "./providers/types.js";
import { AnthropicProvider } from "./providers/anthropic.js";
import { OpenAIProvider } from "./providers/openai.js";
import { GoogleProvider } from "./providers/google.js";
import { SCENARIOS } from "./scenarios.js";
import {
  scoreScenario,
  printResult,
  printSummary,
  type ScenarioResult,
} from "./scorer.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = join(__dirname, "..", "results");

function getProvider(name: string): Provider {
  switch (name) {
    case "anthropic":
      return new AnthropicProvider();
    case "openai":
      return new OpenAIProvider();
    case "google":
      return new GoogleProvider();
    default:
      throw new Error(
        `Unknown provider: ${name}. Must be one of: anthropic, openai, google`,
      );
  }
}

/** Infer provider from model name if not explicitly set. */
function inferProvider(model: string): string {
  if (model.startsWith("claude") || model.startsWith("claude-")) return "anthropic";
  if (model.startsWith("gpt-") || model.startsWith("o1") || model.startsWith("o3")) return "openai";
  if (model.startsWith("gemini")) return "google";
  throw new Error(
    `Cannot infer provider for model '${model}'. Use --provider to specify.`,
  );
}

async function runEval(model: string, providerName: string, scenarioFilter?: string): Promise<void> {
  const provider = getProvider(providerName);
  const scenarios = scenarioFilter
    ? SCENARIOS.filter((s) => s.id === scenarioFilter)
    : SCENARIOS;

  if (scenarios.length === 0) {
    console.error(`No scenarios matching '${scenarioFilter}'`);
    process.exit(1);
  }

  console.log(`\n  Rememora Eval Benchmark`);
  console.log(`  Model: ${model} | Provider: ${providerName}`);
  console.log(`  Scenarios: ${scenarios.length}`);
  console.log("─".repeat(60));

  const results: ScenarioResult[] = [];

  for (const scenario of scenarios) {
    try {
      const completion = await provider.complete(
        model,
        scenario.systemPrompt,
        scenario.userMessage,
      );

      const result = scoreScenario(
        scenario,
        model,
        providerName,
        completion.toolCalls,
        completion.latencyMs,
        completion.inputTokens,
        completion.outputTokens,
      );

      results.push(result);
      printResult(result);
    } catch (err) {
      console.error(`\n  \x1b[31mERROR\x1b[0m  ${scenario.name}: ${err}`);
      results.push({
        scenario: { id: scenario.id, name: scenario.name, description: scenario.description },
        model,
        provider: providerName,
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
  const filename = `${providerName}_${model.replace(/[/.]/g, "-")}_${timestamp}.json`;
  const outPath = join(RESULTS_DIR, filename);

  const output = {
    model,
    provider: providerName,
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
      // Strip raw regex from serialized output
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

  console.log(`\n  Rememora Eval — Model Comparison`);
  console.log("─".repeat(72));
  console.log(
    `  ${"Model".padEnd(30)} ${"Provider".padEnd(12)} ${"Score".padEnd(10)} ${"Pass".padEnd(8)} Latency`,
  );
  console.log("─".repeat(72));

  for (const file of files) {
    const data = JSON.parse(readFileSync(join(RESULTS_DIR, file), "utf-8"));
    const { model, provider, summary } = data;
    const scoreStr = `${(summary.averageScore * 100).toFixed(1)}%`;
    const passStr = `${summary.passed}/${summary.total}`;
    const latencyStr = `${Math.round(summary.totalLatencyMs)}ms`;

    console.log(
      `  ${model.padEnd(30)} ${provider.padEnd(12)} ${scoreStr.padEnd(10)} ${passStr.padEnd(8)} ${latencyStr}`,
    );
  }

  console.log("─".repeat(72));
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    options: {
      model: { type: "string", short: "m" },
      provider: { type: "string", short: "p" },
      scenario: { type: "string", short: "s" },
      compare: { type: "boolean", short: "c", default: false },
    },
    strict: true,
  });

  if (values.compare) {
    runCompare();
    return;
  }

  if (!values.model) {
    console.error(
      "Usage: pnpm --prefix bench run eval -- --model <model> [--provider <provider>] [--scenario <id>]",
    );
    console.error("       pnpm --prefix bench run eval -- --compare");
    console.error("\nModels: claude-haiku-4-5-20251001, gpt-4o-mini, gemini-2.5-pro, ...");
    console.error("Providers: anthropic, openai, google (auto-inferred from model name)");
    console.error(
      `Scenarios: ${SCENARIOS.map((s) => s.id).join(", ")}`,
    );
    process.exit(1);
  }

  const providerName = values.provider ?? inferProvider(values.model);
  await runEval(values.model, providerName, values.scenario);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
