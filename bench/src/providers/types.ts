/** A tool call extracted from a model completion. */
export interface ToolCall {
  id: string;
  name: string;
  input: Record<string, unknown>;
}

/** Result of a single model completion. */
export interface CompletionResult {
  model: string;
  provider: string;
  toolCalls: ToolCall[];
  rawOutput: unknown;
  latencyMs: number;
  inputTokens?: number;
  outputTokens?: number;
}

/** The bash tool definition shared across all providers. */
export const BASH_TOOL = {
  name: "bash",
  description:
    "Execute a bash command. Use this to run rememora CLI commands and other shell operations.",
  parameters: {
    type: "object" as const,
    properties: {
      command: {
        type: "string",
        description: "The bash command to execute",
      },
    },
    required: ["command"],
  },
};

/** Provider interface that each model SDK implements. */
export interface Provider {
  name: string;
  complete(
    model: string,
    systemPrompt: string,
    userMessage: string,
  ): Promise<CompletionResult>;
}
