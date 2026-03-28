/** Expectation for a single tool call in a scenario. */
export interface ToolExpectation {
  /** The tool name expected (e.g. "bash"). */
  toolName: string;
  /** Regex patterns that must ALL match somewhere in the command string. */
  commandPatterns: RegExp[];
  /** Human-readable description of what this expectation checks. */
  description: string;
}

/** A benchmark scenario that tests whether a model follows rememora instructions. */
export interface Scenario {
  id: string;
  name: string;
  description: string;
  /** The system prompt injected (simulates CLAUDE.md / agent instructions). */
  systemPrompt: string;
  /** The user message that triggers the expected behavior. */
  userMessage: string;
  /** Expected tool calls the model should produce. */
  expectations: ToolExpectation[];
}

const REMEMORA_SYSTEM_PROMPT = `You are an AI coding assistant with access to a bash tool for running shell commands.

## Rememora Memory System

Rememora is a cross-agent persistent memory system. Use the \`rememora\` CLI for memory across sessions, projects, and agents.

### On session start:
1. \`rememora context --auto\` — load prior context
2. If project not registered: \`rememora project add <name> --path <cwd> --description "..."\`
3. \`rememora session start --agent claude-code --project <name> --intent "what you're doing"\`

### During work — save knowledge as you discover it:
- Codebase facts: \`rememora save "..." --category entity --project <name>\`
- Decisions: \`rememora save "..." --category decision --project <name> --importance 0.9\`
- Problems solved: \`rememora save "..." --category case --project <name>\`
- Patterns: \`rememora save "..." --category pattern --project <name>\`

### When you need to recall something:
\`rememora search "query" --project <name>\`

### Before ending session:
\`rememora session end <id> --summary "what was accomplished" --working-state "current status"\`

### When handing off to another agent:
\`rememora session end <id> --status transferred --summary "..." --working-state "..."\``;

export const SCENARIOS: Scenario[] = [
  {
    id: "session_start",
    name: "Session Start",
    description:
      "On starting a new session, the agent should load context and start a session",
    systemPrompt: REMEMORA_SYSTEM_PROMPT,
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
    systemPrompt: REMEMORA_SYSTEM_PROMPT,
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
    systemPrompt: REMEMORA_SYSTEM_PROMPT,
    userMessage:
      "I just fixed a tricky bug in acme-api where the auth middleware was failing silently because the JWT_SECRET env var had a trailing newline from the .env file. Save this problem and solution to memory so we remember it.",
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
    systemPrompt: REMEMORA_SYSTEM_PROMPT,
    userMessage:
      "I remember we made a decision about which database to use for acme-api a while back. Can you look that up in memory?",
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
    systemPrompt: REMEMORA_SYSTEM_PROMPT,
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
    systemPrompt: REMEMORA_SYSTEM_PROMPT,
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
