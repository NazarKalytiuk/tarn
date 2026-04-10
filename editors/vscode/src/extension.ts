import * as vscode from "vscode";
import { WorkspaceIndex } from "./workspace/WorkspaceIndex";
import { createTarnTestController } from "./testing/TestController";
import { TestCodeLensProvider } from "./codelens/TestCodeLensProvider";
import { TarnDocumentSymbolProvider } from "./language/DocumentSymbolProvider";
import { TarnStatusBar } from "./statusBar";
import { registerCommands } from "./commands";
import { TarnProcessRunner } from "./backend/TarnProcessRunner";
import { promptInstallIfMissing } from "./backend/binaryResolver";
import { getOutputChannel, disposeOutputChannel } from "./outputChannel";
import { readConfig } from "./config";
import {
  RunHistoryStore,
  RunHistoryTreeProvider,
} from "./views/RunHistoryView";

export interface TarnExtensionApi {
  readonly testControllerId: string;
  readonly indexedFileCount: number;
  readonly commands: readonly string[];
  /** Opaque backend handle exposed for integration tests only. Do not use from production code. */
  readonly testing: {
    readonly backend: import("./backend/TarnBackend").TarnBackend;
  };
}

export async function activate(
  context: vscode.ExtensionContext,
): Promise<TarnExtensionApi | undefined> {
  const output = getOutputChannel();
  output.appendLine("[tarn] activating");

  if (!vscode.workspace.isTrusted) {
    output.appendLine("[tarn] workspace is untrusted; only passive features available");
    context.subscriptions.push(
      vscode.workspace.onDidGrantWorkspaceTrust(() => {
        vscode.commands.executeCommand("workbench.action.reloadWindow");
      }),
    );
    return;
  }

  const resolved = await promptInstallIfMissing();
  const binaryPath = resolved?.path ?? readConfig().binaryPath;
  const backend = new TarnProcessRunner(binaryPath);

  const index = new WorkspaceIndex();
  await index.initialize();
  context.subscriptions.push(index);

  const history = new RunHistoryStore(context.workspaceState);
  const historyTree = new RunHistoryTreeProvider(history);
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("tarn.runHistory", historyTree),
  );

  const tarnController = createTarnTestController(index, backend, history, () =>
    historyTree.refresh(),
  );
  context.subscriptions.push(tarnController);

  const codeLens = new TestCodeLensProvider(index);
  context.subscriptions.push(
    vscode.languages.registerCodeLensProvider({ language: "tarn" }, codeLens),
    codeLens,
  );

  context.subscriptions.push(
    vscode.languages.registerDocumentSymbolProvider(
      { language: "tarn" },
      new TarnDocumentSymbolProvider(index),
    ),
  );

  const statusBar = new TarnStatusBar(tarnController.state);
  context.subscriptions.push(statusBar);

  context.subscriptions.push(
    registerCommands({
      tarnController,
      index,
      backend,
      history,
      refreshStatusBar: () => statusBar.refresh(),
      refreshHistoryView: () => historyTree.refresh(),
    }),
  );

  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("tarn")) {
        statusBar.refresh();
      }
    }),
  );

  output.appendLine(
    `[tarn] ready (${index.all.length} test file(s) indexed)`,
  );

  return {
    testControllerId: tarnController.controller.id,
    indexedFileCount: index.all.length,
    commands: [
      "tarn.runAll",
      "tarn.runFile",
      "tarn.dryRunFile",
      "tarn.validateFile",
      "tarn.rerunLast",
      "tarn.runFailed",
      "tarn.selectEnvironment",
      "tarn.setTagFilter",
      "tarn.clearTagFilter",
      "tarn.showOutput",
      "tarn.installTarn",
      "tarn.exportCurl",
      "tarn.clearHistory",
      "tarn.showWalkthrough",
      "tarn.initProject",
      "tarn.refreshDiscovery",
    ],
    testing: { backend },
  };
}

export function deactivate(): void {
  disposeOutputChannel();
}
