import * as vscode from "vscode";
import type { WorkspaceIndex, ParsedFile } from "../workspace/WorkspaceIndex";
import { buildFileItem, rebuildChildren } from "./discovery";
import { createRunHandler, RunState } from "./runHandler";
import type { TarnBackend } from "../backend/TarnBackend";
import type { RunHistoryStore } from "../views/RunHistoryView";

export interface TarnTestController extends vscode.Disposable {
  controller: vscode.TestController;
  runProfile: vscode.TestRunProfile;
  dryRunProfile: vscode.TestRunProfile;
  state: RunState;
  refresh(): void;
  rerunLast(): Promise<void>;
}

export function createTarnTestController(
  index: WorkspaceIndex,
  backend: TarnBackend,
  history: RunHistoryStore,
  onHistoryChanged: () => void,
): TarnTestController {
  const controller = vscode.tests.createTestController("tarn", "Tarn");

  const state: RunState = {
    activeEnvironment: null,
    activeTags: [],
    lastRequest: undefined,
    lastDryRun: false,
    lastFailedItemIds: new Set(),
  };

  const deps = { controller, backend, index, state, history, onHistoryChanged };
  const runHandler = createRunHandler(deps, false);
  const dryRunHandler = createRunHandler(deps, true);

  const runProfile = controller.createRunProfile(
    "Run",
    vscode.TestRunProfileKind.Run,
    runHandler,
    true,
  );
  const dryRunProfile = controller.createRunProfile(
    "Dry Run",
    vscode.TestRunProfileKind.Run,
    dryRunHandler,
    false,
  );
  runProfile.isDefault = true;
  dryRunProfile.isDefault = false;

  const refresh = () => {
    const items: vscode.TestItem[] = [];
    for (const parsed of index.all) {
      items.push(buildFileItem(controller, parsed));
    }
    controller.items.replace(items);
  };

  const indexListener = index.onDidChange((uri, parsed) => {
    onIndexChange(controller, uri, parsed);
  });

  controller.refreshHandler = async () => {
    await index.initialize();
    refresh();
  };

  refresh();

  return {
    controller,
    runProfile,
    dryRunProfile,
    state,
    refresh,
    async rerunLast() {
      if (!state.lastRequest) {
        vscode.window.showInformationMessage("No previous Tarn run to repeat.");
        return;
      }
      const handler = state.lastDryRun ? dryRunHandler : runHandler;
      const token = new vscode.CancellationTokenSource().token;
      await handler(state.lastRequest, token);
    },
    dispose() {
      indexListener.dispose();
      runProfile.dispose();
      dryRunProfile.dispose();
      controller.dispose();
    },
  };
}

function onIndexChange(
  controller: vscode.TestController,
  uri: vscode.Uri,
  parsed: ParsedFile | undefined,
): void {
  const id = `file:${uri.toString()}`;
  if (!parsed) {
    controller.items.delete(id);
    return;
  }
  const existing = controller.items.get(id);
  if (existing) {
    rebuildChildren(controller, existing, parsed);
    existing.label = parsed.ranges.fileName;
    if (parsed.ranges.fileNameRange) {
      existing.range = parsed.ranges.fileNameRange;
    }
  } else {
    controller.items.add(buildFileItem(controller, parsed));
  }
}
