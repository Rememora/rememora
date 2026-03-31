import { execFile } from "node:child_process";

import type { CliRunner, RunResult, RunOptions, CapturedCommand } from "./types.js";

/** Extract rememora commands from Codex JSONL output. */
function parseCodexOutput(raw: string): CapturedCommand[] {
  const commands: CapturedCommand[] = [];

  for (const line of raw.split("\n")) {
    if (!line.trim()) continue;
    try {
      const event = JSON.parse(line);
      // Codex emits JSONL events — try known shapes for shell commands.
      const cmd =
        event?.command ??
        event?.function_call?.arguments?.command ??
        event?.args?.command;

      if (typeof cmd === "string" && cmd.includes("rememora")) {
        commands.push({ command: cmd, source: "structured" });
      }
    } catch {
      // Not JSON — skip
    }
  }

  return commands;
}

export class CodexRunner implements CliRunner {
  name = "codex";

  async available(): Promise<boolean> {
    return new Promise((resolve) => {
      execFile("which", ["codex"], (err) => resolve(!err));
    });
  }

  async run(prompt: string, options: RunOptions): Promise<RunResult> {
    const start = performance.now();

    const result = await new Promise<{ stdout: string; stderr: string; exitCode: number }>(
      (resolve) => {
        execFile(
          "codex",
          ["exec", prompt],
          {
            cwd: options.cwd,
            env: { ...process.env, ...options.env },
            timeout: options.timeoutMs,
            maxBuffer: 10 * 1024 * 1024,
          },
          (err, stdout, stderr) => {
            resolve({
              stdout: stdout ?? "",
              stderr: stderr ?? "",
              exitCode: err ? 1 : 0,
            });
          },
        );
      },
    );

    const latencyMs = performance.now() - start;
    const commands = parseCodexOutput(result.stdout);

    return {
      cli: this.name,
      commands,
      rawOutput: result.stdout,
      exitCode: result.exitCode,
      latencyMs,
    };
  }
}
