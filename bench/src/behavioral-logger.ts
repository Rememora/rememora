// ---------------------------------------------------------------------------
// Behavioral Logger — extracts rememora CLI calls from agent output
// ---------------------------------------------------------------------------

/** A parsed rememora CLI call captured from agent output. */
export interface RemoraCall {
  /** The rememora subcommand (e.g., "save", "search", "session", "context"). */
  subcommand: string;
  /** The full command string as captured. */
  fullCommand: string;
  /** Whether this call was autonomous (not prompted by the user message). */
  autonomous: boolean;
}

/** Summary of rememora behavior during a task. */
export interface BehaviorSummary {
  /** All rememora save calls. */
  saves: RemoraCall[];
  /** All rememora search calls. */
  searches: RemoraCall[];
  /** All other rememora calls (context, session, etc.). */
  other: RemoraCall[];
  /** Count of autonomous saves. */
  autonomousSaveCount: number;
  /** Count of autonomous searches. */
  autonomousSearchCount: number;
  /** Total rememora calls. */
  totalCalls: number;
}

/**
 * Regex to match rememora CLI invocations.
 *
 * Captures the subcommand (save, search, session, context, etc.)
 * and the rest of the arguments.
 */
const REMEMORA_PATTERN = /rememora\s+(save|search|context|session|project|get|status|export|extract|relate|supersede|setup)\b/g;

/**
 * Keywords in the user message that indicate the user explicitly asked
 * for a save or search action.
 */
const SAVE_PROMPT_PATTERNS = [
  /\bsave\b/i,
  /\bremember\b/i,
  /\bstore\b/i,
  /\bpersist\b/i,
  /\brecord\b/i,
];

const SEARCH_PROMPT_PATTERNS = [
  /\bsearch\b/i,
  /\brecall\b/i,
  /\blook\s*up\b/i,
  /\bfind\b/i,
  /\bretrieve\b/i,
  /\bcheck\s+memory\b/i,
];

/** Check if the user message prompted for a save action. */
function userPromptedSave(userMessage: string): boolean {
  return SAVE_PROMPT_PATTERNS.some((p) => p.test(userMessage));
}

/** Check if the user message prompted for a search action. */
function userPromptedSearch(userMessage: string): boolean {
  return SEARCH_PROMPT_PATTERNS.some((p) => p.test(userMessage));
}

/**
 * Extract all rememora CLI calls from raw agent output.
 *
 * Parses both structured JSON output (tool_use blocks) and plain text
 * for maximum compatibility across CLI runners.
 */
export function extractRemoraCalls(
  rawOutput: string,
  commands: string[],
  userMessage: string,
): BehaviorSummary {
  const allCommands = new Set<string>();

  // Collect from structured commands first
  for (const cmd of commands) {
    if (cmd.includes("rememora")) {
      allCommands.add(cmd);
    }
  }

  // Also scan raw output for any rememora calls we might have missed
  for (const line of rawOutput.split("\n")) {
    const matches = line.match(/(?:^|\s)(rememora\s+\S+[^\n]*)/g);
    if (matches) {
      for (const m of matches) {
        const trimmed = m.trim();
        if (trimmed.startsWith("rememora")) {
          allCommands.add(trimmed);
        }
      }
    }
  }

  const prompted_save = userPromptedSave(userMessage);
  const prompted_search = userPromptedSearch(userMessage);

  const saves: RemoraCall[] = [];
  const searches: RemoraCall[] = [];
  const other: RemoraCall[] = [];

  for (const cmd of allCommands) {
    // Reset regex lastIndex since we're reusing it
    REMEMORA_PATTERN.lastIndex = 0;
    const match = REMEMORA_PATTERN.exec(cmd);
    if (!match) continue;

    const subcommand = match[1];

    if (subcommand === "save") {
      saves.push({
        subcommand,
        fullCommand: cmd,
        autonomous: !prompted_save,
      });
    } else if (subcommand === "search") {
      searches.push({
        subcommand,
        fullCommand: cmd,
        autonomous: !prompted_search,
      });
    } else {
      other.push({
        subcommand,
        fullCommand: cmd,
        // context/session calls during multi-session runs are generally
        // part of the expected workflow, mark as non-autonomous
        autonomous: false,
      });
    }
  }

  return {
    saves,
    searches,
    other,
    autonomousSaveCount: saves.filter((s) => s.autonomous).length,
    autonomousSearchCount: searches.filter((s) => s.autonomous).length,
    totalCalls: saves.length + searches.length + other.length,
  };
}
