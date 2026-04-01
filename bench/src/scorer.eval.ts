import { describe, it, expect, beforeAll } from "vitest";
import { scoreScenario, toJSONLRows, type EvalRow, type ScenarioResult } from "./scorer.js";
import type { Scenario } from "./scenarios.js";
import type { ToolCall } from "./runners/types.js";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const TIMESTAMP = "2026-04-01T12:00:00.000Z";

const scenario: Scenario = {
  id: "save_decision",
  name: "Save Decision",
  description: "When a design decision is made, save it as a decision memory",
  userMessage: "We decided to use PostgreSQL.",
  expectations: [
    {
      toolName: "bash",
      commandPatterns: [/rememora\s+save/, /--category\s+decision/],
      description: "Should save with category=decision",
    },
  ],
};

const multiExpectationScenario: Scenario = {
  id: "session_start",
  name: "Session Start",
  description: "On starting a new session, load context and start a session",
  userMessage: "Initialize your memory system.",
  expectations: [
    {
      toolName: "bash",
      commandPatterns: [/rememora\s+context/],
      description: "Should call rememora context",
    },
    {
      toolName: "bash",
      commandPatterns: [/rememora\s+session\s+start/],
      description: "Should start a rememora session",
    },
  ],
};

const matchingToolCalls: ToolCall[] = [
  { id: "cmd-0", name: "bash", input: { command: "rememora save --category decision --project acme" } },
];

const allMatchingToolCalls: ToolCall[] = [
  { id: "cmd-0", name: "bash", input: { command: "rememora context --auto" } },
  { id: "cmd-1", name: "bash", input: { command: "rememora session start --agent claude --project acme" } },
];

const nonMatchingToolCalls: ToolCall[] = [
  { id: "cmd-0", name: "bash", input: { command: "echo hello" } },
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeRows(results: ScenarioResult[]): EvalRow[] {
  return toJSONLRows(results, TIMESTAMP);
}

function roundTrip(row: EvalRow): EvalRow {
  return JSON.parse(JSON.stringify(row));
}

// ---------------------------------------------------------------------------
// Shape validation — universal interchange format
// ---------------------------------------------------------------------------

describe("EvalRow shape", () => {
  it("has all required top-level keys", () => {
    const result = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const [row] = makeRows([result]);

    expect(row).toHaveProperty("id");
    expect(row).toHaveProperty("input");
    expect(row).toHaveProperty("output");
    expect(row).toHaveProperty("expected");
    expect(row).toHaveProperty("scores");
    expect(row).toHaveProperty("metadata");
    expect(Object.keys(row)).toHaveLength(6);
  });

  it("input is an object with query, scenario_id, scenario_name", () => {
    const result = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const [row] = makeRows([result]);

    expect(typeof row.input.query).toBe("string");
    expect(row.input.query).toBe(scenario.description);
    expect(row.input.scenario_id).toBe("save_decision");
    expect(row.input.scenario_name).toBe("Save Decision");
  });

  it("output has commands (string[]) and tool_calls ({name, command}[])", () => {
    const result = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const [row] = makeRows([result]);

    expect(Array.isArray(row.output.commands)).toBe(true);
    expect(row.output.commands.length).toBeGreaterThan(0);
    expect(typeof row.output.commands[0]).toBe("string");

    expect(Array.isArray(row.output.tool_calls)).toBe(true);
    expect(row.output.tool_calls[0]).toHaveProperty("name");
    expect(row.output.tool_calls[0]).toHaveProperty("command");
    expect(row.output.tool_calls[0].name).toBe("bash");
  });

  it("expected has descriptions (string[]) and patterns (string[])", () => {
    const result = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const [row] = makeRows([result]);

    expect(Array.isArray(row.expected.descriptions)).toBe(true);
    expect(typeof row.expected.descriptions[0]).toBe("string");
    expect(Array.isArray(row.expected.patterns)).toBe(true);
    expect(typeof row.expected.patterns[0]).toBe("string");
  });

  it("scores is Record<string, number> with required keys", () => {
    const result = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const [row] = makeRows([result]);

    for (const [key, value] of Object.entries(row.scores)) {
      expect(typeof key).toBe("string");
      expect(typeof value).toBe("number");
    }

    expect(row.scores).toHaveProperty("accuracy");
    expect(row.scores).toHaveProperty("pass");
    expect(row.scores).toHaveProperty("expectations_met");
    expect(row.scores).toHaveProperty("expectations_total");
  });

  it("metadata has cli, latency_ms, timestamp", () => {
    const result = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const [row] = makeRows([result]);

    expect(typeof row.metadata.cli).toBe("string");
    expect(typeof row.metadata.latency_ms).toBe("number");
    expect(typeof row.metadata.timestamp).toBe("string");
    expect(row.metadata.cli).toBe("claude-code");
    expect(row.metadata.timestamp).toBe(TIMESTAMP);
  });

  it("id is a unique composite of cli/scenario/timestamp", () => {
    const result = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const [row] = makeRows([result]);

    expect(row.id).toBe(`claude-code/save_decision/${TIMESTAMP}`);
  });

  it("survives JSON round-trip without data loss", () => {
    const result = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const [row] = makeRows([result]);

    expect(roundTrip(row)).toEqual(row);
  });
});

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

describe("edge cases", () => {
  it("0 matched expectations → empty output arrays, accuracy 0", () => {
    const result = scoreScenario(scenario, "claude-code", nonMatchingToolCalls, 300);
    const [row] = makeRows([result]);

    expect(row.output.commands).toHaveLength(0);
    expect(row.output.tool_calls).toHaveLength(0);
    expect(row.scores.accuracy).toBe(0);
    expect(row.scores.pass).toBe(0);
    expect(row.scores.expectations_met).toBe(0);
    expect(row.scores.expectations_total).toBe(1);
  });

  it("all expectations matched → accuracy 1, pass 1", () => {
    const result = scoreScenario(multiExpectationScenario, "claude-code", allMatchingToolCalls, 500);
    const [row] = makeRows([result]);

    expect(row.scores.accuracy).toBe(1);
    expect(row.scores.pass).toBe(1);
    expect(row.scores.expectations_met).toBe(2);
    expect(row.scores.expectations_total).toBe(2);
  });

  it("multiple scenarios produce unique IDs", () => {
    const r1 = scoreScenario(scenario, "claude-code", matchingToolCalls, 420);
    const r2 = scoreScenario(multiExpectationScenario, "claude-code", allMatchingToolCalls, 500);
    const rows = makeRows([r1, r2]);

    const ids = rows.map((r) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

// ---------------------------------------------------------------------------
// Platform adapter parsability
// ---------------------------------------------------------------------------

describe("platform adapters", () => {
  let row: EvalRow;

  beforeAll(() => {
    const result = scoreScenario(multiExpectationScenario, "claude-code", allMatchingToolCalls, 500);
    [row] = makeRows([result]);
  });

  it("AI Foundry: can flatten to query/response/ground_truth", () => {
    const foundryRow = {
      query: row.input.query,
      response: row.output.commands.join("\n"),
      ground_truth: row.expected.descriptions.join("\n"),
    };

    expect(typeof foundryRow.query).toBe("string");
    expect(foundryRow.query.length).toBeGreaterThan(0);
    expect(typeof foundryRow.response).toBe("string");
    expect(typeof foundryRow.ground_truth).toBe("string");
  });

  it("Langfuse: expected → expectedOutput, scores iterable as {name, value}", () => {
    const langfuseDatasetItem = {
      input: row.input,
      expectedOutput: row.expected,
      metadata: row.metadata,
    };

    expect(langfuseDatasetItem.expectedOutput).toBe(row.expected);

    const langfuseScores = Object.entries(row.scores).map(([name, value]) => ({
      name,
      value,
      dataType: "NUMERIC" as const,
    }));

    expect(langfuseScores.length).toBeGreaterThan(0);
    for (const s of langfuseScores) {
      expect(typeof s.name).toBe("string");
      expect(typeof s.value).toBe("number");
    }
  });

  it("LangSmith: input → inputs, scores → Feedback {key, score}", () => {
    const langsmithExample = {
      inputs: row.input,
      outputs: row.expected,
      metadata: row.metadata,
    };

    expect(langsmithExample.inputs).toBe(row.input);
    expect(langsmithExample.outputs).toBe(row.expected);

    const feedback = Object.entries(row.scores).map(([key, score]) => ({
      key,
      score,
    }));

    expect(feedback.length).toBeGreaterThan(0);
    for (const f of feedback) {
      expect(typeof f.key).toBe("string");
      expect(typeof f.score).toBe("number");
    }
  });

  it("Braintrust: row is directly importable (all fields match)", () => {
    const braintrustEvent = {
      id: row.id,
      input: row.input,
      output: row.output,
      expected: row.expected,
      scores: row.scores,
      metadata: row.metadata,
    };

    // Braintrust expects these exact top-level keys
    expect(braintrustEvent).toEqual(row);
  });

  it("OpenAI Evals: input.query → messages, expected → ideal", () => {
    const openaiRow = {
      input: [{ role: "user" as const, content: row.input.query }],
      ideal: row.expected.descriptions,
    };

    expect(Array.isArray(openaiRow.input)).toBe(true);
    expect(openaiRow.input[0].role).toBe("user");
    expect(typeof openaiRow.input[0].content).toBe("string");
    expect(Array.isArray(openaiRow.ideal)).toBe(true);
  });
});
