/** A command captured from a CLI agent's execution. */
export interface CapturedCommand {
  /** The full command string (e.g., "rememora context --auto"). */
  command: string;
  /** How the command was captured. */
  source: "structured";
}

/** Result of running a CLI agent on a single prompt. */
export interface RunResult {
  cli: string;
  commands: CapturedCommand[];
  rawOutput: string;
  exitCode: number;
  latencyMs: number;
}

/** Options passed to a CLI runner. */
export interface RunOptions {
  /** Working directory for the CLI process. */
  cwd: string;
  /** Extra environment variables (e.g., PATH with shim dir prepended). */
  env: Record<string, string>;
  /** Timeout in milliseconds. */
  timeoutMs: number;
  /** Optional instruction text to pass as the system prompt. Overrides the runner's built-in default. */
  instructionText?: string;
}

/** Interface that each CLI runner implements. */
export interface CliRunner {
  name: string;
  /** Check if the CLI binary is available on PATH. */
  available(): Promise<boolean>;
  /** Run the CLI with a prompt and return captured commands. */
  run(prompt: string, options: RunOptions): Promise<RunResult>;
}

/** A tool call extracted from CLI output (for scorer compatibility). */
export interface ToolCall {
  id: string;
  name: string;
  input: Record<string, unknown>;
}

/** Convert captured commands into ToolCall format for the scorer. */
export function commandsToToolCalls(commands: CapturedCommand[]): ToolCall[] {
  return commands.map((c, i) => ({
    id: `cmd-${i}`,
    name: "bash",
    input: { command: c.command },
  }));
}
