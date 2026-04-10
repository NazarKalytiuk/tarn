import * as vscode from "vscode";
import type { Report } from "../util/schemaGuards";

export interface RunHistoryEntry {
  id: string;
  timestamp: number;
  label: string;
  environment: string | null;
  tags: string[];
  status: "PASSED" | "FAILED" | "CANCELLED" | "ERRORED";
  passed: number;
  failed: number;
  total: number;
  durationMs: number;
  /** Workspace-relative paths of the files the run targeted. Used for rerun. */
  files: string[];
  /**
   * `FILE[::TEST[::STEP]]` selectors the run was launched with, or an
   * empty array for whole-file / whole-workspace runs. Used by
   * `Tarn: Rerun from History` to replay the exact subset.
   */
  selectors: string[];
  dryRun: boolean;
  /** Pinned entries are kept indefinitely and appear first in the view. */
  pinned: boolean;
}

/**
 * View-level filter applied when listing history entries. `kind`
 * drives the predicate; `value` is interpreted as an environment
 * name (`env`) or tag (`tag`) when the kind supports it.
 */
export interface RunHistoryFilter {
  kind: "all" | "passed" | "failed" | "env" | "tag";
  value?: string;
}

export const DEFAULT_HISTORY_FILTER: RunHistoryFilter = { kind: "all" };

const STORAGE_KEY = "tarn.runHistory";
const MAX_UNPINNED = 20;

export class RunHistoryStore {
  constructor(private readonly memento: vscode.Memento) {}

  /** Returns every entry with a stable pinned-first order. */
  all(): RunHistoryEntry[] {
    const raw = this.memento.get<RunHistoryEntry[]>(STORAGE_KEY, []);
    // Backward-compat: entries persisted before NAZ-276 lack the new
    // fields. Fill them in on read so the view never sees undefined.
    const normalized = raw.map(normalizeEntry);
    return sortEntries(normalized);
  }

  async add(entry: RunHistoryEntry): Promise<void> {
    const current = this.memento.get<RunHistoryEntry[]>(STORAGE_KEY, []);
    const normalized = current.map(normalizeEntry);
    normalized.unshift(normalizeEntry(entry));
    const trimmed = trimWithPinned(normalized, MAX_UNPINNED);
    await this.memento.update(STORAGE_KEY, trimmed);
  }

  async clear(): Promise<void> {
    // Clearing removes unpinned entries only. Pinned runs are kept
    // so users never lose a manually-marked-important run by hitting
    // the trash icon.
    const current = this.memento.get<RunHistoryEntry[]>(STORAGE_KEY, []);
    const kept = current.map(normalizeEntry).filter((e) => e.pinned);
    await this.memento.update(STORAGE_KEY, kept);
  }

  async pin(id: string): Promise<void> {
    await this.setPinned(id, true);
  }

  async unpin(id: string): Promise<void> {
    await this.setPinned(id, false);
  }

  findById(id: string): RunHistoryEntry | undefined {
    return this.all().find((e) => e.id === id);
  }

  private async setPinned(id: string, pinned: boolean): Promise<void> {
    const current = this.memento
      .get<RunHistoryEntry[]>(STORAGE_KEY, [])
      .map(normalizeEntry);
    const updated = current.map((e) => (e.id === id ? { ...e, pinned } : e));
    // After an unpin, the new-unpinned entry may push total unpinned
    // past the cap. Re-trim so the invariant holds.
    const trimmed = pinned ? updated : trimWithPinned(updated, MAX_UNPINNED);
    await this.memento.update(STORAGE_KEY, trimmed);
  }

  static entryFromReport(
    report: Report,
    args: {
      environment: string | null;
      tags: string[];
      files: string[];
      selectors: string[];
      dryRun: boolean;
    },
  ): RunHistoryEntry {
    return {
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      timestamp: Date.now(),
      label: `${report.summary.steps.passed}/${report.summary.steps.total} steps`,
      environment: args.environment,
      tags: args.tags,
      status: report.summary.status,
      passed: report.summary.steps.passed,
      failed: report.summary.steps.failed,
      total: report.summary.steps.total,
      durationMs: report.duration_ms,
      files: args.files,
      selectors: args.selectors,
      dryRun: args.dryRun,
      pinned: false,
    };
  }
}

/**
 * Apply the filter and the pinned-first sort order.
 *
 * Exported for unit tests so the predicate stays independently
 * testable without instantiating a Memento.
 */
export function applyHistoryFilter(
  entries: readonly RunHistoryEntry[],
  filter: RunHistoryFilter,
): RunHistoryEntry[] {
  return entries.filter((entry) => historyFilterPredicate(entry, filter));
}

export function historyFilterPredicate(
  entry: RunHistoryEntry,
  filter: RunHistoryFilter,
): boolean {
  switch (filter.kind) {
    case "all":
      return true;
    case "passed":
      return entry.status === "PASSED";
    case "failed":
      return entry.status === "FAILED" || entry.status === "ERRORED";
    case "env":
      return (entry.environment ?? "") === (filter.value ?? "");
    case "tag":
      if (!filter.value) return entry.tags.length > 0;
      return entry.tags.includes(filter.value);
  }
}

/**
 * Trim the entry list so the number of UNPINNED entries does not
 * exceed `maxUnpinned`. Pinned entries are never evicted.
 */
export function trimWithPinned(
  entries: RunHistoryEntry[],
  maxUnpinned: number,
): RunHistoryEntry[] {
  const pinned: RunHistoryEntry[] = [];
  const unpinned: RunHistoryEntry[] = [];
  for (const entry of entries) {
    if (entry.pinned) pinned.push(entry);
    else unpinned.push(entry);
  }
  const trimmedUnpinned = unpinned.slice(0, maxUnpinned);
  // Preserve original insertion order overall; the view-level sort
  // later hoists pinned to the top without reordering within each
  // partition, so we just reinterleave in the original positions.
  const kept = new Set<RunHistoryEntry>([...pinned, ...trimmedUnpinned]);
  return entries.filter((e) => kept.has(e));
}

function sortEntries(entries: RunHistoryEntry[]): RunHistoryEntry[] {
  const pinned = entries.filter((e) => e.pinned);
  const unpinned = entries.filter((e) => !e.pinned);
  return [...pinned, ...unpinned];
}

function normalizeEntry(raw: RunHistoryEntry): RunHistoryEntry {
  return {
    ...raw,
    selectors: raw.selectors ?? [],
    pinned: raw.pinned ?? false,
    files: raw.files ?? [],
    tags: raw.tags ?? [],
  };
}

export class RunHistoryTreeProvider
  implements vscode.TreeDataProvider<RunHistoryEntry | FileNode>
{
  private readonly emitter = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.emitter.event;
  private filter: RunHistoryFilter = DEFAULT_HISTORY_FILTER;

  constructor(private readonly store: RunHistoryStore) {}

  refresh(): void {
    this.emitter.fire();
  }

  setFilter(filter: RunHistoryFilter): void {
    this.filter = filter;
    this.emitter.fire();
  }

  getFilter(): RunHistoryFilter {
    return this.filter;
  }

  getTreeItem(element: RunHistoryEntry | FileNode): vscode.TreeItem {
    if ("file" in element) {
      const item = new vscode.TreeItem(
        element.file,
        vscode.TreeItemCollapsibleState.None,
      );
      item.resourceUri = vscode.Uri.file(element.file);
      item.command = {
        command: "vscode.open",
        title: "Open",
        arguments: [item.resourceUri],
      };
      return item;
    }
    const entry = element;
    const statusIcon =
      entry.status === "PASSED"
        ? "$(check)"
        : entry.status === "FAILED"
          ? "$(x)"
          : "$(alert)";
    const pinIcon = entry.pinned ? "$(pinned) " : "";
    const date = new Date(entry.timestamp).toLocaleTimeString();
    const label = `${pinIcon}${statusIcon} ${date} · ${entry.label}${
      entry.dryRun ? " (dry)" : ""
    }`;
    const item = new vscode.TreeItem(label, vscode.TreeItemCollapsibleState.Collapsed);
    item.tooltip = this.renderTooltip(entry);
    item.description = this.renderDescription(entry);
    item.id = entry.id;
    item.contextValue = entry.pinned ? "tarnRunEntryPinned" : "tarnRunEntry";
    return item;
  }

  getChildren(
    element?: RunHistoryEntry | FileNode,
  ): vscode.ProviderResult<(RunHistoryEntry | FileNode)[]> {
    if (!element) {
      return applyHistoryFilter(this.store.all(), this.filter);
    }
    if ("file" in element) {
      return [];
    }
    return element.files.map((file) => ({ file }));
  }

  private renderDescription(entry: RunHistoryEntry): string {
    const parts: string[] = [];
    if (entry.environment) {
      parts.push(entry.environment);
    }
    if (entry.tags.length > 0) {
      parts.push(entry.tags.join(","));
    }
    if (entry.selectors.length > 0) {
      parts.push(`${entry.selectors.length} selector${entry.selectors.length === 1 ? "" : "s"}`);
    }
    parts.push(`${(entry.durationMs / 1000).toFixed(1)}s`);
    return parts.join(" · ");
  }

  private renderTooltip(entry: RunHistoryEntry): string {
    const lines = [
      `Status: ${entry.status}`,
      `Passed: ${entry.passed}/${entry.total}`,
      `Duration: ${(entry.durationMs / 1000).toFixed(2)}s`,
    ];
    if (entry.pinned) lines.push("📌 Pinned");
    if (entry.environment) {
      lines.push(`Env: ${entry.environment}`);
    }
    if (entry.tags.length > 0) {
      lines.push(`Tags: ${entry.tags.join(", ")}`);
    }
    if (entry.selectors.length > 0) {
      lines.push(`Selectors:`);
      for (const sel of entry.selectors.slice(0, 6)) lines.push(`  ${sel}`);
      if (entry.selectors.length > 6) {
        lines.push(`  …and ${entry.selectors.length - 6} more`);
      }
    }
    if (entry.dryRun) {
      lines.push("Dry run");
    }
    return lines.join("\n");
  }
}

interface FileNode {
  file: string;
}
