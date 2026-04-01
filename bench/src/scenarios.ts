/** Expectation for a single tool call in a scenario. */
export interface ToolExpectation {
  /** The tool name expected (e.g. "bash"). */
  toolName: string;
  /** Regex patterns that must ALL match somewhere in the command string. */
  commandPatterns: RegExp[];
  /** Human-readable description of what this expectation checks. */
  description: string;
}

/**
 * A benchmark scenario that tests whether a CLI agent follows rememora instructions.
 *
 * The CLI agent's own configuration (e.g. CLAUDE.md) provides the rememora
 * system prompt — scenarios only define the user message and expectations.
 */
export interface Scenario {
  id: string;
  name: string;
  description: string;
  /** The user message sent to the CLI agent. */
  userMessage: string;
  /** Expected tool calls the agent should produce. */
  expectations: ToolExpectation[];
}

export const SCENARIOS: Scenario[] = [
  {
    id: "session_start",
    name: "Session Start",
    description:
      "On starting a new session, the agent should load context and start a session",
    userMessage:
      "You are starting a new coding session on the project 'acme-api' located at /home/user/acme-api. The project is a REST API built with Express. Initialize your memory system.",
    expectations: [
      {
        toolName: "bash",
        commandPatterns: [/rememora\s+context/],
        description: "Should call rememora context to load prior context",
      },
      {
        toolName: "bash",
        commandPatterns: [/rememora\s+session\s+start/],
        description: "Should start a new rememora session",
      },
    ],
  },
  {
    id: "save_decision",
    name: "Save Decision",
    description:
      "When a design decision is made, the agent should save it as a decision memory",
    userMessage:
      "We just decided to use PostgreSQL instead of MongoDB for the acme-api project because we need strong ACID transactions for the payment system. Save this architectural decision to memory.",
    expectations: [
      {
        toolName: "bash",
        commandPatterns: [/rememora\s+save/, /--category\s+decision/],
        description:
          "Should save with category=decision via rememora save --category decision",
      },
    ],
  },
  {
    id: "save_case",
    name: "Save Case",
    description:
      "When a problem is solved, the agent should save it as a case memory",
    userMessage:
      "I just fixed a tricky bug in acme-api where the auth middleware was failing silently because the JWT_SECRET env var had a trailing newline from the .env file. Save this problem and its solution to our persistent memory as a case so other agents can learn from it.",
    expectations: [
      {
        toolName: "bash",
        commandPatterns: [/rememora\s+save/, /--category\s+case/],
        description:
          "Should save with category=case via rememora save --category case",
      },
    ],
  },
  {
    id: "search_knowledge",
    name: "Search Knowledge",
    description:
      "When asked to recall information, the agent should search rememora",
    userMessage:
      "I remember we made a decision about which database to use for acme-api a while back. Search our persistent memory for that decision.",
    expectations: [
      {
        toolName: "bash",
        commandPatterns: [/rememora\s+search/],
        description: "Should search rememora for the database decision",
      },
    ],
  },
  {
    id: "transfer_handoff",
    name: "Transfer Handoff",
    description:
      "When handing off to another agent, the session should end with status=transferred",
    userMessage:
      "I'm going to switch to a different AI agent to continue this work on acme-api. My current session ID is 01JTEST123. Please end the session and hand off properly so the next agent can pick up where we left off.",
    expectations: [
      {
        toolName: "bash",
        commandPatterns: [
          /rememora\s+session\s+end/,
          /--status\s+transferred/,
        ],
        description:
          "Should end session with --status transferred for proper handoff",
      },
    ],
  },
  {
    id: "session_end",
    name: "Session End",
    description:
      "When ending a session normally, the agent should end with a summary",
    userMessage:
      "We're done for today on acme-api. My session ID is 01JTEST456. Please wrap up the session with a summary of what we accomplished: we added the /users endpoint and fixed the auth middleware bug.",
    expectations: [
      {
        toolName: "bash",
        commandPatterns: [/rememora\s+session\s+end/, /--summary/],
        description: "Should end session with --summary flag",
      },
    ],
  },
];
