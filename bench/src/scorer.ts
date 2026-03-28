import type { ToolCall } from "./providers/types.js";
import type { Scenario, ToolExpectation } from "./scenarios.js";

/** Result of scoring a single expectation. */
export interface ExpectationResult {
  expectation: ToolExpectation;
  passed: boolean;
  matchedToolCall?: ToolCall;
  reason: string;
}

/** Result of scoring a full scenario. */
export interface ScenarioResult {
  scenario: Pick<Scenario, "id" | "name" | "description">;
  model: string;
  provider: string;
  passed: boolean;
  score: number;
  expectationResults: ExpectationResult[];
  latencyMs: number;
  inputTokens?: number;
  outputTokens?: number;
}

/**
 * Score tool calls against a scenario's expectations.
 *
 * Each expectation is checked against ALL tool calls. An expectation passes
 * if any tool call matches the tool name AND all command patterns match
 * the command string within that tool call's input.
 */
export function scoreScenario(
  scenario: Scenario,
  model: string,
  provider: string,
  toolCalls: ToolCall[],
  latencyMs: number,
  inputTokens?: number,
  outputTokens?: number,
): ScenarioResult {
  const expectationResults: ExpectationResult[] = scenario.expectations.map(
    (expectation) => {
      // Find a matching tool call
      const match = toolCalls.find((tc) => {
        if (tc.name !== expectation.toolName) return false;

        // Extract the command string from the tool call input
        const command = extractCommand(tc.input);
        if (!command) return false;

        // All patterns must match
        return expectation.commandPatterns.every((pattern) =>
          pattern.test(command),
        );
      });

      if (match) {
        return {
          expectation,
          passed: true,
          matchedToolCall: match,
          reason: `Matched tool call: ${extractCommand(match.input)}`,
        };
      }

      // Build a helpful failure reason
      const commands = toolCalls
        .filter((tc) => tc.name === expectation.toolName)
        .map((tc) => extractCommand(tc.input))
        .filter(Boolean);

      const reason =
        commands.length === 0
          ? `No '${expectation.toolName}' tool calls found`
          : `No command matched all patterns. Commands seen: ${commands.join("; ")}`;

      return { expectation, passed: false, reason };
    },
  );

  const passedCount = expectationResults.filter((r) => r.passed).length;
  const score =
    scenario.expectations.length > 0
      ? passedCount / scenario.expectations.length
      : 0;

  return {
    scenario: {
      id: scenario.id,
      name: scenario.name,
      description: scenario.description,
    },
    model,
    provider,
    passed: score === 1,
    score,
    expectationResults,
    latencyMs,
    inputTokens,
    outputTokens,
  };
}

/** Extract the command string from a bash tool call's input. */
function extractCommand(input: Record<string, unknown>): string | undefined {
  if (typeof input.command === "string") return input.command;
  return undefined;
}

/** Pretty-print a scenario result to the console. */
export function printResult(result: ScenarioResult): void {
  const status = result.passed ? "\x1b[32mPASS\x1b[0m" : "\x1b[31mFAIL\x1b[0m";
  console.log(`\n  ${status}  ${result.scenario.name} (${result.scenario.id})`);
  console.log(`         Model: ${result.model} | Latency: ${Math.round(result.latencyMs)}ms`);

  if (result.inputTokens !== undefined) {
    console.log(
      `         Tokens: ${result.inputTokens} in / ${result.outputTokens ?? "?"} out`,
    );
  }

  for (const er of result.expectationResults) {
    const icon = er.passed ? "\x1b[32m✓\x1b[0m" : "\x1b[31m✗\x1b[0m";
    console.log(`         ${icon} ${er.expectation.description}`);
    if (!er.passed) {
      console.log(`           → ${er.reason}`);
    }
  }
}

/** Print a summary table of all results. */
export function printSummary(results: ScenarioResult[]): void {
  const total = results.length;
  const passed = results.filter((r) => r.passed).length;
  const avgScore =
    total > 0 ? results.reduce((sum, r) => sum + r.score, 0) / total : 0;
  const totalLatency = results.reduce((sum, r) => sum + r.latencyMs, 0);

  console.log("\n" + "─".repeat(60));
  console.log(`  Results: ${passed}/${total} scenarios passed`);
  console.log(`  Average score: ${(avgScore * 100).toFixed(1)}%`);
  console.log(`  Total latency: ${Math.round(totalLatency)}ms`);
  console.log("─".repeat(60));
}
