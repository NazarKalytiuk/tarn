import * as vscode from "vscode";
import { WorkspaceIndex } from "./workspace/WorkspaceIndex";
import { createTarnTestController } from "./testing/TestController";
import { TestCodeLensProvider } from "./codelens/TestCodeLensProvider";
import { TarnDocumentSymbolProvider } from "./language/DocumentSymbolProvider";
import { TarnDiagnosticsProvider } from "./language/DiagnosticsProvider";
import { TarnCompletionProvider } from "./language/CompletionProvider";
import { TarnHoverProvider } from "./language/HoverProvider";
import {
  TarnDefinitionProvider,
  TarnReferencesProvider,
  TarnRenameProvider,
} from "./language/SymbolProviders";
import { TarnFormatProvider } from "./language/FormatProvider";
import { LastRunCache } from "./testing/LastRunCache";
import { RequestResponsePanel } from "./views/RequestResponsePanel";
import { TarnStatusBar } from "./statusBar";
import { registerCommands } from "./commands";
import { TarnProcessRunner } from "./backend/TarnProcessRunner";
import { promptInstallIfMissing } from "./backend/binaryResolver";
import { getOutputChannel, disposeOutputChannel } from "./outputChannel";
import { getExperimentalLspClient, readConfig } from "./config";
import { resolveTarnLspBinary } from "./lsp/tarnLspResolver";
import { startTarnLspClient } from "./lsp/client";
import type { LanguageClient } from "vscode-languageclient/node";
import { warnIfTarnOutdated } from "./version";
import {
  RunHistoryStore,
  RunHistoryTreeProvider,
} from "./views/RunHistoryView";
import { EnvironmentsView } from "./views/EnvironmentsView";
import { CapturesInspector } from "./views/CapturesInspector";
import { FixPlanView } from "./views/FixPlanView";
import { ReportWebview } from "./views/ReportWebview";
import { BenchRunnerPanel } from "./views/BenchRunnerPanel";
import { runImportHurl } from "./commands/importHurl";
import { runInitProject } from "./commands/initProject";
import { FailureNotifier } from "./notifications";
import { buildFailureMessages as buildFailureMessagesImpl } from "./testing/ResultMapper";
import type { TarnExtensionApi } from "./api";

export type { TarnExtensionApi } from "./api";

/**
 * Module-scoped handle on the experimental `tarn-lsp` language
 * client. Kept outside `activate()` so `deactivate()` can dispose
 * it without relying on a `context.subscriptions` round trip —
 * VS Code's `deactivate` contract is allowed to be async and
 * awaiting `client.stop()` here is the only way to guarantee the
 * `shutdown` / `exit` handshake drains before the extension host
 * tears down the child process.
 *
 * Intentionally not exposed on `TarnExtensionApi`: the NAZ-285
 * stable API surface promise locks `src/api.ts`, and the LSP
 * client is an internal implementation detail of the Phase V
 * dual-host migration.
 */
let tarnLspClient: LanguageClient | undefined;

export async function activate(
  context: vscode.ExtensionContext,
): Promise<TarnExtensionApi | undefined> {
  const output = getOutputChannel();
  // l10n-ignore: debug log with static prefix; not user-facing copy.
  output.appendLine("[tarn] activating");

  if (!vscode.workspace.isTrusted) {
    // l10n-ignore: debug log only, shown in Tarn output channel for diagnostics.
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

  // Check that the resolved Tarn CLI is at or above the extension's
  // declared `tarn.minVersion`. Non-fatal: a mismatch shows a warning
  // with an install link but activation continues so the user can
  // still browse files, edit, and format.
  if (resolved) {
    void warnIfTarnOutdated(context, binaryPath);
  }

  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const index = new WorkspaceIndex({ backend, cwd: workspaceRoot });
  await index.initialize();
  context.subscriptions.push(index);

  const history = new RunHistoryStore(context.workspaceState);
  const historyTree = new RunHistoryTreeProvider(history);
  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("tarn.runHistory", historyTree),
  );

  const lastRunCache = new LastRunCache();
  const stepDetailsPanel = new RequestResponsePanel(context.extensionUri);
  context.subscriptions.push(stepDetailsPanel);

  const capturesInspector = new CapturesInspector();
  context.subscriptions.push(
    capturesInspector,
    vscode.window.registerTreeDataProvider("tarn.captures", capturesInspector),
  );

  const fixPlanView = new FixPlanView(index);
  const fixPlanTree = vscode.window.createTreeView("tarn.fixPlan", {
    treeDataProvider: fixPlanView,
    showCollapseAll: true,
  });
  context.subscriptions.push(fixPlanView, fixPlanTree);

  // "Tarn view focused" = any of our activity-bar tree views is
  // currently visible. They all flip together when the user selects
  // the Tarn container, so checking one is enough — we use the Fix
  // Plan tree since it's the most relevant target for the
  // notification's "Show Fix Plan" action.
  const failureNotifier = new FailureNotifier(() => fixPlanTree.visible);

  const reportWebview = new ReportWebview(index);
  context.subscriptions.push(reportWebview);

  const benchRunnerPanel = new BenchRunnerPanel();
  context.subscriptions.push(benchRunnerPanel);

  const tarnController = createTarnTestController(
    index,
    backend,
    history,
    lastRunCache,
    capturesInspector,
    fixPlanView,
    failureNotifier,
    () => historyTree.refresh(),
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

  const diagnostics = new TarnDiagnosticsProvider(backend);
  context.subscriptions.push(diagnostics);

  const environmentsView = new EnvironmentsView(backend, tarnController.state);
  context.subscriptions.push(
    environmentsView,
    vscode.window.registerTreeDataProvider("tarn.environments", environmentsView),
  );

  const completionProvider = new TarnCompletionProvider(environmentsView);
  context.subscriptions.push(
    vscode.languages.registerCompletionItemProvider(
      { language: "tarn" },
      completionProvider,
      "{",
      ".",
      "$",
      " ",
    ),
  );

  const hoverProvider = new TarnHoverProvider(environmentsView);
  context.subscriptions.push(
    vscode.languages.registerHoverProvider({ language: "tarn" }, hoverProvider),
  );

  const formatProvider = new TarnFormatProvider(backend);
  context.subscriptions.push(
    vscode.languages.registerDefinitionProvider(
      { language: "tarn" },
      new TarnDefinitionProvider(environmentsView),
    ),
    vscode.languages.registerReferenceProvider(
      { language: "tarn" },
      new TarnReferencesProvider(),
    ),
    vscode.languages.registerRenameProvider(
      { language: "tarn" },
      new TarnRenameProvider(),
    ),
    vscode.languages.registerDocumentFormattingEditProvider(
      { language: "tarn" },
      formatProvider,
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
      environmentsView,
      lastRunCache,
      stepDetailsPanel,
      capturesInspector,
      fixPlanView,
      reportWebview,
      benchRunnerPanel,
      workspaceState: context.workspaceState,
      historyTree,
      refreshStatusBar: () => statusBar.refresh(),
      refreshHistoryView: () => historyTree.refresh(),
      refreshEnvironmentsView: () => environmentsView.refresh(),
    }),
  );

  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("tarn")) {
        statusBar.refresh();
      }
    }),
  );

  // Phase V1 (NAZ-309): experimental side-by-side `tarn-lsp` host.
  // The flag is off by default; turning it on spawns the LSP in
  // parallel with the direct providers. Any failure here is
  // advisory — the direct providers already cover every feature,
  // so we log, show a single warning, and continue.
  if (getExperimentalLspClient()) {
    try {
      const resolvedLsp = await resolveTarnLspBinary();
      tarnLspClient = await startTarnLspClient(context, resolvedLsp.path);
      // l10n-ignore: debug log with tarn-lsp prefix.
      output.appendLine(
        `[tarn-lsp] experimental client started (binary=${resolvedLsp.path})`,
      );
    } catch (err) {
      // l10n-ignore: debug log with tarn-lsp prefix.
      output.appendLine(
        `[tarn-lsp] experimental client failed to start: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
      void vscode.window.showWarningMessage(
        vscode.l10n.t(
          "Tarn experimental LSP client failed to start. Direct providers are still active. See the Tarn output channel for details.",
        ),
      );
    }
  }

  // l10n-ignore: debug log with tarn prefix; engineers read this in the output channel.
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
      "tarn.reloadEnvironments",
      "tarn.showStepDetails",
      "tarn.copyCaptureValue",
      "tarn.toggleHideCaptures",
      "tarn.jumpToFailure",
      "tarn.openHtmlReport",
      "tarn.benchStep",
      "tarn.importHurl",
      "tarn.pinHistoryEntry",
      "tarn.unpinHistoryEntry",
      "tarn.filterHistory",
      "tarn.rerunFromHistory",
    ],
    testing: {
      backend,
      validateDocument: async (uri: vscode.Uri) => {
        const doc = await vscode.workspace.openTextDocument(uri);
        await diagnostics.validate(doc);
      },
      reloadEnvironments: async () => {
        await environmentsView.reload();
      },
      listEnvironments: async () => environmentsView.getEntries(),
      getActiveEnvironment: () => tarnController.state.activeEnvironment,
      formatDocument: async (uri: vscode.Uri) => {
        const doc = await vscode.workspace.openTextDocument(uri);
        const cts = new vscode.CancellationTokenSource();
        try {
          const result = await formatProvider.provideDocumentFormattingEdits(
            doc,
            { tabSize: 2, insertSpaces: true },
            cts.token,
          );
          return result ?? [];
        } finally {
          cts.dispose();
        }
      },
      lastRunCacheSize: () => lastRunCache.size(),
      loadLastRunFromReport: (report) => lastRunCache.loadFromReport(report),
      showStepDetails: (key) => {
        const snapshot = lastRunCache.get(key);
        if (!snapshot) return false;
        stepDetailsPanel.show(snapshot);
        return true;
      },
      loadCapturesFromReport: (report) => capturesInspector.loadFromReport(report),
      capturesTotalCount: () => capturesInspector.totalCaptureCount(),
      isCaptureKeyRedacted: (key) => capturesInspector.isKeyRedacted(key),
      isHidingAllCaptures: () => capturesInspector.isHidingAllValues(),
      toggleHideCaptures: () => capturesInspector.toggleHideAllValues(),
      loadFixPlanFromReport: (report) => fixPlanView.loadFromReport(report),
      fixPlanSnapshot: () => fixPlanView.snapshot(),
      showReportHtml: (html) => reportWebview.show(html),
      sendReportMessage: (message) => reportWebview.handleMessage(message),
      showBenchResult: (context) => benchRunnerPanel.show(context),
      lastBenchContext: () => benchRunnerPanel.lastContext(),
      importHurl: (source, dest, cwd) =>
        runImportHurl(backend, source, dest, cwd),
      initProject: (options) => runInitProject({ backend }, options),
      history: {
        add: (entry) => history.add(entry),
        all: () => history.all(),
        clear: () => history.clear(),
        setFilter: (filter) => historyTree.setFilter(filter),
        getFilter: () => historyTree.getFilter(),
      },
      notifier: {
        isTarnViewFocused: () => fixPlanTree.visible,
        wouldNotify: (report, options) =>
          failureNotifier.wouldNotify(report, options),
        maybeNotify: (report, options) =>
          failureNotifier.maybeNotify(report, options),
      },
      workspaceIndexSnapshot: () =>
        index.all.map((parsed) => ({
          uri: parsed.uri.toString(),
          fileName: parsed.ranges.fileName,
          tests: parsed.ranges.tests.map((t) => ({
            name: t.name,
            stepCount: t.steps.length,
          })),
          fromScopedList: parsed.fromScopedList === true,
        })),
      refreshSingleFile: (uri) => index.refreshSingleFile(uri),
      startExperimentalLspClient: async () => {
        // Integration-test only. Start the experimental LSP
        // client on demand regardless of the current
        // `tarn.experimentalLspClient` flag, so the integration
        // suite can drive the flag-enabled code path without
        // reloading the extension host mid-test.
        //
        // Skip if the scaffold is already running (prevents a
        // second child process from fighting the first over the
        // same document selector) OR if the binary cannot be
        // resolved. Both states resolve to `undefined` so the
        // test can skip gracefully.
        if (tarnLspClient) {
          return undefined;
        }
        let resolvedPath: string;
        try {
          const resolvedLspForTest = await resolveTarnLspBinary();
          resolvedPath = resolvedLspForTest.path;
        } catch {
          return undefined;
        }
        try {
          tarnLspClient = await startTarnLspClient(context, resolvedPath);
        } catch {
          tarnLspClient = undefined;
          return undefined;
        }
        if (!tarnLspClient) {
          return undefined;
        }
        const clientHandle = tarnLspClient;
        // `State.Running = 2` in `vscode-languageclient/node` —
        // pinned here as a numeric literal so the integration
        // test does not have to import the full language-client
        // module just to read one enum value. The unit test in
        // `tests/unit/lspClient.test.ts` pins the numeric value
        // against the live enum and fails loudly if upstream
        // ever renumbers it.
        const numericState = clientHandle.state as unknown as number;
        return {
          running: numericState === 2,
          state: numericState,
          dispose: async () => {
            try {
              await clientHandle.stop();
            } catch {
              /* ignore */
            }
            if (tarnLspClient === clientHandle) {
              tarnLspClient = undefined;
            }
          },
        };
      },
      buildFailureMessagesForStep: (step, fileUri, astFallback) => {
        // Synthesize a minimal ParsedFile. We deliberately do not pull
        // the real WorkspaceIndex entry so the test can feed in a
        // specific URI (e.g., a fixture outside the indexed workspace).
        const parsed = {
          uri: fileUri,
          ranges: {
            fileName: "(integration-test synthetic)",
            fileNameRange: undefined,
            tests: [],
            setup: [],
            teardown: [],
          },
        };
        // Synthesize a minimal TestItem with just the fields
        // buildFailureMessages reads (`range`). Using a plain object
        // matches the unit-test pattern and avoids the TestController
        // tree.
        const stepItem = { range: astFallback ?? undefined };
        return buildFailureMessagesImpl(
          step,
          stepItem as unknown as vscode.TestItem,
          parsed as unknown as import("./workspace/WorkspaceIndex").ParsedFile,
        );
      },
    },
  };
}

export async function deactivate(): Promise<void> {
  // Drain the `tarn-lsp` stdio handshake before the extension
  // host tears down the child process. `client.stop()` is a
  // best-effort shutdown — if the server is already dead we
  // swallow the error rather than blocking deactivate.
  if (tarnLspClient) {
    try {
      await tarnLspClient.stop();
    } catch {
      /* ignore: client was already stopped or failed to stop */
    } finally {
      tarnLspClient = undefined;
    }
  }
  disposeOutputChannel();
}
