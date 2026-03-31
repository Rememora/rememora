import { execFile } from "node:child_process";

import type { CliRunner, RunResult, RunOptions, CapturedCommand } from "./types.js";

/** Extract rememora commands from Claude Code's JSON output (tool_use blocks). */
function parseStructuredOutput(raw: string): CapturedCommand[] {
  try {
    const events = JSON.parse(raw);
    if (!Array.isArray(events)) return [];

    const commands: CapturedCommand[] = [];
    for (const event of events) {
      // Claude -p --output-format json emits objects with type "assistant"
      // whose message.content contains tool_use blocks.
      const content = event?.message?.content ?? event?.content;
      if (!Array.isArray(content)) continue;

      for (const block of content) {
        if (block.type !== "tool_use") continue;
        if (block.name !== "Bash" && block.name !== "bash") continue;
        const cmd = block.input?.command;
        if (typeof cmd === "string" && cmd.includes("rememora")) {
          commands.push({ command: cmd, source: "structured" });
        }
      }
    }
    return commands;
  } catch {
    return [];
  }
}

export class ClaudeCodeRunner implements CliRunner {
  name = "claude-code";

  async available(): Promise<boolean> {
    return new Promise((resolve) => {
      execFile("which", ["claude"], (err) => resolve(!err));
    });
  }

  async run(prompt: string, options: RunOptions): Promise<RunResult> {
    const start = performance.now();

    const result = await new Promise<{ stdout: string; stderr: string; exitCode: number }>(
      (resolve) => {
        execFile(
          "claude",
          [
            "-p", prompt,
            "--output-format", "json",
            "--allowedTools", "Bash(rememora:*)",
            "--max-turns", "5",
          ],
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
    const commands = parseStructuredOutput(result.stdout);

    return {
      cli: this.name,
      commands,
      rawOutput: result.stdout,
      exitCode: result.exitCode,
      latencyMs,
    };
  }
}
