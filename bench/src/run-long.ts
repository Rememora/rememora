import { readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { loadTaskSequence, loadCondition } from "./task-sequence.js";
import { runLongRun, runMatrix } from "./long-run.js";

const __dirname = dirname(fileURLToPath(import.meta.url));

async function main(): Promise<void> {
  const args = process.argv.slice(2).filter((a) => a !== "--");

  const { values } = parseArgs({
    args,
    options: {
      sequence: { type: "string", short: "s" },
      condition: { type: "string", short: "c" },
      conditions: { type: "string", short: "C" },
      timeout: { type: "string", short: "t" },
      "keep-db": { type: "boolean", default: false },
      "db-path": { type: "string" },
      quiet: { type: "boolean", default: false },
      matrix: { type: "boolean", default: false },
      repeats: { type: "string", short: "r" },
    },
    strict: true,
  });

  if (!values.sequence) {
    console.error(
      "Usage: pnpm --prefix bench eval:long -- --sequence <path> --condition <path> [--timeout <ms>] [--keep-db] [--repeats N]",
    );
    console.error(
      "       pnpm --prefix bench eval:matrix -- --sequence <path> --conditions <dir> [--timeout <ms>] [--repeats N]",
    );
    process.exit(1);
  }

  // Resolve paths relative to bench/ directory
  const benchDir = join(__dirname, "..");
  const sequencePath = values.sequence.startsWith("/")
    ? values.sequence
    : join(benchDir, values.sequence);

  const sequence = loadTaskSequence(sequencePath);
  const timeoutMs = values.timeout ? parseInt(values.timeout, 10) : 120_000;

  const repeats = values.repeats ? parseInt(values.repeats, 10) : 1;

  if (values.matrix || values.conditions) {
    // Matrix mode — run across all conditions in a directory
    const conditionsDir = values.conditions ?? join(benchDir, "conditions");
    const conditionsPath = conditionsDir.startsWith("/")
      ? conditionsDir
      : join(benchDir, conditionsDir);

    const conditionFiles = readdirSync(conditionsPath)
      .filter((f) => f.endsWith(".json"))
      .sort();

    if (conditionFiles.length === 0) {
      console.error(`No condition files found in ${conditionsPath}`);
      process.exit(1);
    }

    const conditions = conditionFiles.map((f) =>
      loadCondition(join(conditionsPath, f)),
    );

    for (let r = 0; r < repeats; r++) {
      if (repeats > 1) {
        console.log(`\n  ═══ Matrix repeat ${r + 1}/${repeats} ═══`);
      }
      await runMatrix(sequence, conditions, {
        timeoutMs,
        keepDb: values["keep-db"],
        quiet: values.quiet,
      });
    }
  } else if (values.condition) {
    // Single condition mode
    const conditionPath = values.condition.startsWith("/")
      ? values.condition
      : join(benchDir, values.condition);

    const condition = loadCondition(conditionPath);

    for (let r = 0; r < repeats; r++) {
      if (repeats > 1) {
        console.log(`\n  ═══ Repeat ${r + 1}/${repeats} ═══`);
      }
      await runLongRun(sequence, condition, {
        timeoutMs,
        keepDb: values["keep-db"],
        dbPath: values["db-path"],
        quiet: values.quiet,
      });
    }
  } else {
    console.error(
      "Must specify either --condition <path> or --conditions <dir> (or --matrix)",
    );
    process.exit(1);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
