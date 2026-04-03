import { execFileSync, execSync } from "node:child_process";
import { writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { randomBytes } from "node:crypto";

import type { CliRunner, RunResult, RunOptions, CapturedCommand } from "./types.js";

// ---------------------------------------------------------------------------
// Tmux helpers
// ---------------------------------------------------------------------------

function tmuxRun(cmd: string): string {
  return execSync(cmd, { encoding: "utf-8", timeout: 10_000 }).trim();
}

function tmuxSendKeys(session: string, keys: string): void {
  // Write to a temp file and use load-buffer to avoid shell escaping issues
  const tmpFile = join("/tmp", `tmux-keys-${randomBytes(4).toString("hex")}.txt`);
  writeFileSync(tmpFile, keys);
  execSync(`tmux load-buffer ${tmpFile}`, { timeout: 5_000 });
  execSync(`tmux paste-buffer -t ${session}`, { timeout: 5_000 });
  execSync(`rm -f ${tmpFile}`, { timeout: 5_000 });
}

function tmuxSendEnter(session: string): void {
  execSync(`tmux send-keys -t ${session} Enter`, { timeout: 5_000 });
}

function tmuxCapture(session: string, lines = 500): string {
  try {
    return execSync(
      `tmux capture-pane -t ${session} -p -S -${lines}`,
      { encoding: "utf-8", timeout: 10_000 },
    );
  } catch {
    return "";
  }
}

function tmuxSessionExists(session: string): boolean {
  try {
    execSync(`tmux has-session -t ${session}`, { timeout: 5_000, stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function tmuxKill(session: string): void {
  try {
    execSync(`tmux kill-session -t ${session}`, { timeout: 5_000, stdio: "ignore" });
  } catch { /* already gone */ }
}

// ---------------------------------------------------------------------------
// Polling: wait for Claude's input prompt to appear
// ---------------------------------------------------------------------------

/**
 * Claude Code shows a `>` prompt when ready for input.
 * After responding, it shows another `>`.
 * We detect this by looking for a line matching the prompt pattern
 * at the bottom of the pane.
 */
function waitForPrompt(
  session: string,
  timeoutMs: number,
  afterText?: string,
): { output: string; timedOut: boolean } {
  const start = Date.now();
  const pollIntervalMs = 2_000;
  let lastCapture = "";

  while (Date.now() - start < timeoutMs) {
    const capture = tmuxCapture(session, 1000);
    const lines = capture.split("\n");

    // Find the last non-empty line
    let lastLine = "";
    for (let i = lines.length - 1; i >= 0; i--) {
      const stripped = lines[i].replace(/\x1b\[[0-9;]*m/g, "").trim();
      if (stripped.length > 0) {
        lastLine = stripped;
        break;
      }
    }

    // Claude's prompt is typically ">" or "> " at the start of the last line
    // It can also look like "❯" or contain the path
    const isPrompt = /^[>❯]\s*$/.test(lastLine) || /^[>❯]/.test(lastLine);

    // If we have afterText, make sure the capture contains content AFTER it
    // (so we don't match the prompt from before we sent our message)
    if (isPrompt && afterText) {
      const afterIdx = capture.lastIndexOf(afterText);
      if (afterIdx >= 0) {
        const contentAfter = capture.slice(afterIdx + afterText.length);
        // There should be substantial content between our message and the new prompt
        const stripped = contentAfter.replace(/\x1b\[[0-9;]*m/g, "").trim();
        if (stripped.length > 10) {
          return { output: capture, timedOut: false };
        }
      }
    } else if (isPrompt && !afterText) {
      return { output: capture, timedOut: false };
    }

    lastCapture = capture;
    execSync(`sleep ${pollIntervalMs / 1000}`);
  }

  return { output: lastCapture, timedOut: true };
}

// ---------------------------------------------------------------------------
// Parse rememora commands from terminal output
// ---------------------------------------------------------------------------

export function parseTerminalCommands(raw: string): CapturedCommand[] {
  return parseTerminalOutput(raw);
}

function parseTerminalOutput(raw: string): CapturedCommand[] {
  const commands: CapturedCommand[] = [];
  const seen = new Set<string>();

  for (const line of raw.split("\n")) {
    // Strip ANSI escape codes
    const clean = line.replace(/\x1b\[[0-9;]*m/g, "").trim();

    // Look for rememora CLI invocations
    // They appear as: $ rememora save "..." or just rememora save "..."
    const match = clean.match(/(?:^|\$\s*)(rememora\s+(?:save|search|context|session|project|get|status|export|extract|relate|supersede|setup|evolve)\b[^\n]*)/);
    if (match) {
      const cmd = match[1].trim();
      if (!seen.has(cmd)) {
        seen.add(cmd);
        commands.push({ command: cmd, source: "structured" });
      }
    }
  }

  return commands;
}

// ---------------------------------------------------------------------------
// Claude Tmux Runner
// ---------------------------------------------------------------------------

export class ClaudeTmuxRunner implements CliRunner {
  name = "claude-tmux";

  async available(): Promise<boolean> {
    try {
      execFileSync("which", ["tmux"], { stdio: "ignore" });
      execFileSync("which", ["claude"], { stdio: "ignore" });
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Start a persistent Claude interactive session in tmux.
   * Returns the session name for subsequent prompt injections.
   */
  startSession(options: RunOptions): string {
    const sessionName = `rememora-eval-${randomBytes(4).toString("hex")}`;

    // Build claude command with optional system prompt
    const parts = ["claude"];
    if (options.instructionText && options.instructionText.trim().length > 0) {
      // Write instruction text to a temp file to avoid shell escaping
      const instrFile = join("/tmp", `rememora-instr-${randomBytes(4).toString("hex")}.txt`);
      writeFileSync(instrFile, options.instructionText);
      parts.push("--append-system-prompt-file", instrFile);
    }

    const claudeCmd = parts.join(" ");

    // Create tmux session running claude in the project directory
    execSync(
      `tmux new-session -d -s ${sessionName} -c "${options.cwd}"`,
      { timeout: 10_000 },
    );

    // Set environment variables in the tmux session
    for (const [key, value] of Object.entries(options.env)) {
      execSync(
        `tmux send-keys -t ${sessionName} 'export ${key}="${value}"' Enter`,
        { timeout: 5_000 },
      );
    }

    // Small delay for exports to take effect
    execSync("sleep 1");

    // Launch claude
    tmuxSendKeys(sessionName, claudeCmd);
    tmuxSendEnter(sessionName);

    // Wait for Claude to be ready, handling the trust dialog if it appears
    const startupDeadline = Date.now() + 90_000;
    while (Date.now() < startupDeadline) {
      execSync("sleep 3");
      const pane = tmuxCapture(sessionName, 50);

      // Handle trust dialog: "Yes, I trust this folder"
      if (pane.includes("trust this folder") || pane.includes("Enter to confirm")) {
        execSync(`tmux send-keys -t ${sessionName} Enter`, { timeout: 5_000 });
        execSync("sleep 3");
        continue;
      }

      // Handle any other yes/no prompts during startup
      if (pane.includes("(y/n)") || pane.includes("[Y/n]")) {
        execSync(`tmux send-keys -t ${sessionName} y Enter`, { timeout: 5_000 });
        execSync("sleep 2");
        continue;
      }

      // Check if Claude's input prompt is ready
      const lines = pane.split("\n");
      for (let i = lines.length - 1; i >= 0; i--) {
        const stripped = lines[i].replace(/\x1b\[[0-9;]*m/g, "").trim();
        if (stripped.length > 0) {
          if (/^[>❯]/.test(stripped)) {
            return sessionName; // Ready for input
          }
          break;
        }
      }
    }

    tmuxKill(sessionName);
    throw new Error("Claude did not start within 90s");

    return sessionName;
  }

  /**
   * Send a prompt to an existing tmux Claude session and wait for the response.
   */
  async sendPrompt(
    sessionName: string,
    prompt: string,
    timeoutMs: number,
  ): Promise<{ output: string; latencyMs: number }> {
    const start = performance.now();

    // Capture pane before sending so we can diff
    const before = tmuxCapture(sessionName, 1000);

    // Send the prompt
    tmuxSendKeys(sessionName, prompt);
    tmuxSendEnter(sessionName);

    // Wait for Claude to finish and show prompt again
    // Use a snippet of the prompt as the "after" marker
    const marker = prompt.slice(0, 80);
    const { output, timedOut } = waitForPrompt(sessionName, timeoutMs, marker);

    const latencyMs = performance.now() - start;

    if (timedOut) {
      // Send Escape to cancel any in-progress generation, then capture what we have
      execSync(`tmux send-keys -t ${sessionName} Escape`, { timeout: 5_000 });
      execSync("sleep 2");
    }

    // Get the new content (diff from before)
    const newContent = output.slice(before.length > 100 ? output.indexOf(marker) : 0);

    return { output: newContent || output, latencyMs };
  }

  /**
   * End the tmux Claude session.
   */
  endSession(sessionName: string): void {
    // Try to exit Claude gracefully
    try {
      tmuxSendKeys(sessionName, "/exit");
      tmuxSendEnter(sessionName);
      execSync("sleep 2");
    } catch { /* already gone */ }

    tmuxKill(sessionName);
  }

  /**
   * Run a single prompt (CliRunner interface compatibility).
   * For multi-task sequences, use startSession/sendPrompt/endSession directly.
   */
  async run(prompt: string, options: RunOptions): Promise<RunResult> {
    const sessionName = this.startSession(options);

    try {
      const { output, latencyMs } = await this.sendPrompt(
        sessionName,
        prompt,
        options.timeoutMs,
      );

      const commands = parseTerminalOutput(output);

      return {
        cli: this.name,
        commands,
        rawOutput: output,
        exitCode: 0,
        latencyMs,
      };
    } finally {
      this.endSession(sessionName);
    }
  }
}
