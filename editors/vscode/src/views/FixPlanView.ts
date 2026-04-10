import * as path from "path";
import * as vscode from "vscode";
import type { Report, StepResult } from "../util/schemaGuards";
import type { ParsedFile, WorkspaceIndex } from "../workspace/WorkspaceIndex";

/**
 * Treeview that surfaces ranked remediation hints from the most
 * recent failing run, grouped by `failure_category`. Clicking a hint
 * jumps to the offending step's range in the editor.
 *
 * Data source: `remediation_hints` and `failure_category` on failed
 * step results in the final JSON report. When the MCP backend lands
 * (Phase 5) this view will prefer the `tarn_fix_plan` tool output,
 * but the fallback path stays load-bearing forever.
 */
export class FixPlanView
  implements vscode.TreeDataProvider<FixPlanNode>, vscode.Disposable
{
  private readonly emitter = new vscode.EventEmitter<FixPlanNode | undefined>();
  readonly onDidChangeTreeData = this.emitter.event;

  private groups: FixPlanGroup[] = [];

  constructor(private readonly index: WorkspaceIndex) {}

  /** Replace the view state with entries aggregated from a new report. */
  loadFromReport(report: Report): void {
    this.groups = flattenReportToPlan(report, (filePath) =>
      resolveParsedFile(filePath, this.index),
    );
    this.emitter.fire(undefined);
  }

  /** Total number of hint entries across every category. */
  totalEntryCount(): number {
    return this.groups.reduce((sum, g) => sum + g.entries.length, 0);
  }

  /** Exposed for tests — returns the raw group shape. */
  snapshot(): readonly FixPlanGroup[] {
    return this.groups;
  }

  clear(): void {
    this.groups = [];
    this.emitter.fire(undefined);
  }

  getTreeItem(element: FixPlanNode): vscode.TreeItem {
    if (element.kind === "placeholder") {
      const item = new vscode.TreeItem(
        element.message,
        vscode.TreeItemCollapsibleState.None,
      );
      item.contextValue = "tarnFixPlanPlaceholder";
      return item;
    }
    if (element.kind === "group") {
      const item = new vscode.TreeItem(
        humanizeCategory(element.category),
        vscode.TreeItemCollapsibleState.Expanded,
      );
      item.description = `${element.entries.length} hint${
        element.entries.length === 1 ? "" : "s"
      }`;
      item.iconPath = new vscode.ThemeIcon(iconForCategory(element.category));
      item.contextValue = "tarnFixPlanGroup";
      return item;
    }
    // entry
    const item = new vscode.TreeItem(
      element.hint,
      vscode.TreeItemCollapsibleState.None,
    );
    item.description = `${element.testName} / ${element.stepName}`;
    item.iconPath = new vscode.ThemeIcon("lightbulb");
    item.tooltip = renderTooltip(element);
    item.contextValue = "tarnFixPlanEntry";
    if (element.location) {
      item.command = {
        command: "tarn.jumpToFailure",
        title: "Jump to failure",
        arguments: [
          element.location.uri.toString(),
          serializeRange(element.location.range),
        ],
      };
    }
    return item;
  }

  getChildren(element?: FixPlanNode): vscode.ProviderResult<FixPlanNode[]> {
    if (!element) {
      if (this.groups.length === 0) {
        return [
          {
            kind: "placeholder",
            message:
              "No fix plan available. Run a test that fails to populate remediation hints here.",
          },
        ];
      }
      return this.groups.map<FixPlanNode>((group) => ({
        kind: "group",
        category: group.category,
        entries: group.entries,
      }));
    }
    if (element.kind === "group") {
      return element.entries.map<FixPlanNode>((entry) => ({
        kind: "entry",
        ...entry,
      }));
    }
    return [];
  }

  dispose(): void {
    this.emitter.dispose();
  }
}

export interface FixPlanEntry {
  category: string;
  testName: string;
  stepName: string;
  hint: string;
  /** Only set when the step's file is parsed by the workspace index. */
  location?: {
    uri: vscode.Uri;
    range: vscode.Range;
  };
}

export interface FixPlanGroup {
  category: string;
  entries: FixPlanEntry[];
}

export type FixPlanNode =
  | { kind: "placeholder"; message: string }
  | { kind: "group"; category: string; entries: FixPlanEntry[] }
  | ({ kind: "entry" } & FixPlanEntry);

/**
 * Walk a JSON report and produce a grouped fix plan. Exported for
 * unit tests; the view calls this with a real resolver. Steps that
 * passed are skipped. Failed steps contribute one entry per hint; a
 * step that failed without any remediation hints still shows up as a
 * single placeholder entry so the failure is visible in the view.
 */
export function flattenReportToPlan(
  report: Report,
  resolveFile: (filePath: string) => ParsedFile | undefined,
): FixPlanGroup[] {
  const groups = new Map<string, FixPlanEntry[]>();
  for (const file of report.files) {
    if (file.status !== "FAILED") continue;
    const parsed = resolveFile(file.file);
    for (const test of file.tests) {
      if (test.status !== "FAILED") continue;
      test.steps.forEach((step, stepIndex) => {
        if (step.status !== "FAILED") return;
        const category = step.failure_category ?? "unknown";
        const location = locationForStep(parsed, test.name, stepIndex);
        const hints = hintsForStep(step);
        const baseEntry: Omit<FixPlanEntry, "hint"> = {
          category,
          testName: test.name,
          stepName: step.name,
          location,
        };
        let bucket = groups.get(category);
        if (!bucket) {
          bucket = [];
          groups.set(category, bucket);
        }
        for (const hint of hints) {
          bucket.push({ ...baseEntry, hint });
        }
      });
    }
  }
  return Array.from(groups.entries())
    .map<FixPlanGroup>(([category, entries]) => ({ category, entries }))
    .sort((a, b) => categoryOrder(a.category) - categoryOrder(b.category));
}

function hintsForStep(step: StepResult): string[] {
  const hints = step.remediation_hints ?? [];
  if (hints.length > 0) {
    return hints;
  }
  // A failure with no remediation hints would otherwise disappear
  // from the view. Surface it with a minimal placeholder so users
  // can still click-through to the failing line.
  const code = step.error_code ? ` (${step.error_code})` : "";
  return [`No remediation hints available${code}.`];
}

function locationForStep(
  parsed: ParsedFile | undefined,
  testName: string,
  stepIndex: number,
): FixPlanEntry["location"] {
  if (!parsed) return undefined;
  const test = parsed.ranges.tests.find((t) => t.name === testName);
  const step = test?.steps.find((s) => s.index === stepIndex);
  if (!step) return undefined;
  return { uri: parsed.uri, range: step.nameRange };
}

/**
 * The report emits `file.file` as a workspace-relative path, but the
 * index keys by absolute `fsPath`. Try both to be resilient against
 * whichever form the runner used.
 */
function resolveParsedFile(
  filePath: string,
  index: WorkspaceIndex,
): ParsedFile | undefined {
  for (const parsed of index.all) {
    if (
      parsed.uri.fsPath === filePath ||
      parsed.uri.fsPath.endsWith(filePath) ||
      path.basename(parsed.uri.fsPath) === path.basename(filePath)
    ) {
      return parsed;
    }
  }
  return undefined;
}

function serializeRange(range: vscode.Range): [number, number, number, number] {
  return [
    range.start.line,
    range.start.character,
    range.end.line,
    range.end.character,
  ];
}

export function deserializeRange(
  raw: [number, number, number, number],
): vscode.Range {
  return new vscode.Range(
    new vscode.Position(raw[0], raw[1]),
    new vscode.Position(raw[2], raw[3]),
  );
}

function renderTooltip(entry: FixPlanEntry): vscode.MarkdownString {
  const lines = [
    `**${entry.stepName}** — ${entry.testName}`,
    `Category: \`${entry.category}\``,
    "",
    entry.hint,
  ];
  const md = new vscode.MarkdownString(lines.join("\n\n"));
  md.isTrusted = false;
  md.supportHtml = false;
  return md;
}

/**
 * Map failure categories to a display order. Lower numbers sort
 * first. Anything unknown falls to the bottom so new categories from
 * a future Tarn release still show up in the view.
 */
export function categoryOrder(category: string): number {
  const order: Record<string, number> = {
    assertion_failed: 0,
    capture_error: 1,
    unresolved_template: 2,
    parse_error: 3,
    connection_error: 4,
    timeout: 5,
  };
  return order[category] ?? 99;
}

export function humanizeCategory(category: string): string {
  switch (category) {
    case "assertion_failed":
      return "Assertion failed";
    case "capture_error":
      return "Capture error";
    case "unresolved_template":
      return "Unresolved template";
    case "parse_error":
      return "Parse error";
    case "connection_error":
      return "Connection error";
    case "timeout":
      return "Timeout";
    case "unknown":
      return "Other";
    default:
      return category;
  }
}

function iconForCategory(category: string): string {
  switch (category) {
    case "assertion_failed":
      return "error";
    case "capture_error":
      return "symbol-key";
    case "unresolved_template":
      return "symbol-variable";
    case "parse_error":
      return "bracket-error";
    case "connection_error":
      return "plug";
    case "timeout":
      return "clock";
    default:
      return "warning";
  }
}
