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
 * Claude Code shows a `❯` prompt when ready for input.
 * The prompt appears in the middle of the TUI (not the very last line)
 * because the status bar sits below it. We scan the last N lines for
 * a line that starts with `❯` or `>`.
 */
function paneHasPrompt(capture: string): boolean {
  const lines = capture.split("\n");
  // Scan the last 15 lines — the prompt is above the status bar
  const tail = lines.slice(-15);
  for (const line of tail) {
    const stripped = line.replace(/\x1b\[[0-9;]*m/g, "").trim();
    if (/^[>❯]\s*$/.test(stripped)) return true;
  }
  return false;
}

function waitForPrompt(
  session: string,
  timeoutMs: number,
  afterText?: string,
): { output: string; timedOut: boolean } {
  const start = Date.now();
  const pollIntervalMs = 3_000;
  let lastCapture = "";

  while (Date.now() - start < timeoutMs) {
    const capture = tmuxCapture(session, 1000);
    const hasPrompt = paneHasPrompt(capture);

    if (hasPrompt && afterText) {
      // Make sure the capture contains content AFTER our sent text
      const afterIdx = capture.lastIndexOf(afterText);
      if (afterIdx >= 0) {
        const contentAfter = capture.slice(afterIdx + afterText.length);
        const stripped = contentAfter.replace(/\x1b\[[0-9;]*m/g, "").trim();
        if (stripped.length > 20) {
          return { output: capture, timedOut: false };
        }
      }
    } else if (hasPrompt && !afterText) {
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

  // Claude Code TUI shows tool calls as:
  //   ⏺ Bash(rememora save "..." --category decision ...)
  //   ⎿  <output>
  // Also match raw command lines:
  //   $ rememora save "..."
  //   rememora save "..."
  const REMEMORA_SUBCOMMANDS = "save|search|context|session|project|get|status|export|extract|relate|supersede|setup|evolve";

  for (const line of raw.split("\n")) {
    // Strip ANSI escape codes
    const clean = line.replace(/\x1b\[[0-9;]*m/g, "").trim();

    // Match: ⏺ Bash(rememora save "..." ...)
    const bashToolMatch = clean.match(new RegExp(`Bash\\((rememora\\s+(?:${REMEMORA_SUBCOMMANDS})\\b[^)]*)`));
    if (bashToolMatch) {
      const cmd = bashToolMatch[1].trim();
      if (!seen.has(cmd)) {
        seen.add(cmd);
        commands.push({ command: cmd, source: "structured" });
      }
      continue;
    }

    // Match: $ rememora save "..." or bare rememora save "..."
    const rawMatch = clean.match(new RegExp(`(?:^|\\$\\s*)(rememora\\s+(?:${REMEMORA_SUBCOMMANDS})\\b[^\\n]*)`));
    if (rawMatch) {
      const cmd = rawMatch[1].trim();
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

    // Build claude command with optional system prompt.
    // --dangerously-skip-permissions avoids interactive permission prompts
    // that block the eval. This is safe in eval context (temp fixture dir).
    const parts = ["claude", "--dangerously-skip-permissions"];
    if (options.instructionText && options.instructionText.trim().length > 0) {
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

    // Set environment variables at BOTH tmux session level and shell level.
    // tmux setenv makes vars available to all new processes in the session.
    // Shell export ensures the current shell (and claude) inherit them.
    for (const [key, value] of Object.entries(options.env)) {
      execSync(
        `tmux setenv -t ${sessionName} ${key} "${value}"`,
        { timeout: 5_000 },
      );
      execSync(
        `tmux send-keys -t ${sessionName} 'export ${key}="${value}"' Enter`,
        { timeout: 5_000 },
      );
    }

    // Small delay for exports to take effect
    execSync("sleep 1");

    // Launch claude with env vars as prefix for belt-and-suspenders
    const envPrefix = Object.entries(options.env)
      .map(([k, v]) => `${k}="${v}"`)
      .join(" ");
    tmuxSendKeys(sessionName, `${envPrefix} ${claudeCmd}`);
    tmuxSendEnter(sessionName);

    // Wait for Claude to be ready, handling the trust dialog if it appears
    const startupDeadline = Date.now() + 120_000;
    while (Date.now() < startupDeadline) {
      execSync("sleep 4");
      const pane = tmuxCapture(sessionName, 50);

      // Handle trust dialog: "Yes, I trust this folder"
      if (pane.includes("trust this folder") || pane.includes("Enter to confirm")) {
        execSync(`tmux send-keys -t ${sessionName} Enter`, { timeout: 5_000 });
        execSync("sleep 5");
        continue;
      }

      // Handle any other yes/no prompts during startup
      if (pane.includes("(y/n)") || pane.includes("[Y/n]")) {
        execSync(`tmux send-keys -t ${sessionName} y Enter`, { timeout: 5_000 });
        execSync("sleep 3");
        continue;
      }

      // Check if Claude's ❯ prompt is visible (it sits above the status bar)
      if (paneHasPrompt(pane)) {
        return sessionName; // Ready for input
      }
    }

    tmuxKill(sessionName);
    throw new Error("Claude did not start within 120s");

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
    const beforeLines = tmuxCapture(sessionName, 1000).split("\n").length;

    // Send the prompt
    tmuxSendKeys(sessionName, prompt);
    tmuxSendEnter(sessionName);

    // Wait for Claude to finish and show prompt again
    const marker = prompt.slice(0, 80);
    const { output, timedOut } = waitForPrompt(sessionName, timeoutMs, marker);

    const latencyMs = performance.now() - start;

    if (timedOut) {
      // Send Escape to cancel, then grab whatever we have
      try {
        execSync(`tmux send-keys -t ${sessionName} Escape`, { timeout: 5_000 });
      } catch { /* session may be gone */ }
      execSync("sleep 3");
    }

    // Always capture the full pane as fallback
    const finalCapture = tmuxCapture(sessionName, 2000);
    const result = output.length > finalCapture.length ? output : finalCapture;

    return { output: result, latencyMs };
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
