import * as vscode from "vscode";
import type { TarnTestController } from "../testing/TestController";
import type { WorkspaceIndex } from "../workspace/WorkspaceIndex";
import type { TarnBackend } from "../backend/TarnBackend";
import { getOutputChannel } from "../outputChannel";
import { ids } from "../testing/discovery";
import type { RunHistoryStore } from "../views/RunHistoryView";

export interface CommandDeps {
  tarnController: TarnTestController;
  index: WorkspaceIndex;
  backend: TarnBackend;
  history: RunHistoryStore;
  refreshStatusBar: () => void;
  refreshHistoryView: () => void;
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
      const envs = await collectEnvironments();
      type Pick = vscode.QuickPickItem & { value: string | null };
      const items: Pick[] = [
        { label: "$(close) (none)", description: "clear active environment", value: null },
        ...envs.map<Pick>((e) => ({ label: e, description: "", value: e })),
      ];
      const picked = await vscode.window.showQuickPick<Pick>(items, {
        placeHolder: "Select Tarn environment",
      });
      if (!picked) {
        return;
      }
      deps.tarnController.state.activeEnvironment = picked.value;
      deps.refreshStatusBar();
    }),
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
    vscode.commands.registerCommand("tarn.clearHistory", async () => {
      await deps.history.clear();
      deps.refreshHistoryView();
    }),
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

async function collectEnvironments(): Promise<string[]> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    return [];
  }
  const pattern = new vscode.RelativePattern(folder, "tarn.env.*.yaml");
  const uris = await vscode.workspace.findFiles(pattern);
  return uris
    .map((u) => {
      const base = u.path.split("/").pop() ?? "";
      const match = /^tarn\.env\.([A-Za-z0-9_\-]+)\.yaml$/.exec(base);
      return match?.[1] ?? "";
    })
    .filter((n) => n.length > 0 && n !== "local");
}
