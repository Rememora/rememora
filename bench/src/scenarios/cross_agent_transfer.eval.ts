import { describe, it, expect } from "vitest";

import {
  CLAUDE_TO_CODEX_V0,
  parseHandoffAssertions,
  toEvalRow,
  type CrossAgentAnchor,
  type CrossAgentEvalRow,
  type CrossAgentResult,
  type DbContextRow,
  type DbSessionRow,
  type HandoffAssertionId,
  type HandoffAssertionInput,
} from "./cross_agent_transfer.js";

// ---------------------------------------------------------------------------
// Fixtures — pure data, no child processes.
// ---------------------------------------------------------------------------

const ANCHOR: CrossAgentAnchor = CLAUDE_TO_CODEX_V0.anchor;

const VALID_CATEGORIES = [
  "preference",
  "entity",
  "decision",
  "event",
  "case",
  "pattern",
] as const;

function goodContextRow(): DbContextRow {
  return {
    id: "01FAKEDBCTX0001",
    uri: "rememora://projects/xagent/memories/decision/postgresql-prisma",
    context_type: "memory",
    category: "decision",
    name: ANCHOR.content,
    abstract: ANCHOR.content,
    created_at: "2026-04-21T00:00:00.000Z",
  };
}

function goodSessionRow(): DbSessionRow {
  return {
    id: "01FAKESESS0001",
    agent: "claude-code",
    project: "xagent-claude-to-codex-v0",
    intent: "architect payments API",
    started_at: "2026-04-21T00:00:00.000Z",
    ended_at: "2026-04-21T00:00:30.000Z",
    status: "transferred",
    summary: "handed off to codex",
  };
}

function goodInput(): HandoffAssertionInput {
  return {
    consumerCommands: [
      "rememora context --project xagent-claude-to-codex-v0",
      "rememora search \"payments architecture\" --project xagent-claude-to-codex-v0",
    ],
    consumerRawOutput:
      "Based on prior decisions, the project uses PostgreSQL + Prisma for ACID payment processing. Implementing POST /payments accordingly...",
    dbContexts: [goodContextRow()],
    dbSessions: [goodSessionRow()],
    anchor: ANCHOR,
  };
}

function assertionById(
  assertions: ReturnType<typeof parseHandoffAssertions>,
  id: HandoffAssertionId,
) {
  const a = assertions.find((x) => x.id === id);
  if (!a) throw new Error(`missing assertion ${id}`);
  return a;
}

// ---------------------------------------------------------------------------
// 1. Scenario shape
// ---------------------------------------------------------------------------

describe("cross-agent scenario shape — claude-to-codex-v0", () => {
  it("has exactly two steps: producer + consumer", () => {
    const steps = [CLAUDE_TO_CODEX_V0.producer, CLAUDE_TO_CODEX_V0.consumer];
    expect(steps).toHaveLength(2);
    expect(CLAUDE_TO_CODEX_V0.producer).toBeDefined();
    expect(CLAUDE_TO_CODEX_V0.consumer).toBeDefined();
  });

  it("producer is claude-code and consumer is codex", () => {
    expect(CLAUDE_TO_CODEX_V0.producer.agent).toBe("claude-code");
    expect(CLAUDE_TO_CODEX_V0.consumer.agent).toBe("codex");
  });

  it("anchor.category is a valid rememora category", () => {
    expect(VALID_CATEGORIES).toContain(CLAUDE_TO_CODEX_V0.anchor.category);
  });

  it("producer.userMessage does not leak the word 'rememora'", () => {
    // The agent must figure out to use rememora from its own CLAUDE.md, not
    // from being told by the user.
    expect(
      CLAUDE_TO_CODEX_V0.producer.userMessage.toLowerCase(),
    ).not.toContain("rememora");
  });

  it("anchor.content is a non-empty human-readable string", () => {
    expect(typeof CLAUDE_TO_CODEX_V0.anchor.content).toBe("string");
    expect(CLAUDE_TO_CODEX_V0.anchor.content.length).toBeGreaterThan(5);
  });
});

// ---------------------------------------------------------------------------
// 2. parseHandoffAssertions — known-good + independent mutation failures
// ---------------------------------------------------------------------------

describe("parseHandoffAssertions", () => {
  it("returns all four assertions on a known-good fixture", () => {
    const results = parseHandoffAssertions(goodInput());
    expect(results).toHaveLength(4);
    for (const r of results) {
      expect(r.passed).toBe(true);
    }
  });

  it("`codex_loaded_context` fails when consumer emits no context/search command", () => {
    const input = goodInput();
    input.consumerCommands = [
      "rememora save \"unrelated\" --category event",
      "ls",
    ];
    const results = parseHandoffAssertions(input);

    expect(assertionById(results, "codex_loaded_context").passed).toBe(false);
    // The other assertions must still evaluate on their own inputs — they do
    // not depend on the consumer commands list.
    expect(assertionById(results, "transferred_memory_present_in_db").passed).toBe(true);
    expect(assertionById(results, "producer_session_transferred").passed).toBe(true);
    expect(assertionById(results, "codex_referenced_anchor").passed).toBe(true);
  });

  it("`transferred_memory_present_in_db` fails when dbContexts is empty", () => {
    const input = goodInput();
    input.dbContexts = [];
    const results = parseHandoffAssertions(input);

    expect(assertionById(results, "transferred_memory_present_in_db").passed).toBe(false);
    expect(assertionById(results, "codex_loaded_context").passed).toBe(true);
    expect(assertionById(results, "producer_session_transferred").passed).toBe(true);
    expect(assertionById(results, "codex_referenced_anchor").passed).toBe(true);
  });

  it("`producer_session_transferred` fails when no session has status=transferred", () => {
    const input = goodInput();
    input.dbSessions = [{ ...goodSessionRow(), status: "ended" }];
    const results = parseHandoffAssertions(input);

    expect(assertionById(results, "producer_session_transferred").passed).toBe(false);
    expect(assertionById(results, "codex_loaded_context").passed).toBe(true);
    expect(assertionById(results, "transferred_memory_present_in_db").passed).toBe(true);
    expect(assertionById(results, "codex_referenced_anchor").passed).toBe(true);
  });

  it("`codex_referenced_anchor` fails when consumerRawOutput lacks anchor content", () => {
    const input = goodInput();
    input.consumerRawOutput = "Implementing POST /payments now, nothing to see here.";
    const results = parseHandoffAssertions(input);

    expect(assertionById(results, "codex_referenced_anchor").passed).toBe(false);
    expect(assertionById(results, "codex_loaded_context").passed).toBe(true);
    expect(assertionById(results, "transferred_memory_present_in_db").passed).toBe(true);
    expect(assertionById(results, "producer_session_transferred").passed).toBe(true);
  });

  it("anchor match against consumerRawOutput is case-insensitive", () => {
    const input = goodInput();
    input.consumerRawOutput =
      "Confirming the project uses POSTGRESQL + PRISMA FOR ACID PAYMENT PROCESSING.";
    const results = parseHandoffAssertions(input);
    expect(assertionById(results, "codex_referenced_anchor").passed).toBe(true);
  });

  it("anchor match against DB rows is case-insensitive (name/abstract)", () => {
    const input = goodInput();
    input.dbContexts = [
      {
        ...goodContextRow(),
        name: "POSTGRESQL + PRISMA FOR ACID PAYMENT PROCESSING",
        abstract: "unrelated abstract",
      },
    ];
    const results = parseHandoffAssertions(input);
    expect(
      assertionById(results, "transferred_memory_present_in_db").passed,
    ).toBe(true);
  });

  it("non-memory context types do not satisfy the DB anchor assertion", () => {
    const input = goodInput();
    input.dbContexts = [
      {
        ...goodContextRow(),
        context_type: "project",
      },
    ];
    const results = parseHandoffAssertions(input);
    expect(
      assertionById(results, "transferred_memory_present_in_db").passed,
    ).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// 3. CrossAgentEvalRow shape — Braintrust keys + round-trip + pass domain
// ---------------------------------------------------------------------------

describe("CrossAgentEvalRow shape", () => {
  const timestamp = "2026-04-21T12:00:00.000Z";
  const assertions = parseHandoffAssertions(goodInput());
  const allPassed = assertions.every((a) => a.passed);

  const fakeResult: CrossAgentResult = {
    scenarioId: CLAUDE_TO_CODEX_V0.id,
    producer: {
      cli: "claude-code",
      commands: [
        {
          command:
            "rememora save \"PostgreSQL + Prisma for ACID payment processing\" --category decision",
          source: "structured",
        },
      ],
      rawOutput: "{}",
      exitCode: 0,
      latencyMs: 4321.5,
    },
    consumer: {
      cli: "codex",
      commands: [
        { command: "rememora context --project xagent", source: "structured" },
      ],
      rawOutput: "",
      exitCode: 0,
      latencyMs: 1234.5,
    },
    assertions,
    score: allPassed ? 1 : assertions.filter((a) => a.passed).length / assertions.length,
    pass: allPassed,
    dbPath: "/tmp/fake.db",
    projectDir: "/tmp/fake-project",
    keptDb: false,
    timestamp,
  };

  const row: CrossAgentEvalRow = toEvalRow(CLAUDE_TO_CODEX_V0, fakeResult);

  it("has all six Braintrust top-level keys", () => {
    const expectedKeys = ["id", "input", "output", "expected", "scores", "metadata"];
    for (const key of expectedKeys) {
      expect(row).toHaveProperty(key);
    }
    expect(Object.keys(row).sort()).toEqual([...expectedKeys].sort());
  });

  it("id is a scenario/timestamp composite", () => {
    expect(row.id).toBe(`cross-agent/${CLAUDE_TO_CODEX_V0.id}/${timestamp}`);
  });

  it("scores.pass is exactly 0 or 1", () => {
    expect([0, 1]).toContain(row.scores.pass);
  });

  it("scores.score is in [0, 1]", () => {
    expect(row.scores.score).toBeGreaterThanOrEqual(0);
    expect(row.scores.score).toBeLessThanOrEqual(1);
  });

  it("survives a JSON round-trip without data loss", () => {
    const roundTripped = JSON.parse(JSON.stringify(row));
    expect(roundTripped).toEqual(row);
  });

  it("expected.required_assertions lists the four handoff assertion IDs", () => {
    expect(row.expected.required_assertions).toEqual([
      "codex_loaded_context",
      "transferred_memory_present_in_db",
      "producer_session_transferred",
      "codex_referenced_anchor",
    ]);
  });

  it("output.assertion_ids_passed + assertion_ids_failed partition all assertions", () => {
    const union = new Set([
      ...row.output.assertion_ids_passed,
      ...row.output.assertion_ids_failed,
    ]);
    expect(union.size).toBe(4);
  });
});
