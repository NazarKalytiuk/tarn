import * as vscode from "vscode";
import type { BenchResult } from "../util/schemaGuards";

const PANEL_ID = "tarn.benchRunner";

/**
 * Singleton webview that renders a {@link BenchResult} from
 * `tarn bench --format json`. The layout is three blocks:
 *
 *   1. Summary header — throughput, total/successful/failed, error rate.
 *   2. Latency panel — min/mean/median/max + CSS-width bars for
 *      p50/p95/p99 so users can see the percentile spread at a glance.
 *   3. Details — status-code distribution, errors, gates, and the
 *      raw JSON in a pre block for copy-paste.
 *
 * There is deliberately no chart.js dependency: `tarn bench` does not
 * emit histogram buckets (only aggregate percentiles), so the only
 * data worth charting is p50/p95/p99, which four CSS bars render
 * cleanly without shipping a 200 KB external library.
 */
export class BenchRunnerPanel implements vscode.Disposable {
  private panel: vscode.WebviewPanel | undefined;
  private current: BenchRunContext | undefined;
  private readonly disposables: vscode.Disposable[] = [];

  show(context: BenchRunContext): void {
    this.current = context;
    if (!this.panel) {
      this.panel = vscode.window.createWebviewPanel(
        PANEL_ID,
        buildTitle(context),
        { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false },
        {
          enableScripts: false,
          retainContextWhenHidden: true,
          localResourceRoots: [],
        },
      );
      this.panel.onDidDispose(
        () => {
          this.panel = undefined;
          this.current = undefined;
        },
        undefined,
        this.disposables,
      );
    } else {
      this.panel.title = buildTitle(context);
      this.panel.reveal(vscode.ViewColumn.Beside, false);
    }
    this.panel.webview.html = renderHtml(context);
  }

  /** Exposed for tests — returns the last rendered context. */
  lastContext(): BenchRunContext | undefined {
    return this.current;
  }

  dispose(): void {
    for (const d of this.disposables) d.dispose();
    this.panel?.dispose();
    this.panel = undefined;
  }
}

export interface BenchRunContext {
  /** The parsed `tarn bench` JSON. */
  result: BenchResult;
  /** The workspace-relative file this run targeted. */
  file: string;
  /** The containing test name, if the user selected a named test. */
  testName?: string;
}

function buildTitle(context: BenchRunContext): string {
  const label = context.testName
    ? `${context.testName} / ${context.result.step_name}`
    : context.result.step_name;
  return `Tarn Bench: ${label}`;
}

function renderHtml(context: BenchRunContext): string {
  const { result, file, testName } = context;
  const maxPercentile = Math.max(
    result.latency.median_ms,
    result.latency.p95_ms,
    result.latency.p99_ms,
    1, // avoid divide-by-zero when everything is sub-millisecond
  );
  const statusRows = Object.entries(result.status_codes ?? {})
    .sort(([a], [b]) => Number(a) - Number(b))
    .map(
      ([code, count]) =>
        `<tr><td class="mono">${escapeHtml(code)}</td><td class="mono">${count}</td></tr>`,
    )
    .join("");
  const errorList =
    result.errors && result.errors.length > 0
      ? `<ul>${result.errors
          .slice(0, 20)
          .map((e) => `<li class="mono">${escapeHtml(e)}</li>`)
          .join("")}${
          result.errors.length > 20
            ? `<li>…and ${result.errors.length - 20} more</li>`
            : ""
        }</ul>`
      : '<div class="muted">No errors</div>';
  const gates = result.gates ?? [];
  const gateRows = gates
    .map((gate) => {
      const iconClass = gate.passed ? "ok" : "bad";
      const icon = gate.passed ? "✓" : "✗";
      return `<tr>
        <td class="icon ${iconClass}">${icon}</td>
        <td class="mono">${escapeHtml(gate.name)}</td>
        <td class="mono">${escapeHtml(String(gate.threshold ?? ""))}</td>
        <td class="mono">${escapeHtml(String(gate.value ?? ""))}</td>
        <td>${escapeHtml(gate.message ?? "")}</td>
      </tr>`;
    })
    .join("");
  const errorRateClass = result.error_rate === 0 ? "ok" : "bad";
  const summaryClass = (result.passed_gates ?? result.error_rate === 0)
    ? "ok"
    : "bad";

  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>${escapeHtml(buildTitle(context))}</title>
<style>
  body {
    font-family: var(--vscode-font-family);
    font-size: var(--vscode-font-size);
    color: var(--vscode-foreground);
    background: var(--vscode-editor-background);
    padding: 16px 20px;
    line-height: 1.5;
  }
  .mono { font-family: var(--vscode-editor-font-family, monospace); }
  .muted { color: var(--vscode-descriptionForeground); }
  h1 { font-size: 1.25rem; margin: 0 0 4px; }
  h2 { font-size: 1rem; margin: 24px 0 8px; }
  .subtitle { color: var(--vscode-descriptionForeground); margin-bottom: 16px; }
  .subtitle .mono { color: var(--vscode-textPreformat-foreground); }
  .summary-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 12px;
    margin-bottom: 16px;
  }
  .summary-card {
    border: 1px solid var(--vscode-widget-border, var(--vscode-panel-border));
    border-radius: 4px;
    padding: 10px 12px;
  }
  .summary-card .label { font-size: 0.75rem; color: var(--vscode-descriptionForeground); text-transform: uppercase; letter-spacing: 0.05em; }
  .summary-card .value { font-family: var(--vscode-editor-font-family, monospace); font-size: 1.2rem; margin-top: 2px; }
  .ok { color: var(--vscode-testing-iconPassed, #3fb950); }
  .bad { color: var(--vscode-testing-iconFailed, #f85149); }
  .bars { display: flex; flex-direction: column; gap: 6px; margin-top: 8px; }
  .bar-row { display: grid; grid-template-columns: 60px 1fr 80px; gap: 12px; align-items: center; }
  .bar-label { font-family: var(--vscode-editor-font-family, monospace); font-size: 0.85rem; }
  .bar-track {
    background: var(--vscode-editorWidget-background, rgba(128,128,128,0.15));
    border-radius: 3px;
    height: 16px;
    position: relative;
    overflow: hidden;
  }
  .bar-fill {
    background: var(--vscode-progressBar-background, #3794ff);
    height: 100%;
    border-radius: 3px;
  }
  .bar-value { font-family: var(--vscode-editor-font-family, monospace); font-size: 0.85rem; text-align: right; }
  table { border-collapse: collapse; margin-top: 4px; min-width: 240px; }
  td, th { padding: 4px 10px 4px 0; text-align: left; vertical-align: top; }
  th { font-weight: 600; color: var(--vscode-descriptionForeground); }
  td.icon { width: 1.2rem; text-align: center; }
  pre {
    background: var(--vscode-editorWidget-background, rgba(128,128,128,0.08));
    border: 1px solid var(--vscode-widget-border, var(--vscode-panel-border));
    border-radius: 4px;
    padding: 12px;
    overflow-x: auto;
    margin: 8px 0 24px;
    white-space: pre;
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: 0.85rem;
  }
</style>
</head>
<body>
  <h1>${escapeHtml(buildTitle(context))}</h1>
  <div class="subtitle">
    <span class="mono">${escapeHtml(result.method)} ${escapeHtml(result.url)}</span>
    &middot; <span class="mono">${escapeHtml(file)}</span>
    ${testName ? `&middot; test <span class="mono">${escapeHtml(testName)}</span>` : ""}
  </div>

  <div class="summary-grid">
    <div class="summary-card">
      <div class="label">Throughput</div>
      <div class="value">${formatNumber(result.throughput_rps)} req/s</div>
    </div>
    <div class="summary-card">
      <div class="label">Total requests</div>
      <div class="value">${result.total_requests}</div>
    </div>
    <div class="summary-card">
      <div class="label">Succeeded</div>
      <div class="value ok">${result.successful}</div>
    </div>
    <div class="summary-card">
      <div class="label">Failed</div>
      <div class="value ${result.failed > 0 ? "bad" : ""}">${result.failed}</div>
    </div>
    <div class="summary-card">
      <div class="label">Error rate</div>
      <div class="value ${errorRateClass}">${formatPercent(result.error_rate)}</div>
    </div>
    <div class="summary-card">
      <div class="label">Wall-clock</div>
      <div class="value">${formatDuration(result.total_duration_ms)}</div>
    </div>
    <div class="summary-card">
      <div class="label">Concurrency</div>
      <div class="value">${result.concurrency}</div>
    </div>
    <div class="summary-card">
      <div class="label">Outcome</div>
      <div class="value ${summaryClass}">${
    result.passed_gates === false ? "gates failed" : "ok"
  }</div>
    </div>
  </div>

  <h2>Latency</h2>
  <div class="bars">
    ${renderBar("min", result.latency.min_ms, maxPercentile)}
    ${renderBar("mean", result.latency.mean_ms, maxPercentile)}
    ${renderBar("p50", result.latency.median_ms, maxPercentile)}
    ${renderBar("p95", result.latency.p95_ms, maxPercentile)}
    ${renderBar("p99", result.latency.p99_ms, maxPercentile)}
    ${renderBar("max", result.latency.max_ms, maxPercentile)}
  </div>
  <div class="muted" style="margin-top: 6px;">Std dev: ${formatNumber(
    result.latency.stdev_ms,
  )} ms</div>

  <h2>Status codes</h2>
  ${
    statusRows.length > 0
      ? `<table><thead><tr><th>Code</th><th>Count</th></tr></thead><tbody>${statusRows}</tbody></table>`
      : '<div class="muted">No status codes recorded</div>'
  }

  <h2>Errors</h2>
  ${errorList}

  ${
    gates.length > 0
      ? `<h2>Gates</h2>
         <table>
           <thead><tr><th></th><th>Name</th><th>Threshold</th><th>Value</th><th>Message</th></tr></thead>
           <tbody>${gateRows}</tbody>
         </table>`
      : ""
  }

  <h2>Raw JSON</h2>
  <pre>${escapeHtml(JSON.stringify(result, null, 2))}</pre>
</body>
</html>`;
}

export function renderBar(label: string, valueMs: number, maxMs: number): string {
  const pct = percentWidth(valueMs, maxMs);
  return `<div class="bar-row">
    <span class="bar-label">${escapeHtml(label)}</span>
    <div class="bar-track"><div class="bar-fill" style="width: ${pct}%"></div></div>
    <span class="bar-value">${formatNumber(valueMs)} ms</span>
  </div>`;
}

export function percentWidth(value: number, max: number): number {
  if (!Number.isFinite(value) || !Number.isFinite(max) || max <= 0) {
    return 0;
  }
  const pct = (value / max) * 100;
  if (pct < 0) return 0;
  if (pct > 100) return 100;
  // Clamp tiny but non-zero values to 2% so the bar remains visible.
  if (value > 0 && pct < 2) return 2;
  return Math.round(pct * 100) / 100;
}

export function formatNumber(value: number): string {
  if (!Number.isFinite(value)) return "–";
  if (value >= 100) return Math.round(value).toString();
  if (value >= 10) return value.toFixed(1);
  return value.toFixed(2);
}

export function formatPercent(rate: number): string {
  if (!Number.isFinite(rate)) return "–";
  return `${formatNumber(rate * 100)}%`;
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms)) return "–";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
