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
import { readConfig } from "./config";
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

export interface TarnExtensionApi {
  readonly testControllerId: string;
  readonly indexedFileCount: number;
  readonly commands: readonly string[];
  /** Opaque handles exposed for integration tests only. Do not use from production code. */
  readonly testing: {
    readonly backend: import("./backend/TarnBackend").TarnBackend;
    readonly validateDocument: (uri: vscode.Uri) => Promise<void>;
    readonly reloadEnvironments: () => Promise<void>;
    readonly listEnvironments: () => Promise<
      ReadonlyArray<{ name: string; source_file: string; vars: Readonly<Record<string, string>> }>
    >;
    readonly getActiveEnvironment: () => string | null;
    readonly formatDocument: (uri: vscode.Uri) => Promise<vscode.TextEdit[]>;
    readonly lastRunCacheSize: () => number;
    readonly loadLastRunFromReport: (
      report: import("./util/schemaGuards").Report,
    ) => void;
    readonly showStepDetails: (key: import("./testing/LastRunCache").StepKey) => boolean;
    readonly loadCapturesFromReport: (
      report: import("./util/schemaGuards").Report,
    ) => void;
    readonly capturesTotalCount: () => number;
    readonly isCaptureKeyRedacted: (key: string) => boolean;
    readonly isHidingAllCaptures: () => boolean;
    readonly toggleHideCaptures: () => void;
    readonly loadFixPlanFromReport: (
      report: import("./util/schemaGuards").Report,
    ) => void;
    readonly fixPlanSnapshot: () => ReadonlyArray<
      import("./views/FixPlanView").FixPlanGroup
    >;
    readonly showReportHtml: (html: string) => void;
    readonly sendReportMessage: (message: unknown) => Promise<boolean>;
    readonly showBenchResult: (
      context: import("./views/BenchRunnerPanel").BenchRunContext,
    ) => void;
    readonly lastBenchContext: () =>
      | import("./views/BenchRunnerPanel").BenchRunContext
      | undefined;
    readonly importHurl: (
      source: string,
      dest: string,
      cwd: string,
    ) => Promise<{ success: boolean; exitCode: number | null; stderr: string }>;
    readonly initProject: (
      options: import("./commands/initProject").InitProjectOptions,
    ) => Promise<import("./commands/initProject").InitProjectOutcome>;
    readonly history: {
      readonly add: (
        entry: import("./views/RunHistoryView").RunHistoryEntry,
      ) => Promise<void>;
      readonly all: () => ReadonlyArray<
        import("./views/RunHistoryView").RunHistoryEntry
      >;
      readonly clear: () => Promise<void>;
      readonly setFilter: (
        filter: import("./views/RunHistoryView").RunHistoryFilter,
      ) => void;
      readonly getFilter: () =>
        import("./views/RunHistoryView").RunHistoryFilter;
    };
    readonly notifier: {
      readonly isTarnViewFocused: () => boolean;
      readonly wouldNotify: (
        report: import("./util/schemaGuards").Report,
        options: { dryRun: boolean },
      ) => boolean;
      readonly maybeNotify: (
        report: import("./util/schemaGuards").Report,
        options: { dryRun: boolean; files: string[] },
      ) => Promise<boolean>;
    };
    /**
     * Build VS Code `TestMessage`s for a failing step, honoring the
     * JSON-reported `location` metadata (Tarn T55) with an AST range
     * fallback. Exposed for integration tests that want to verify the
     * location resolution pipeline end-to-end against a real Tarn run.
     *
     * The `astFallback` parameter simulates the AST-derived
     * `stepItem.range` from discovery. Pass `null` to simulate a step
     * with no AST anchor at all.
     */
    readonly buildFailureMessagesForStep: (
      step: import("./util/schemaGuards").StepResult,
      fileUri: vscode.Uri,
      astFallback: vscode.Range | null,
    ) => vscode.TestMessage[];
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

export function deactivate(): void {
  disposeOutputChannel();
}
