/**
 * generate-report.ts — Generates a self-contained HTML report from JSONL eval results.
 *
 * Usage:
 *   tsx src/generate-report.ts <sequence-id> [results-dir]
 *   tsx src/generate-report.ts instruction-mode-eval
 *   tsx src/generate-report.ts instruction-mode-eval ./results
 *
 * Reads all `longrun_<sequenceId>_*.jsonl` files, aggregates metrics,
 * and writes a single HTML file with comparison tables and charts.
 */

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

import type { LongRunEvalRow } from "./task-sequence.js";
import {
  compareConditions,
  type ComparisonReport,
  type ConditionSummary,
} from "./compare-conditions.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = join(__dirname, "..", "results");

// ---------------------------------------------------------------------------
// JSONL helpers
// ---------------------------------------------------------------------------

function readJsonlFile(path: string): LongRunEvalRow[] {
  const content = readFileSync(path, "utf-8");
  return content
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line) as LongRunEvalRow);
}

/** Load all rows grouped by condition, then by run (timestamp). */
function loadAllRuns(
  sequenceId: string,
  resultsDir: string,
): Map<string, Map<string, LongRunEvalRow[]>> {
  const prefix = `longrun_${sequenceId}_`;
  const files = readdirSync(resultsDir)
    .filter((f) => f.startsWith(prefix) && f.endsWith(".jsonl"))
    .sort();

  // condition -> timestamp -> rows
  const grouped = new Map<string, Map<string, LongRunEvalRow[]>>();

  for (const file of files) {
    const rows = readJsonlFile(join(resultsDir, file));
    for (const row of rows) {
      const cond = row.metadata.condition;
      const ts = row.metadata.timestamp;
      if (!grouped.has(cond)) grouped.set(cond, new Map());
      const byTs = grouped.get(cond)!;
      if (!byTs.has(ts)) byTs.set(ts, []);
      byTs.get(ts)!.push(row);
    }
  }

  return grouped;
}

// ---------------------------------------------------------------------------
// Per-task breakdown for detail tables
// ---------------------------------------------------------------------------

interface TaskDetail {
  taskId: string;
  taskDescription: string;
  completed: boolean;
  autonomousSaves: number;
  autonomousSearches: number;
  totalSaves: number;
  totalSearches: number;
  kbStart: number;
  kbEnd: number;
  latencyMs: number;
  saves: string[];
  searches: string[];
}

function getLatestRunDetails(
  runs: Map<string, LongRunEvalRow[]>,
): TaskDetail[] {
  // Pick the latest run (last timestamp)
  const timestamps = Array.from(runs.keys()).sort();
  const latestTs = timestamps[timestamps.length - 1];
  const rows = runs.get(latestTs) ?? [];

  return rows
    .sort((a, b) => a.input.task_index - b.input.task_index)
    .map((row) => ({
      taskId: row.input.task_id,
      taskDescription: row.input.task,
      completed: row.scores.task_completion > 0,
      autonomousSaves: row.scores.autonomous_saves,
      autonomousSearches: row.scores.autonomous_searches,
      totalSaves: row.output.saves.length,
      totalSearches: row.output.searches.length,
      kbStart: row.scores.kb_size_at_start,
      kbEnd: row.scores.kb_size_at_end,
      latencyMs: row.metadata.latency_ms,
      saves: row.output.saves,
      searches: row.output.searches,
    }));
}

// ---------------------------------------------------------------------------
// HTML generation
// ---------------------------------------------------------------------------

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function bar(value: number, max: number, color: string): string {
  const pct = max > 0 ? Math.min((value / max) * 100, 100) : 0;
  return `<div class="bar-container"><div class="bar" style="width:${pct.toFixed(1)}%;background:${color}"></div><span class="bar-label">${value}</span></div>`;
}

function conditionLabel(id: string): string {
  const labels: Record<string, string> = {
    none: "None (baseline)",
    "reference-card": "Reference Card",
    "behavioral-triggers": "Behavioral Triggers",
    "hooks-only": "Hooks Only",
    "full-hybrid": "Full Hybrid",
    "full-hybrid-tmux": "Full Hybrid (tmux)",
    "none-tmux": "None (tmux)",
  };
  return labels[id] ?? id;
}

function conditionColor(id: string): string {
  const colors: Record<string, string> = {
    none: "#94a3b8",
    "reference-card": "#60a5fa",
    "behavioral-triggers": "#34d399",
    "hooks-only": "#fbbf24",
    "full-hybrid": "#a78bfa",
    "full-hybrid-tmux": "#c084fc",
    "none-tmux": "#cbd5e1",
  };
  return colors[id] ?? "#94a3b8";
}

function generateHtml(
  report: ComparisonReport,
  allRuns: Map<string, Map<string, LongRunEvalRow[]>>,
  sequenceId: string,
): string {
  const maxSaves = Math.max(...report.conditions.map((c) => c.autonomousSaves), 1);
  const maxSearches = Math.max(...report.conditions.map((c) => c.autonomousSearches), 1);
  const maxKb = Math.max(...report.conditions.map((c) => c.avgKbGrowthPerTask), 0.1);

  // Summary table rows
  const summaryRows = report.conditions
    .map((c) => {
      const color = conditionColor(c.condition);
      return `<tr>
        <td><span class="dot" style="background:${color}"></span>${escapeHtml(conditionLabel(c.condition))}</td>
        <td class="num">${c.runCount}</td>
        <td class="num">${c.tasksCompleted}/${c.totalTasks}</td>
        <td class="num">${(c.taskCompletionRate * 100).toFixed(0)}%</td>
        <td>${bar(c.autonomousSaves, maxSaves, "#34d399")}</td>
        <td>${bar(c.autonomousSearches, maxSearches, "#60a5fa")}</td>
        <td class="num">${c.promptedSaves}</td>
        <td class="num">${c.promptedSearches}</td>
        <td>${bar(c.avgKbGrowthPerTask, maxKb, "#a78bfa")}</td>
      </tr>`;
    })
    .join("\n");

  // Chart: autonomous saves comparison (horizontal bar)
  const savesChart = report.conditions
    .map((c) => {
      const color = conditionColor(c.condition);
      const pct = maxSaves > 0 ? (c.autonomousSaves / maxSaves) * 100 : 0;
      return `<div class="chart-row">
        <div class="chart-label">${escapeHtml(conditionLabel(c.condition))}</div>
        <div class="chart-bar-wrap"><div class="chart-bar" style="width:${pct.toFixed(1)}%;background:${color}">${c.autonomousSaves}</div></div>
      </div>`;
    })
    .join("\n");

  // Chart: autonomous searches comparison
  const searchesChart = report.conditions
    .map((c) => {
      const color = conditionColor(c.condition);
      const pct = maxSearches > 0 ? (c.autonomousSearches / maxSearches) * 100 : 0;
      return `<div class="chart-row">
        <div class="chart-label">${escapeHtml(conditionLabel(c.condition))}</div>
        <div class="chart-bar-wrap"><div class="chart-bar" style="width:${pct.toFixed(1)}%;background:${color}">${c.autonomousSearches}</div></div>
      </div>`;
    })
    .join("\n");

  // Per-condition detail sections
  const detailSections = report.conditions
    .map((c) => {
      const runs = allRuns.get(c.condition);
      if (!runs) return "";

      const details = getLatestRunDetails(runs);
      const color = conditionColor(c.condition);

      const taskRows = details
        .map((d) => {
          const statusIcon = d.completed ? "&#10003;" : "&#10007;";
          const statusClass = d.completed ? "pass" : "fail";
          const savesHtml = d.saves.length > 0
            ? `<details><summary>${d.totalSaves} save(s)</summary><pre class="cmd-list">${d.saves.map(escapeHtml).join("\n")}</pre></details>`
            : `<span class="muted">0 saves</span>`;
          const searchesHtml = d.searches.length > 0
            ? `<details><summary>${d.totalSearches} search(es)</summary><pre class="cmd-list">${d.searches.map(escapeHtml).join("\n")}</pre></details>`
            : `<span class="muted">0 searches</span>`;

          return `<tr>
            <td class="${statusClass}">${statusIcon}</td>
            <td>${escapeHtml(d.taskId)}</td>
            <td class="desc">${escapeHtml(d.taskDescription)}</td>
            <td class="num">${d.autonomousSaves}</td>
            <td class="num">${d.autonomousSearches}</td>
            <td class="num">${d.kbStart}&rarr;${d.kbEnd}</td>
            <td class="num">${(d.latencyMs / 1000).toFixed(1)}s</td>
            <td>${savesHtml}</td>
            <td>${searchesHtml}</td>
          </tr>`;
        })
        .join("\n");

      return `<section class="condition-detail">
        <h3><span class="dot" style="background:${color}"></span>${escapeHtml(conditionLabel(c.condition))}</h3>
        <p class="meta">${c.runCount} run(s) &middot; ${c.tasksCompleted}/${c.totalTasks} tasks completed &middot; completion rate: ${(c.taskCompletionRate * 100).toFixed(0)}%</p>
        <table class="detail-table">
          <thead>
            <tr>
              <th></th><th>Task</th><th>Description</th><th>A-Saves</th><th>A-Search</th><th>KB</th><th>Latency</th><th>Save Commands</th><th>Search Commands</th>
            </tr>
          </thead>
          <tbody>${taskRows}</tbody>
        </table>
      </section>`;
    })
    .join("\n");

  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Rememora Eval Report — ${escapeHtml(sequenceId)}</title>
<style>
  :root {
    --bg: #0f172a;
    --surface: #1e293b;
    --surface2: #334155;
    --text: #e2e8f0;
    --muted: #64748b;
    --accent: #818cf8;
    --green: #34d399;
    --blue: #60a5fa;
    --yellow: #fbbf24;
    --red: #f87171;
    --radius: 8px;
  }
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
    background: var(--bg);
    color: var(--text);
    line-height: 1.6;
    padding: 2rem;
    max-width: 1400px;
    margin: 0 auto;
  }
  h1 { font-size: 1.8rem; margin-bottom: 0.25rem; }
  h2 { font-size: 1.3rem; margin: 2rem 0 1rem; color: var(--accent); border-bottom: 1px solid var(--surface2); padding-bottom: 0.5rem; }
  h3 { font-size: 1.1rem; margin-bottom: 0.5rem; }
  .subtitle { color: var(--muted); font-size: 0.9rem; margin-bottom: 2rem; }
  .dot { display: inline-block; width: 10px; height: 10px; border-radius: 50%; margin-right: 8px; vertical-align: middle; }

  /* Summary table */
  table { width: 100%; border-collapse: collapse; margin-bottom: 1.5rem; }
  th { text-align: left; padding: 0.6rem 0.8rem; border-bottom: 2px solid var(--surface2); color: var(--muted); font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.05em; white-space: nowrap; }
  td { padding: 0.6rem 0.8rem; border-bottom: 1px solid var(--surface); vertical-align: middle; }
  tr:hover td { background: var(--surface); }
  .num { text-align: right; font-variant-numeric: tabular-nums; }
  .desc { max-width: 250px; font-size: 0.85rem; color: var(--muted); }
  .pass { color: var(--green); font-weight: bold; }
  .fail { color: var(--red); font-weight: bold; }
  .muted { color: var(--muted); font-size: 0.85rem; }

  /* Bar charts in tables */
  .bar-container { display: flex; align-items: center; gap: 8px; min-width: 120px; }
  .bar { height: 18px; border-radius: 3px; min-width: 2px; transition: width 0.3s; }
  .bar-label { font-size: 0.85rem; font-variant-numeric: tabular-nums; white-space: nowrap; }

  /* Horizontal bar charts */
  .chart { background: var(--surface); border-radius: var(--radius); padding: 1.2rem; margin-bottom: 1.5rem; }
  .chart-row { display: flex; align-items: center; margin-bottom: 0.6rem; }
  .chart-row:last-child { margin-bottom: 0; }
  .chart-label { width: 180px; font-size: 0.85rem; flex-shrink: 0; }
  .chart-bar-wrap { flex: 1; background: var(--surface2); border-radius: 4px; height: 24px; overflow: hidden; }
  .chart-bar { height: 100%; border-radius: 4px; display: flex; align-items: center; justify-content: flex-end; padding-right: 8px; font-size: 0.8rem; font-weight: 600; color: var(--bg); min-width: fit-content; transition: width 0.3s; }

  /* Detail sections */
  .condition-detail { background: var(--surface); border-radius: var(--radius); padding: 1.5rem; margin-bottom: 1.5rem; }
  .condition-detail .meta { color: var(--muted); font-size: 0.85rem; margin-bottom: 1rem; }
  .detail-table { font-size: 0.85rem; }
  .detail-table th { font-size: 0.75rem; }
  .detail-table td { padding: 0.4rem 0.6rem; }

  details { cursor: pointer; }
  details summary { color: var(--blue); font-size: 0.85rem; }
  .cmd-list { font-family: 'SF Mono', 'Fira Code', monospace; font-size: 0.75rem; background: var(--bg); padding: 0.6rem; border-radius: 4px; margin-top: 0.4rem; white-space: pre-wrap; word-break: break-all; max-height: 200px; overflow-y: auto; }

  /* Grid for charts */
  .charts-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 1.5rem; }
  @media (max-width: 900px) { .charts-grid { grid-template-columns: 1fr; } }

  .card { background: var(--surface); border-radius: var(--radius); padding: 1.2rem; }
  .card h3 { font-size: 0.95rem; margin-bottom: 1rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; }

  /* Stats row */
  .stats { display: flex; gap: 1.5rem; margin-bottom: 2rem; flex-wrap: wrap; }
  .stat { background: var(--surface); border-radius: var(--radius); padding: 1rem 1.5rem; flex: 1; min-width: 150px; }
  .stat-value { font-size: 2rem; font-weight: 700; }
  .stat-label { font-size: 0.8rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; }

  footer { margin-top: 3rem; padding-top: 1rem; border-top: 1px solid var(--surface2); color: var(--muted); font-size: 0.8rem; text-align: center; }
</style>
</head>
<body>

<h1>Rememora Eval Report</h1>
<p class="subtitle">Sequence: <strong>${escapeHtml(sequenceId)}</strong> &middot; Generated: ${escapeHtml(report.generatedAt)} &middot; Conditions: ${report.conditions.length}</p>

<div class="stats">
  <div class="stat">
    <div class="stat-value">${report.conditions.length}</div>
    <div class="stat-label">Conditions</div>
  </div>
  <div class="stat">
    <div class="stat-value">${report.conditions.reduce((s, c) => s + c.runCount, 0)}</div>
    <div class="stat-label">Total Runs</div>
  </div>
  <div class="stat">
    <div class="stat-value">${report.conditions.reduce((s, c) => s + c.autonomousSaves, 0)}</div>
    <div class="stat-label">Autonomous Saves</div>
  </div>
  <div class="stat">
    <div class="stat-value">${report.conditions.reduce((s, c) => s + c.autonomousSearches, 0)}</div>
    <div class="stat-label">Autonomous Searches</div>
  </div>
</div>

<h2>Condition Comparison</h2>
<table>
  <thead>
    <tr>
      <th>Condition</th><th>Runs</th><th>Completed</th><th>Rate</th><th>Autonomous Saves</th><th>Autonomous Searches</th><th>P-Saves</th><th>P-Search</th><th>KB/Task</th>
    </tr>
  </thead>
  <tbody>
    ${summaryRows}
  </tbody>
</table>

<h2>Autonomous Behavior Charts</h2>
<div class="charts-grid">
  <div class="card">
    <h3>Autonomous Saves by Condition</h3>
    ${savesChart}
  </div>
  <div class="card">
    <h3>Autonomous Searches by Condition</h3>
    ${searchesChart}
  </div>
</div>

<h2>Per-Condition Task Breakdown (Latest Run)</h2>
${detailSections}

<footer>
  Rememora Eval Benchmark &middot; <a href="https://github.com/Rememora/rememora" style="color:var(--accent)">github.com/Rememora/rememora</a>
</footer>

</body>
</html>`;
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

async function main(): Promise<void> {
  const sequenceId = process.argv[2];
  const resultsDir = process.argv[3] ?? RESULTS_DIR;

  if (!sequenceId) {
    console.error("Usage: tsx src/generate-report.ts <sequence-id> [results-dir]");
    console.error("Example: tsx src/generate-report.ts instruction-mode-eval");
    process.exit(1);
  }

  console.log(`\n  Generating HTML report for: ${sequenceId}`);

  const report = compareConditions(sequenceId, resultsDir);
  const allRuns = loadAllRuns(sequenceId, resultsDir);

  const html = generateHtml(report, allRuns, sequenceId);

  const ts = new Date().toISOString().replace(/[:.]/g, "-");
  const htmlPath = join(resultsDir, `report_${sequenceId}_${ts}.html`);
  writeFileSync(htmlPath, html);

  console.log(`  HTML report: ${htmlPath}`);
  console.log(`  Conditions: ${report.conditions.length}`);
  console.log(`  Total runs: ${report.conditions.reduce((s, c) => s + c.runCount, 0)}`);
  console.log();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
