import * as vscode from "vscode";
import type { TarnTestController } from "../testing/TestController";
import type { WorkspaceIndex } from "../workspace/WorkspaceIndex";
import type { TarnBackend } from "../backend/TarnBackend";
import { getOutputChannel } from "../outputChannel";
import { ids } from "../testing/discovery";
import type {
  RunHistoryEntry,
  RunHistoryFilter,
  RunHistoryStore,
  RunHistoryTreeProvider,
} from "../views/RunHistoryView";
import { EnvironmentsView, resolveEnvSourceUri } from "../views/EnvironmentsView";
import type { LastRunCache, StepKey } from "../testing/LastRunCache";
import type { RequestResponsePanel } from "../views/RequestResponsePanel";
import type { CapturesInspector } from "../views/CapturesInspector";
import type { FixPlanView } from "../views/FixPlanView";
import { deserializeRange } from "../views/FixPlanView";
import type { ReportWebview } from "../views/ReportWebview";
import type { BenchRunnerPanel } from "../views/BenchRunnerPanel";
import { registerBenchCommand } from "./bench";
import { registerImportHurlCommand } from "./importHurl";
import { readConfig } from "../config";
import * as fs from "fs";
import * as path from "path";

export interface CommandDeps {
  tarnController: TarnTestController;
  index: WorkspaceIndex;
  backend: TarnBackend;
  history: RunHistoryStore;
  environmentsView: EnvironmentsView;
  lastRunCache: LastRunCache;
  stepDetailsPanel: RequestResponsePanel;
  capturesInspector: CapturesInspector;
  fixPlanView: FixPlanView;
  reportWebview: ReportWebview;
  benchRunnerPanel: BenchRunnerPanel;
  workspaceState: vscode.Memento;
  historyTree: RunHistoryTreeProvider;
  refreshStatusBar: () => void;
  refreshHistoryView: () => void;
  refreshEnvironmentsView: () => void;
}

export function registerCommands(deps: CommandDeps): vscode.Disposable {
  const registrations: vscode.Disposable[] = [];

  registrations.push(
    vscode.commands.registerCommand("tarn.runAll", async () => {
      const request = new vscode.TestRunRequest(
        undefined,
        undefined,
        deps.tarnController.runProfile,
      );
      await runViaProfile(request, deps.tarnController.runProfile);
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.runFile", async () => {
      await runActiveFile(false);
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.dryRunFile", async () => {
      await runActiveFile(true);
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.validateFile", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        return;
      }
      const folder = vscode.workspace.getWorkspaceFolder(editor.document.uri);
      if (!folder) {
        return;
      }
      const token = new vscode.CancellationTokenSource().token;
      const result = await deps.backend.validate(
        [editor.document.uri.fsPath],
        folder.uri.fsPath,
        token,
      );
      if (result.exitCode === 0) {
        vscode.window.showInformationMessage("Tarn: file is valid.");
      } else {
        const out = getOutputChannel();
        out.show(true);
        out.appendLine(result.stdout || result.stderr || "Tarn validation failed");
      }
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.rerunLast", async () => {
      await deps.tarnController.rerunLast();
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.runFailed", async () => {
      const failedIds = deps.tarnController.state.lastFailedItemIds;
      if (failedIds.size === 0) {
        vscode.window.showInformationMessage("Tarn: no failures from the last run.");
        return;
      }
      const items: vscode.TestItem[] = [];
      const visit = (item: vscode.TestItem) => {
        if (failedIds.has(item.id)) {
          items.push(item);
        }
        item.children.forEach(visit);
      };
      deps.tarnController.controller.items.forEach(visit);
      if (items.length === 0) {
        vscode.window.showInformationMessage(
          "Tarn: failed items from the last run are no longer present.",
        );
        return;
      }
      const request = new vscode.TestRunRequest(
        items,
        undefined,
        deps.tarnController.runProfile,
      );
      await runViaProfile(request, deps.tarnController.runProfile);
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.selectEnvironment", async () => {
      const entries = await deps.environmentsView.getEntries();
      type Pick = vscode.QuickPickItem & { value: string | null };
      const items: Pick[] = [
        { label: "$(close) (none)", description: "clear active environment", value: null },
        ...entries.map<Pick>((e) => ({
          label: e.name,
          description: e.source_file,
          detail: `${Object.keys(e.vars).length} inline vars`,
          value: e.name,
        })),
      ];
      if (items.length === 1) {
        vscode.window.showInformationMessage(
          "Tarn: no environments configured in tarn.config.yaml.",
        );
        return;
      }
      const picked = await vscode.window.showQuickPick<Pick>(items, {
        placeHolder: "Select Tarn environment",
      });
      if (!picked) {
        return;
      }
      deps.tarnController.state.activeEnvironment = picked.value;
      deps.refreshStatusBar();
      deps.refreshEnvironmentsView();
    }),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.setEnvironmentFromTree",
      async (envName: string | null) => {
        if (envName === deps.tarnController.state.activeEnvironment) {
          deps.tarnController.state.activeEnvironment = null;
        } else {
          deps.tarnController.state.activeEnvironment = envName;
        }
        deps.refreshStatusBar();
        deps.refreshEnvironmentsView();
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.openEnvironmentSource",
      async (envName: string) => {
        const entry = deps.environmentsView.findByName(envName);
        if (!entry) {
          vscode.window.showWarningMessage(`Tarn: no environment named '${envName}'.`);
          return;
        }
        const folder = vscode.workspace.workspaceFolders?.[0];
        if (!folder) {
          return;
        }
        const uri = resolveEnvSourceUri(folder, entry);
        try {
          const doc = await vscode.workspace.openTextDocument(uri);
          await vscode.window.showTextDocument(doc);
        } catch (err) {
          vscode.window.showWarningMessage(
            `Tarn: source file for '${envName}' not found at ${uri.fsPath}.`,
          );
          getOutputChannel().appendLine(
            `[tarn] openEnvironmentSource failed: ${String(err)}`,
          );
        }
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.copyEnvironmentAsFlag",
      async (envName: string) => {
        const entry = deps.environmentsView.findByName(envName);
        if (!entry) {
          return;
        }
        await vscode.env.clipboard.writeText(`--env ${entry.name}`);
        vscode.window.showInformationMessage(
          `Tarn: copied '--env ${entry.name}' to clipboard.`,
        );
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.reloadEnvironments", async () => {
      await deps.environmentsView.reload();
    }),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.copyCaptureValue",
      async (value: string, label?: string) => {
        if (typeof value !== "string") {
          return;
        }
        await vscode.env.clipboard.writeText(value);
        const hint = label ? ` (${label})` : "";
        vscode.window.setStatusBarMessage(
          `Tarn: copied capture value${hint}`,
          2000,
        );
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.toggleHideCaptures", () => {
      deps.capturesInspector.toggleHideAllValues();
    }),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.jumpToFailure",
      async (
        uriString: string,
        rangeRaw: [number, number, number, number],
      ) => {
        if (typeof uriString !== "string" || !Array.isArray(rangeRaw)) {
          return;
        }
        try {
          const uri = vscode.Uri.parse(uriString);
          const range = deserializeRange(rangeRaw);
          const doc = await vscode.workspace.openTextDocument(uri);
          await vscode.window.showTextDocument(doc, {
            selection: range,
            preserveFocus: false,
          });
        } catch (err) {
          getOutputChannel().appendLine(
            `[tarn] jumpToFailure failed: ${String(err)}`,
          );
        }
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.showStepDetails",
      async (arg?: StepKey | { encodedKey?: string }) => {
        let snapshot;
        if (arg && "encodedKey" in arg && typeof arg.encodedKey === "string") {
          snapshot = deps.lastRunCache.getByEncoded(arg.encodedKey);
        } else if (arg && typeof arg === "object" && "file" in arg) {
          snapshot = deps.lastRunCache.get(arg);
        }
        if (!snapshot) {
          vscode.window.showInformationMessage(
            "Tarn: no step details available. Run some tests first and click this command on a failing step.",
          );
          return;
        }
        deps.stepDetailsPanel.show(snapshot);
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.setTagFilter", async () => {
      const input = await vscode.window.showInputBox({
        prompt: "Comma-separated tag filter (leave empty to clear)",
        value: deps.tarnController.state.activeTags.join(","),
      });
      if (input === undefined) {
        return;
      }
      deps.tarnController.state.activeTags = input
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      deps.refreshStatusBar();
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.clearTagFilter", () => {
      deps.tarnController.state.activeTags = [];
      deps.refreshStatusBar();
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.showOutput", () => {
      getOutputChannel().show(true);
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.installTarn", async () => {
      await vscode.env.openExternal(
        vscode.Uri.parse("https://github.com/NazarKalytiuk/hive#install"),
      );
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.exportCurl", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        return;
      }
      const folder = vscode.workspace.getWorkspaceFolder(editor.document.uri);
      if (!folder) {
        return;
      }
      const mode = await vscode.window.showQuickPick(
        [
          { label: "All steps", description: "--format curl-all", value: "all" as const },
          {
            label: "Failed steps only",
            description: "--format curl",
            value: "failed" as const,
          },
        ],
        { placeHolder: "Export mode" },
      );
      if (!mode) {
        return;
      }
      const token = new vscode.CancellationTokenSource().token;
      const result = await deps.backend.exportCurl(
        [editor.document.uri.fsPath],
        folder.uri.fsPath,
        mode.value,
        token,
      );
      if (result.stdout.length === 0) {
        vscode.window.showInformationMessage("Tarn: nothing to export.");
        return;
      }
      const doc = await vscode.workspace.openTextDocument({
        language: "shellscript",
        content: result.stdout,
      });
      await vscode.window.showTextDocument(doc, { preview: false });
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.openHtmlReport", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showInformationMessage(
          "Tarn: open a .tarn.yaml file first to generate its HTML report.",
        );
        return;
      }
      const folder = vscode.workspace.getWorkspaceFolder(editor.document.uri);
      if (!folder) {
        vscode.window.showInformationMessage(
          "Tarn: no workspace folder for the active file.",
        );
        return;
      }
      const relFile = path.relative(folder.uri.fsPath, editor.document.uri.fsPath);
      const config = readConfig();
      const cts = new vscode.CancellationTokenSource();
      const out = getOutputChannel();
      const activeEnv = deps.tarnController.state.activeEnvironment;
      const activeTags = deps.tarnController.state.activeTags;
      const outcome = await vscode.window.withProgress(
        {
          location: vscode.ProgressLocation.Notification,
          title: "Tarn: generating HTML report…",
          cancellable: true,
        },
        async (_progress, token) => {
          token.onCancellationRequested(() => cts.cancel());
          return deps.backend.runHtmlReport({
            files: [relFile],
            cwd: folder.uri.fsPath,
            environment: activeEnv ?? config.defaultEnvironment,
            tags: activeTags.length > 0 ? activeTags : config.defaultTags,
            token: cts.token,
          });
        },
      );
      cts.dispose();
      if (!outcome.htmlPath) {
        vscode.window.showErrorMessage(
          `Tarn: HTML report not generated (exit ${outcome.exitCode}).`,
        );
        if (outcome.stderr) {
          out.appendLine(outcome.stderr);
          out.show(true);
        }
        return;
      }
      try {
        const html = await fs.promises.readFile(outcome.htmlPath, "utf8");
        const title = `Tarn Report — ${path.basename(relFile)}`;
        deps.reportWebview.show(html, title);
      } catch (err) {
        vscode.window.showErrorMessage(
          `Tarn: failed to read HTML report: ${String(err)}`,
        );
      } finally {
        // The HTML is self-contained and already loaded into the webview;
        // drop the tmp file immediately to avoid littering /tmp.
        fs.promises.unlink(outcome.htmlPath).catch(() => {});
      }
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.clearHistory", async () => {
      await deps.history.clear();
      deps.refreshHistoryView();
    }),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.pinHistoryEntry",
      async (entry: RunHistoryEntry) => {
        if (!entry?.id) return;
        await deps.history.pin(entry.id);
        deps.refreshHistoryView();
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.unpinHistoryEntry",
      async (entry: RunHistoryEntry) => {
        if (!entry?.id) return;
        await deps.history.unpin(entry.id);
        deps.refreshHistoryView();
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.filterHistory", async () => {
      await showHistoryFilterPicker(deps);
    }),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.rerunFromHistory",
      async (arg?: RunHistoryEntry | string) => {
        const entry =
          typeof arg === "string"
            ? deps.history.findById(arg)
            : arg && "id" in arg
              ? deps.history.findById(arg.id)
              : undefined;
        if (!entry) {
          vscode.window.showInformationMessage(
            "Tarn: this history entry is no longer available.",
          );
          return;
        }
        await rerunHistoryEntry(deps, entry);
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.showWalkthrough", async () => {
      await vscode.commands.executeCommand(
        "workbench.action.openWalkthrough",
        "nazarkalytiuk.tarn-vscode#tarn.gettingStarted",
        false,
      );
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.initProject", async () => {
      const folder = await pickInitFolder();
      if (!folder) {
        return;
      }
      const existing = await detectExistingScaffold(folder);
      if (existing) {
        const choice = await vscode.window.showWarningMessage(
          `The folder already contains '${existing}'. Running tarn init here will overwrite scaffold files.`,
          { modal: true },
          "Proceed",
        );
        if (choice !== "Proceed") {
          return;
        }
      }
      const token = new vscode.CancellationTokenSource().token;
      const out = getOutputChannel();
      out.show(true);
      out.appendLine(`[tarn] initializing project in ${folder.fsPath}`);
      const result = await deps.backend.initProject(folder.fsPath, token);
      if (result.stdout) {
        out.appendLine(result.stdout.trim());
      }
      if (result.stderr) {
        out.appendLine(result.stderr.trim());
      }
      if (result.exitCode !== 0) {
        vscode.window.showErrorMessage(
          `Tarn init failed (exit ${result.exitCode}). See output for details.`,
        );
        return;
      }
      const isCurrentWorkspace = vscode.workspace.workspaceFolders?.some(
        (f) => f.uri.fsPath === folder.fsPath,
      );
      if (isCurrentWorkspace) {
        await vscode.commands.executeCommand("tarn.refreshDiscovery");
        vscode.window.showInformationMessage("Tarn: project scaffolded in current workspace.");
      } else {
        const open = await vscode.window.showInformationMessage(
          "Tarn: project scaffolded. Open the folder?",
          "Open in New Window",
          "Open in Current Window",
        );
        if (open === "Open in New Window") {
          await vscode.commands.executeCommand("vscode.openFolder", folder, { forceNewWindow: true });
        } else if (open === "Open in Current Window") {
          await vscode.commands.executeCommand("vscode.openFolder", folder);
        }
      }
    }),
  );

  registrations.push(
    vscode.commands.registerCommand("tarn.refreshDiscovery", async () => {
      await deps.index.initialize();
      deps.tarnController.refresh();
    }),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.runTestFromCodeLens",
      async (itemId: string, dryRun: boolean) => {
        const item = findItemById(deps.tarnController.controller, itemId);
        if (!item) {
          return;
        }
        const profile = dryRun
          ? deps.tarnController.dryRunProfile
          : deps.tarnController.runProfile;
        const request = new vscode.TestRunRequest([item], undefined, profile);
        await runViaProfile(request, profile);
      },
    ),
  );

  registrations.push(
    vscode.commands.registerCommand(
      "tarn.dryRunTestFromCodeLens",
      async (itemId: string) => {
        await vscode.commands.executeCommand("tarn.runTestFromCodeLens", itemId, true);
      },
    ),
  );

  registrations.push(
    registerBenchCommand({
      backend: deps.backend,
      index: deps.index,
      panel: deps.benchRunnerPanel,
      tarnController: deps.tarnController,
      workspaceState: deps.workspaceState,
    }),
  );

  registrations.push(registerImportHurlCommand({ backend: deps.backend }));

  return vscode.Disposable.from(...registrations);

  async function runActiveFile(dryRun: boolean): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      return;
    }
    const parsed = deps.index.get(editor.document.uri);
    if (!parsed) {
      vscode.window.showInformationMessage(
        "Tarn: current file is not indexed as a Tarn test file.",
      );
      return;
    }
    const item = deps.tarnController.controller.items.get(ids.file(parsed.uri));
    if (!item) {
      return;
    }
    const profile = dryRun
      ? deps.tarnController.dryRunProfile
      : deps.tarnController.runProfile;
    const request = new vscode.TestRunRequest([item], undefined, profile);
    await runViaProfile(request, profile);
  }
}

async function runViaProfile(
  request: vscode.TestRunRequest,
  profile: vscode.TestRunProfile,
): Promise<void> {
  const cts = new vscode.CancellationTokenSource();
  try {
    await profile.runHandler(request, cts.token);
  } finally {
    cts.dispose();
  }
}

async function showHistoryFilterPicker(deps: CommandDeps): Promise<void> {
  const history = deps.history.all();
  const envs = Array.from(
    new Set(history.map((e) => e.environment).filter((e): e is string => !!e)),
  ).sort();
  const tagSet = new Set<string>();
  for (const entry of history) {
    for (const tag of entry.tags) tagSet.add(tag);
  }
  const tags = Array.from(tagSet).sort();

  type Item = vscode.QuickPickItem & { filter: RunHistoryFilter };
  const items: Item[] = [
    { label: "$(list-flat) All runs", filter: { kind: "all" } },
    { label: "$(check) Passed only", filter: { kind: "passed" } },
    { label: "$(x) Failed or errored", filter: { kind: "failed" } },
  ];
  for (const env of envs) {
    items.push({
      label: `$(symbol-variable) env · ${env}`,
      filter: { kind: "env", value: env },
    });
  }
  for (const tag of tags) {
    items.push({
      label: `$(tag) tag · ${tag}`,
      filter: { kind: "tag", value: tag },
    });
  }
  const picked = await vscode.window.showQuickPick(items, {
    placeHolder: "Filter Run History",
    matchOnDescription: true,
  });
  if (!picked) return;
  deps.historyTree.setFilter(picked.filter);
}

async function rerunHistoryEntry(
  deps: CommandDeps,
  entry: RunHistoryEntry,
): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    vscode.window.showInformationMessage("Tarn: no workspace folder available.");
    return;
  }

  // Restore the env + tags the original run used so the rerun
  // executes in the exact same context. Users can tweak the env
  // picker afterwards if they want something different.
  deps.tarnController.state.activeEnvironment = entry.environment;
  deps.tarnController.state.activeTags = [...entry.tags];
  deps.refreshStatusBar();

  const profile = entry.dryRun
    ? deps.tarnController.dryRunProfile
    : deps.tarnController.runProfile;
  const includes = resolveHistoryItems(deps, entry, folder.uri.fsPath);
  const request = new vscode.TestRunRequest(
    includes.length > 0 ? includes : undefined,
    undefined,
    profile,
  );
  await runViaProfile(request, profile);
}

function resolveHistoryItems(
  deps: CommandDeps,
  entry: RunHistoryEntry,
  workspaceRoot: string,
): vscode.TestItem[] {
  const controller = deps.tarnController.controller;
  const items: vscode.TestItem[] = [];

  // Per-selector runs: each selector points at a specific test or
  // step. Look up the TestItem by its stable id.
  if (entry.selectors.length > 0) {
    for (const selector of entry.selectors) {
      const parts = selector.split("::");
      if (parts.length < 2) continue;
      const relPath = parts[0];
      const testName = parts[1];
      const stepRaw = parts[2];
      const uri = vscode.Uri.file(path.resolve(workspaceRoot, relPath));
      const id =
        stepRaw !== undefined
          ? ids.step(uri, testName, Number(stepRaw))
          : ids.test(uri, testName);
      const item = findItemById(controller, id);
      if (item) items.push(item);
    }
    return items;
  }

  // File-scoped runs: include the file TestItem so the run handler
  // scopes to just that file without injecting selectors.
  if (entry.files.length > 0) {
    for (const relPath of entry.files) {
      const uri = vscode.Uri.file(path.resolve(workspaceRoot, relPath));
      const item = controller.items.get(ids.file(uri));
      if (item) items.push(item);
    }
    return items;
  }

  // Whole-workspace run: empty includes → the profile runs all
  // discovered files.
  return items;
}

function findItemById(
  controller: vscode.TestController,
  id: string,
): vscode.TestItem | undefined {
  let found: vscode.TestItem | undefined;
  const visit = (item: vscode.TestItem) => {
    if (found) {
      return;
    }
    if (item.id === id) {
      found = item;
      return;
    }
    item.children.forEach(visit);
  };
  controller.items.forEach(visit);
  return found;
}

async function pickInitFolder(): Promise<vscode.Uri | undefined> {
  const current = vscode.workspace.workspaceFolders ?? [];
  const items: (vscode.QuickPickItem & { value: "current" | "browse" | vscode.Uri })[] = [];
  for (const folder of current) {
    items.push({
      label: `$(folder) ${folder.name}`,
      description: folder.uri.fsPath,
      detail: "Use this workspace folder",
      value: folder.uri,
    });
  }
  items.push({
    label: "$(folder-opened) Browse…",
    description: "Pick another folder",
    value: "browse",
  });
  if (items.length === 0) {
    return undefined;
  }
  const picked = await vscode.window.showQuickPick(items, {
    placeHolder: "Where should Tarn init the new project?",
  });
  if (!picked) {
    return undefined;
  }
  if (picked.value === "browse") {
    const uris = await vscode.window.showOpenDialog({
      canSelectFiles: false,
      canSelectFolders: true,
      canSelectMany: false,
      openLabel: "Initialize Tarn here",
    });
    return uris?.[0];
  }
  if (picked.value === "current") {
    return undefined;
  }
  return picked.value;
}

async function detectExistingScaffold(folder: vscode.Uri): Promise<string | undefined> {
  const candidates = ["tarn.config.yaml", "tarn.env.yaml", "tests", "examples"];
  for (const name of candidates) {
    try {
      await vscode.workspace.fs.stat(vscode.Uri.joinPath(folder, name));
      return name;
    } catch {
      // not present, keep going
    }
  }
  return undefined;
}

