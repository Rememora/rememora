/**
 * Cross-agent transfer scenario — Claude→Codex (v0).
 *
 * This scenario exercises the producer/consumer handoff end-to-end against
 * a shared rememora DB:
 *
 *   1. Claude Code is asked to make an architectural decision on a fresh
 *      project. No mention of "rememora" in the user message — the agent's
 *      own CLAUDE.md configuration must drive the save + session-transfer
 *      behaviour.
 *   2. Codex is then asked to implement work that depends on that prior
 *      decision. No mention of the anchor content — Codex must discover
 *      the decision via `rememora context` / `rememora search`.
 *
 * We grade the handoff with four independent assertions (see
 * `parseHandoffAssertions`). The assertions are pure functions over the
 * captured commands, raw stdout, and the rememora DB state — no child
 * processes in the assertion path, so the test suite can run them against
 * fixtures without touching the real CLIs.
 *
 * NOTE: This module is intentionally standalone. It is NOT imported from
 * `run.ts`, `long-run.ts`, or the vitest bench suite, and it is NOT wired
 * into any `package.json` script. It is an opt-in live experiment invoked
 * directly via `tsx` when a human wants to dogfood the handoff. See the
 * CLI entry at the bottom of this file for invocation.
 */

import { execFile, execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  closeSync,
  openSync,
} from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { randomBytes } from "node:crypto";
import { parseArgs } from "node:util";

import type { RunResult, RunOptions, CapturedCommand } from "../runners/types.js";
import { ClaudeCodeRunner } from "../runners/claude-code.js";
import { CodexRunner } from "../runners/codex.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** A single agent turn in a cross-agent scenario. */
export interface CrossAgentStep {
  /** Which CLI runner is responsible for this step. */
  agent: "claude-code" | "codex";
  /** The user message sent to the agent (no "rememora" leakage). */
  userMessage: string;
}

/** The piece of knowledge that must survive the handoff. */
export interface CrossAgentAnchor {
  /** Human-readable anchor text — what the consumer must end up referencing. */
  content: string;
  /** Rememora category the producer is expected to classify the memory under. */
  category: "preference" | "entity" | "decision" | "event" | "case" | "pattern";
}

/** A two-step cross-agent scenario (producer → consumer). */
export interface CrossAgentScenario {
  id: string;
  name: string;
  description: string;
  anchor: CrossAgentAnchor;
  /** Producer step — creates the memory and transfers the session. */
  producer: CrossAgentStep;
  /** Consumer step — reads the memory and continues the work. */
  consumer: CrossAgentStep;
}

/** Outcome of a single handoff-grading assertion. */
export interface HandoffAssertion {
  /** Stable identifier for the assertion (e.g. `codex_loaded_context`). */
  id: HandoffAssertionId;
  /** Whether the assertion passed. */
  passed: boolean;
  /** Human-readable reason (especially useful on failure). */
  reason: string;
}

/** The four discrete assertions graded for every cross-agent run. */
export type HandoffAssertionId =
  | "codex_loaded_context"
  | "transferred_memory_present_in_db"
  | "producer_session_transferred"
  | "codex_referenced_anchor";

/** Shape of a context row as returned by `rememora export --json`. */
export interface DbContextRow {
  id: string;
  uri: string;
  context_type: string;
  category: string | null;
  name: string;
  abstract: string;
  created_at: string;
  [k: string]: unknown;
}

/** Shape of a session row as returned by `rememora session list --json`. */
export interface DbSessionRow {
  id: string;
  agent: string;
  project: string;
  intent: string | null;
  started_at: string;
  ended_at: string | null;
  status: string;
  summary: string | null;
  [k: string]: unknown;
}

/** Input to the pure assertion evaluator. */
export interface HandoffAssertionInput {
  consumerCommands: string[];
  consumerRawOutput: string;
  dbContexts: DbContextRow[];
  dbSessions: DbSessionRow[];
  anchor: CrossAgentAnchor;
}

/** Full result of `runCrossAgentScenario`. */
export interface CrossAgentResult {
  scenarioId: string;
  producer: RunResult;
  consumer: RunResult;
  assertions: HandoffAssertion[];
  /** Fraction of assertions passed, in [0, 1]. */
  score: number;
  /** True if every assertion passed. */
  pass: boolean;
  dbPath: string;
  projectDir: string;
  keptDb: boolean;
  timestamp: string;
}

/** Braintrust-aligned JSONL row emitted per scenario. */
export interface CrossAgentEvalRow {
  id: string;
  input: {
    scenario_id: string;
    producer_agent: string;
    consumer_agent: string;
    producer_user_message: string;
    consumer_user_message: string;
    anchor_category: CrossAgentAnchor["category"];
  };
  output: {
    producer_commands: string[];
    consumer_commands: string[];
    assertion_ids_passed: HandoffAssertionId[];
    assertion_ids_failed: HandoffAssertionId[];
  };
  expected: {
    anchor_content: string;
    required_assertions: HandoffAssertionId[];
  };
  scores: Record<string, number> & { pass: 0 | 1; score: number };
  metadata: {
    producer_latency_ms: number;
    consumer_latency_ms: number;
    db_path: string;
    timestamp: string;
    scenario_version: "v0";
  };
}

// ---------------------------------------------------------------------------
// Concrete scenarios
// ---------------------------------------------------------------------------

/** The only scenario defined for v0 — Claude→Codex handoff. */
export const CLAUDE_TO_CODEX_V0: CrossAgentScenario = {
  id: "claude-to-codex-v0",
  name: "Claude → Codex architectural handoff",
  description:
    "Claude architects a payments endpoint and persists its decision; " +
    "Codex later picks up the work and must discover the prior decision " +
    "without being told what it is.",
  anchor: {
    content: "PostgreSQL + Prisma for ACID payment processing",
    category: "decision",
  },
  producer: {
    agent: "claude-code",
    userMessage:
      "You are architecting a new Node.js + Express API for payment " +
      "processing. Decide which database and ORM to use (pick PostgreSQL + " +
      "Prisma — ACID guarantees matter for payments). Write up the " +
      "architectural decision and persist it so a future AI coding agent " +
      "picking up this work can find it. When you're done, hand the " +
      "session off cleanly — another agent is going to continue the work.",
  },
  consumer: {
    agent: "codex",
    userMessage:
      "You're continuing work on a Node.js + Express payments API. Before " +
      "you write any code, look up any prior architectural decisions made " +
      "for this project by a previous agent. Then implement a minimal " +
      "POST /payments endpoint consistent with whatever database and ORM " +
      "choices were already made. Do not re-decide the database.",
  },
};

export const ALL_CROSS_AGENT_SCENARIOS: CrossAgentScenario[] = [
  CLAUDE_TO_CODEX_V0,
];

// ---------------------------------------------------------------------------
// Pure grading function
// ---------------------------------------------------------------------------

/**
 * Evaluate the four handoff assertions for a cross-agent run.
 *
 * Pure over its inputs — no I/O, no child processes, safe to call from
 * tests against hand-built fixtures.
 */
export function parseHandoffAssertions(
  input: HandoffAssertionInput,
): HandoffAssertion[] {
  const { consumerCommands, consumerRawOutput, dbContexts, dbSessions, anchor } =
    input;

  const results: HandoffAssertion[] = [];

  // 1. Did Codex emit any context/search call?
  const contextPattern = /rememora\s+(context|search)/;
  const consumerHitContext = consumerCommands.some((c) => contextPattern.test(c));
  results.push({
    id: "codex_loaded_context",
    passed: consumerHitContext,
    reason: consumerHitContext
      ? "consumer emitted at least one `rememora context|search` call"
      : "consumer never invoked `rememora context` or `rememora search`",
  });

  // 2. Is there a memory context whose name/abstract case-insensitively
  //    contains the anchor content?
  const needle = anchor.content.toLowerCase();
  const matchingContext = dbContexts.find(
    (c) =>
      c.context_type === "memory" &&
      ((c.name ?? "").toLowerCase().includes(needle) ||
        (c.abstract ?? "").toLowerCase().includes(needle)),
  );
  results.push({
    id: "transferred_memory_present_in_db",
    passed: Boolean(matchingContext),
    reason: matchingContext
      ? `found memory "${matchingContext.name}" matching anchor`
      : "no memory-context in DB matched anchor content",
  });

  // 3. Did the producer end at least one session with status=transferred?
  const transferredSession = dbSessions.find((s) => s.status === "transferred");
  results.push({
    id: "producer_session_transferred",
    passed: Boolean(transferredSession),
    reason: transferredSession
      ? `session ${transferredSession.id} has status=transferred`
      : "no session in DB has status=transferred",
  });

  // 4. Did Codex actually reference the anchor content in its output?
  const consumerReferenced = consumerRawOutput.toLowerCase().includes(needle);
  results.push({
    id: "codex_referenced_anchor",
    passed: consumerReferenced,
    reason: consumerReferenced
      ? "consumer output mentioned anchor content"
      : "consumer output never mentioned anchor content",
  });

  return results;
}

/** Build a Braintrust-aligned JSONL row from a completed result. */
export function toEvalRow(
  scenario: CrossAgentScenario,
  result: CrossAgentResult,
): CrossAgentEvalRow {
  const passed = result.assertions.filter((a) => a.passed).map((a) => a.id);
  const failed = result.assertions.filter((a) => !a.passed).map((a) => a.id);

  return {
    id: `cross-agent/${scenario.id}/${result.timestamp}`,
    input: {
      scenario_id: scenario.id,
      producer_agent: scenario.producer.agent,
      consumer_agent: scenario.consumer.agent,
      producer_user_message: scenario.producer.userMessage,
      consumer_user_message: scenario.consumer.userMessage,
      anchor_category: scenario.anchor.category,
    },
    output: {
      producer_commands: result.producer.commands.map((c) => c.command),
      consumer_commands: result.consumer.commands.map((c) => c.command),
      assertion_ids_passed: passed,
      assertion_ids_failed: failed,
    },
    expected: {
      anchor_content: scenario.anchor.content,
      required_assertions: [
        "codex_loaded_context",
        "transferred_memory_present_in_db",
        "producer_session_transferred",
        "codex_referenced_anchor",
      ],
    },
    scores: {
      pass: result.pass ? 1 : 0,
      score: result.score,
      assertions_passed: passed.length,
      assertions_total: result.assertions.length,
    },
    metadata: {
      producer_latency_ms: Math.round(result.producer.latencyMs),
      consumer_latency_ms: Math.round(result.consumer.latencyMs),
      db_path: result.dbPath,
      timestamp: result.timestamp,
      scenario_version: "v0",
    },
  };
}

// ---------------------------------------------------------------------------
// Live orchestration (spawns real CLIs) — NOT called from tests
// ---------------------------------------------------------------------------

/** Options to the orchestrator. */
export interface RunCrossAgentOptions {
  /** Per-agent timeout in ms (default: 180_000). */
  timeoutMs?: number;
  /** Keep the DB + project fixture on disk for inspection. */
  keepDb?: boolean;
  /** Override the slug used for `rememora project add`. */
  projectSlug?: string;
  /** Suppress console output. */
  quiet?: boolean;
}

/** Create an empty DB file so rememora's first-run setup gate is satisfied. */
function ensureDbFile(dbPath: string): void {
  const dir = dirname(dbPath);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  if (!existsSync(dbPath)) {
    closeSync(openSync(dbPath, "w"));
  }
}

/** Create a minimal project fixture for the agent to operate against. */
function createProjectFixture(slug: string): string {
  const dir = join(
    tmpdir(),
    `rememora-xagent-${slug}-${randomBytes(4).toString("hex")}`,
  );
  mkdirSync(join(dir, "src"), { recursive: true });

  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify(
      {
        name: slug,
        version: "1.0.0",
        description: "Rememora cross-agent transfer fixture",
        dependencies: { express: "^4.18.0" },
      },
      null,
      2,
    ),
  );

  writeFileSync(
    join(dir, "src", "index.ts"),
    `import express from "express";\n\nconst app = express();\napp.use(express.json());\n\n// Routes will be added here.\n\nexport default app;\n`,
  );

  return dir;
}

/** Run `rememora project add` — tolerate the "already exists" case. */
function addProjectTolerant(
  slug: string,
  projectDir: string,
  dbPath: string,
  log: (msg: string) => void,
): void {
  try {
    execFileSync(
      "rememora",
      [
        "project",
        "add",
        slug,
        "--path",
        projectDir,
        "--description",
        "Rememora cross-agent transfer fixture",
      ],
      {
        env: { ...process.env, REMEMORA_DB: dbPath },
        timeout: 10_000,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
  } catch (err: unknown) {
    const msg = String(
      (err as { stderr?: Buffer | string; message?: string }).stderr ??
        (err as Error).message ??
        "",
    );
    if (/already exists|unique constraint/i.test(msg)) {
      log(`  project ${slug} already exists — continuing`);
    } else {
      throw err;
    }
  }
}

/** Pull context rows from the shared DB via `rememora export --json`. */
function loadDbContexts(dbPath: string): DbContextRow[] {
  try {
    const out = execFileSync("rememora", ["export", "--json"], {
      env: { ...process.env, REMEMORA_DB: dbPath },
      timeout: 15_000,
      encoding: "utf-8",
      maxBuffer: 10 * 1024 * 1024,
    });
    const parsed = JSON.parse(out);
    return Array.isArray(parsed) ? (parsed as DbContextRow[]) : [];
  } catch {
    return [];
  }
}

/** Pull session rows via `rememora session list --json --project <slug>`. */
function loadDbSessions(dbPath: string, slug: string): DbSessionRow[] {
  try {
    const out = execFileSync(
      "rememora",
      ["session", "list", "--project", slug, "--json", "--limit", "50"],
      {
        env: { ...process.env, REMEMORA_DB: dbPath },
        timeout: 10_000,
        encoding: "utf-8",
        maxBuffer: 5 * 1024 * 1024,
      },
    );
    const parsed = JSON.parse(out);
    return Array.isArray(parsed) ? (parsed as DbSessionRow[]) : [];
  } catch {
    return [];
  }
}

/** Write a single JSONL row to `bench/results/xagent_<id>_<ts>.jsonl`. */
function writeResultJsonl(row: CrossAgentEvalRow, timestamp: string): string {
  const __dirname = dirname(fileURLToPath(import.meta.url));
  const resultsDir = join(__dirname, "..", "..", "results");
  if (!existsSync(resultsDir)) mkdirSync(resultsDir, { recursive: true });

  const tsSlug = timestamp.replace(/[:.]/g, "-");
  const outPath = join(
    resultsDir,
    `xagent_${row.input.scenario_id}_${tsSlug}.jsonl`,
  );
  writeFileSync(outPath, JSON.stringify(row) + "\n");
  return outPath;
}

/** Orchestrate a full cross-agent run against live CLIs. */
export async function runCrossAgentScenario(
  scenario: CrossAgentScenario,
  opts: RunCrossAgentOptions = {},
): Promise<CrossAgentResult> {
  const timeoutMs = opts.timeoutMs ?? 180_000;
  const keepDb = opts.keepDb ?? false;
  const slug = opts.projectSlug ?? `xagent-${scenario.id}`;
  const log = opts.quiet ? () => {} : (m: string) => console.log(m);

  const dbPath = join(
    tmpdir(),
    `rememora-xagent-${scenario.id}-${randomBytes(4).toString("hex")}.db`,
  );
  ensureDbFile(dbPath);
  const projectDir = createProjectFixture(slug);
  const timestamp = new Date().toISOString();

  log(`\n  Cross-agent transfer: ${scenario.id}`);
  log(`  DB:      ${dbPath}`);
  log(`  Project: ${projectDir}`);
  log(`  Slug:    ${slug}`);
  log("─".repeat(60));

  addProjectTolerant(slug, projectDir, dbPath, log);

  const runOptions: RunOptions = {
    cwd: projectDir,
    env: { REMEMORA_DB: dbPath },
    timeoutMs,
  };

  // --- Producer (Claude) ---
  log(`\n  [producer/${scenario.producer.agent}] running...`);
  const producerRunner = new ClaudeCodeRunner();
  const producer = await producerRunner.run(
    scenario.producer.userMessage,
    runOptions,
  );
  log(
    `  [producer] done in ${Math.round(producer.latencyMs)}ms, ${producer.commands.length} rememora calls`,
  );

  // --- Consumer (Codex) ---
  log(`\n  [consumer/${scenario.consumer.agent}] running...`);
  const consumerRunner = new CodexRunner();
  const consumer = await consumerRunner.run(
    scenario.consumer.userMessage,
    runOptions,
  );
  log(
    `  [consumer] done in ${Math.round(consumer.latencyMs)}ms, ${consumer.commands.length} rememora calls`,
  );

  // --- Grade ---
  const dbContexts = loadDbContexts(dbPath);
  const dbSessions = loadDbSessions(dbPath, slug);

  const assertions = parseHandoffAssertions({
    consumerCommands: consumer.commands.map((c: CapturedCommand) => c.command),
    consumerRawOutput: consumer.rawOutput,
    dbContexts,
    dbSessions,
    anchor: scenario.anchor,
  });

  const passedCount = assertions.filter((a) => a.passed).length;
  const score = assertions.length === 0 ? 0 : passedCount / assertions.length;
  const pass = passedCount === assertions.length;

  const result: CrossAgentResult = {
    scenarioId: scenario.id,
    producer,
    consumer,
    assertions,
    score,
    pass,
    dbPath,
    projectDir,
    keptDb: keepDb,
    timestamp,
  };

  log("\n  Assertions:");
  for (const a of assertions) {
    const icon = a.passed ? "PASS" : "FAIL";
    log(`    [${icon}] ${a.id} — ${a.reason}`);
  }
  log(`\n  Score: ${(score * 100).toFixed(0)}% (${passedCount}/${assertions.length})`);

  // --- Persist + cleanup ---
  const row = toEvalRow(scenario, result);
  const outPath = writeResultJsonl(row, timestamp);
  log(`\n  JSONL: ${outPath}`);

  if (!keepDb) {
    for (const ext of ["", "-wal", "-shm"]) {
      try {
        rmSync(`${dbPath}${ext}`, { force: true });
      } catch {
        /* best-effort */
      }
    }
    try {
      rmSync(projectDir, { recursive: true, force: true });
    } catch {
      /* best-effort */
    }
  }

  return result;
}

// ---------------------------------------------------------------------------
// CLI entry — opt-in live invocation, e.g.
//   pnpm --prefix bench exec tsx src/scenarios/cross_agent_transfer.ts \
//     --scenario claude-to-codex-v0
// ---------------------------------------------------------------------------

async function mainCli(): Promise<void> {
  const { values } = parseArgs({
    args: process.argv.slice(2).filter((a) => a !== "--"),
    options: {
      scenario: { type: "string", short: "s" },
      "keep-db": { type: "boolean", default: false },
      timeout: { type: "string", short: "t" },
      quiet: { type: "boolean", default: false },
    },
    strict: true,
  });

  const scenarioId = values.scenario ?? CLAUDE_TO_CODEX_V0.id;
  const scenario = ALL_CROSS_AGENT_SCENARIOS.find((s) => s.id === scenarioId);
  if (!scenario) {
    const ids = ALL_CROSS_AGENT_SCENARIOS.map((s) => s.id).join(", ");
    console.error(`Unknown scenario '${scenarioId}'. Known: ${ids}`);
    process.exit(1);
  }

  // Availability check — surface a clear error before we spin up fixtures.
  const claude = new ClaudeCodeRunner();
  const codex = new CodexRunner();
  const [claudeOk, codexOk] = await Promise.all([
    claude.available(),
    codex.available(),
  ]);
  if (!claudeOk || !codexOk) {
    const missing = [
      !claudeOk ? "claude" : null,
      !codexOk ? "codex" : null,
    ].filter(Boolean);
    console.error(
      `Missing CLI(s) on PATH: ${missing.join(", ")}. Install before running this scenario.`,
    );
    process.exit(1);
  }

  const timeoutMs = values.timeout ? parseInt(values.timeout, 10) : undefined;

  const result = await runCrossAgentScenario(scenario, {
    timeoutMs,
    keepDb: values["keep-db"],
    quiet: values.quiet,
  });

  process.exit(result.pass ? 0 : 1);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  // execFile is imported to satisfy runtime sanity on older tsx versions;
  // referenced here so tree-shakers / ts-unused-vars don't complain in CI.
  void execFile;
  mainCli().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
