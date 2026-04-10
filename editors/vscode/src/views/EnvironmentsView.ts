import * as path from "path";
import * as vscode from "vscode";
import type { TarnBackend } from "../backend/TarnBackend";
import type { EnvEntry, EnvReport } from "../util/schemaGuards";
import type { RunState } from "../testing/runHandler";
import { getOutputChannel } from "../outputChannel";

/**
 * Tree data provider backed by `tarn env --json`.
 *
 * Loads once at activation and refreshes on `tarn.config.yaml` changes
 * or when the active environment changes. Keeps its own cache of the
 * last successful report so commands (`tarn.selectEnvironment`,
 * `tarn.openEnvironmentSource`) can read the same data without
 * re-spawning the backend.
 */
export class EnvironmentsView
  implements vscode.TreeDataProvider<EnvNode>, vscode.Disposable
{
  private readonly emitter = new vscode.EventEmitter<EnvNode | undefined>();
  readonly onDidChangeTreeData = this.emitter.event;

  private readonly disposables: vscode.Disposable[] = [];
  private cache: EnvReport | undefined;
  private loadErrors: string[] = [];
  private loadInFlight: Promise<void> | undefined;

  constructor(
    private readonly backend: TarnBackend,
    private readonly state: RunState,
  ) {
    const folder = vscode.workspace.workspaceFolders?.[0];
    if (folder) {
      const watcher = vscode.workspace.createFileSystemWatcher(
        new vscode.RelativePattern(folder, "tarn.config.yaml"),
      );
      watcher.onDidCreate(() => void this.reload());
      watcher.onDidChange(() => void this.reload());
      watcher.onDidDelete(() => void this.reload());
      this.disposables.push(watcher);
    }

    void this.reload();
  }

  /** Returns the cached entries, triggering an initial load if needed. */
  async getEntries(): Promise<EnvEntry[]> {
    if (!this.cache && !this.loadInFlight) {
      await this.reload();
    } else if (this.loadInFlight) {
      await this.loadInFlight;
    }
    return this.cache?.environments ?? [];
  }

  /** Look up a single environment by name in the current cache. */
  findByName(name: string): EnvEntry | undefined {
    return this.cache?.environments.find((env) => env.name === name);
  }

  /** Re-spawn `tarn env --json` and update the cache. */
  async reload(): Promise<void> {
    if (this.loadInFlight) {
      return this.loadInFlight;
    }
    const folder = vscode.workspace.workspaceFolders?.[0];
    if (!folder) {
      this.cache = undefined;
      this.loadErrors = ["No workspace folder available"];
      this.emitter.fire(undefined);
      return;
    }
    const cts = new vscode.CancellationTokenSource();
    this.loadInFlight = (async () => {
      try {
        const report = await this.backend.envStructured(folder.uri.fsPath, cts.token);
        if (!report) {
          // Non-fatal: projects without a tarn.config.yaml fall into
          // this branch. Keep the cache empty and show an explanatory
          // placeholder node.
          this.cache = { environments: [] };
          this.loadErrors = ["No environments returned by `tarn env --json`."];
        } else {
          this.cache = report;
          this.loadErrors = [];
        }
      } catch (err) {
        getOutputChannel().appendLine(
          `[tarn] EnvironmentsView reload failed: ${String(err)}`,
        );
        this.cache = { environments: [] };
        this.loadErrors = [String(err)];
      } finally {
        cts.dispose();
        this.loadInFlight = undefined;
        this.emitter.fire(undefined);
      }
    })();
    return this.loadInFlight;
  }

  /** External refresh hook — called after the active env changes. */
  refresh(): void {
    this.emitter.fire(undefined);
  }

  getTreeItem(element: EnvNode): vscode.TreeItem {
    if (element.kind === "entry") {
      const isActive = this.state.activeEnvironment === element.entry.name;
      const icon = isActive ? "$(check) " : "";
      const item = new vscode.TreeItem(
        `${icon}${element.entry.name}`,
        vscode.TreeItemCollapsibleState.None,
      );
      const varCount = Object.keys(element.entry.vars).length;
      item.description = `${element.entry.source_file} · ${varCount} vars`;
      item.contextValue = isActive ? "tarnEnvEntryActive" : "tarnEnvEntry";
      item.tooltip = this.renderTooltip(element.entry, isActive);
      item.command = {
        command: "tarn.setEnvironmentFromTree",
        title: "Set Active",
        arguments: [element.entry.name],
      };
      item.iconPath = new vscode.ThemeIcon(isActive ? "pass" : "symbol-variable");
      return item;
    }
    if (element.kind === "placeholder") {
      const item = new vscode.TreeItem(
        element.message,
        vscode.TreeItemCollapsibleState.None,
      );
      item.contextValue = "tarnEnvPlaceholder";
      return item;
    }
    throw new Error("unreachable");
  }

  getChildren(element?: EnvNode): vscode.ProviderResult<EnvNode[]> {
    if (element) {
      return [];
    }
    if (!this.cache) {
      return [{ kind: "placeholder", message: "Loading environments…" }];
    }
    if (this.cache.environments.length === 0) {
      const hint = this.loadErrors.length > 0 ? this.loadErrors[0] : undefined;
      return [
        {
          kind: "placeholder",
          message: hint
            ? `No environments configured (${hint})`
            : "No environments configured in tarn.config.yaml",
        },
      ];
    }
    return this.cache.environments.map<EnvNode>((entry) => ({
      kind: "entry",
      entry,
    }));
  }

  private renderTooltip(entry: EnvEntry, isActive: boolean): vscode.MarkdownString {
    const lines: string[] = [];
    lines.push(`**${entry.name}**${isActive ? " (active)" : ""}`);
    lines.push(`Source: \`${entry.source_file}\``);
    const keys = Object.keys(entry.vars);
    if (keys.length === 0) {
      lines.push("No inline vars");
    } else {
      lines.push(`Inline vars (${keys.length}):`);
      for (const key of keys.sort()) {
        const value = entry.vars[key];
        lines.push(`- \`${key}\`: \`${value}\``);
      }
    }
    const md = new vscode.MarkdownString(lines.join("\n\n"));
    md.isTrusted = false;
    md.supportHtml = false;
    return md;
  }

  dispose(): void {
    for (const d of this.disposables) {
      d.dispose();
    }
    this.emitter.dispose();
  }
}

export type EnvNode =
  | { kind: "entry"; entry: EnvEntry }
  | { kind: "placeholder"; message: string };

/** Convenience helper the extension exposes on its test API. */
export function resolveEnvSourceUri(
  folder: vscode.WorkspaceFolder,
  entry: EnvEntry,
): vscode.Uri {
  if (path.isAbsolute(entry.source_file)) {
    return vscode.Uri.file(entry.source_file);
  }
  return vscode.Uri.joinPath(folder.uri, entry.source_file);
}
