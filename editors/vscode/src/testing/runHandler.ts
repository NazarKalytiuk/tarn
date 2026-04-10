import * as vscode from "vscode";
import * as path from "path";
import type { NdjsonEvent, TarnBackend } from "../backend/TarnBackend";
import type { WorkspaceIndex, ParsedFile } from "../workspace/WorkspaceIndex";
import { applyReport } from "./ResultMapper";
import { readConfig } from "../config";
import { getOutputChannel } from "../outputChannel";
import { RunHistoryStore } from "../views/RunHistoryView";
import { getItemMeta, ids } from "./discovery";
import type { LastRunCache } from "./LastRunCache";
import type { CapturesInspector } from "../views/CapturesInspector";
import type { FixPlanView } from "../views/FixPlanView";

export interface RunState {
  activeEnvironment: string | null;
  activeTags: string[];
  lastRequest: vscode.TestRunRequest | undefined;
  lastDryRun: boolean;
  /** Item IDs that failed in the last completed run. Used by Tarn: Run Failed. */
  lastFailedItemIds: Set<string>;
}

export interface HandlerDeps {
  controller: vscode.TestController;
  backend: TarnBackend;
  index: WorkspaceIndex;
  state: RunState;
  history: RunHistoryStore;
  lastRunCache: LastRunCache;
  capturesInspector: CapturesInspector;
  fixPlanView: FixPlanView;
  onHistoryChanged: () => void;
}

export function createRunHandler(
  deps: HandlerDeps,
  dryRun: boolean,
): (request: vscode.TestRunRequest, token: vscode.CancellationToken) => Promise<void> {
  return async (request, token) => {
    deps.state.lastRequest = request;
    deps.state.lastDryRun = dryRun;

    const run = deps.controller.createTestRun(request, dryRun ? "Tarn Dry Run" : "Tarn Run", true);
    try {
      await executeRun(deps, request, run, token, dryRun);
    } catch (err) {
      getOutputChannel().appendLine(`[tarn] run failed: ${String(err)}`);
      vscode.window.showErrorMessage(`Tarn run failed: ${String(err)}`);
    } finally {
      run.end();
    }
  };
}

async function executeRun(
  deps: HandlerDeps,
  request: vscode.TestRunRequest,
  run: vscode.TestRun,
  token: vscode.CancellationToken,
  dryRun: boolean,
): Promise<void> {
  const cwd = primaryWorkspaceFolder();
  if (!cwd) {
    run.appendOutput("No workspace folder found; cannot invoke tarn.\r\n");
    return;
  }

  const itemsById = collectAllTestItems(deps.controller);
  const parsedByPath = new Map<string, ParsedFile>();
  for (const parsed of deps.index.all) {
    parsedByPath.set(parsed.uri.fsPath, parsed);
  }

  const { filesToRun, selectors } = planRun(deps, request, cwd);
  if (filesToRun.length === 0) {
    run.appendOutput("No Tarn test files matched this run.\r\n");
    return;
  }

  for (const file of filesToRun) {
    enqueueFileItems(file, run, itemsById);
  }

  const config = readConfig();

  run.appendOutput(
    `[tarn] Running ${filesToRun.length} file(s)${dryRun ? " (dry run)" : ""}${
      selectors.length > 0 ? ` · selectors: ${selectors.length}` : ""
    }\r\n`,
  );

  // NDJSON streaming is the default. Users can opt out by not having the
  // flag (we always use it when available). Failure TestMessages still
  // come from the final JSON report for rich expected/actual/diff output.
  const streamed = new Set<string>();
  const streamFailed = new Set<string>();

  const outcome = await deps.backend.run({
    files: filesToRun.map((f) => path.relative(cwd, f.uri.fsPath)),
    cwd,
    environment: deps.state.activeEnvironment ?? config.defaultEnvironment,
    tags: deps.state.activeTags.length > 0 ? deps.state.activeTags : config.defaultTags,
    selectors: selectors.length > 0 ? selectors : undefined,
    parallel: config.parallel,
    jsonMode: config.jsonMode,
    dryRun,
    streamNdjson: true,
    token,
    onEvent: (event) => {
      handleNdjsonEvent(event, run, parsedByPath, itemsById, streamed, streamFailed);
    },
  });

  if (token.isCancellationRequested || outcome.cancelled) {
    run.appendOutput("[tarn] Run cancelled.\r\n");
    return;
  }

  if (!outcome.report) {
    run.appendOutput(
      `[tarn] Run did not produce a parseable JSON report (exit ${outcome.exitCode}).\r\n`,
    );
    if (outcome.stderr) {
      run.appendOutput(outcome.stderr);
    }
    markAllErrored(filesToRun, itemsById, run, outcome.stderr || "tarn produced no JSON report");
    return;
  }

  // Apply the final JSON report so failures get rich TestMessages (diff,
  // request, response, remediation hints). This re-marks items that the
  // NDJSON stream already touched; the final state wins.
  applyReport(outcome.report, {
    run,
    parsedByPath,
    testItemsById: itemsById,
  });

  // Stash the full report in the last-run cache so the Request/
  // Response Inspector webview can look up individual step details
  // via tarn.showStepDetails.
  deps.lastRunCache.loadFromReport(outcome.report);
  deps.capturesInspector.loadFromReport(outcome.report);
  deps.fixPlanView.loadFromReport(outcome.report);

  // Track which items failed so Tarn: Run Failed can target them later.
  deps.state.lastFailedItemIds = collectFailedItemIds(itemsById, outcome.report, parsedByPath);

  const summary = outcome.report.summary;
  run.appendOutput(
    `[tarn] Done. ${summary.steps.passed}/${summary.steps.total} steps passed across ${summary.files} file(s).\r\n`,
  );

  const entry = RunHistoryStore.entryFromReport(outcome.report, {
    environment: deps.state.activeEnvironment ?? config.defaultEnvironment,
    tags: deps.state.activeTags.length > 0 ? deps.state.activeTags : config.defaultTags,
    files: filesToRun.map((f) => path.relative(cwd, f.uri.fsPath)),
    selectors,
    dryRun,
  });
  await deps.history.add(entry);
  deps.onHistoryChanged();
}

interface RunPlan {
  filesToRun: ParsedFile[];
  selectors: string[];
}

function planRun(
  deps: HandlerDeps,
  request: vscode.TestRunRequest,
  cwd: string,
): RunPlan {
  const excluded = new Set((request.exclude ?? []).map((i) => i.id));
  const includes = request.include ?? [];

  if (includes.length === 0) {
    // Whole workspace — honor excludes only
    const filesToRun = deps.index.all.filter((parsed) => {
      return !excluded.has(ids.file(parsed.uri));
    });
    return { filesToRun, selectors: [] };
  }

  const fileSet = new Map<string, ParsedFile>();
  const selectors: string[] = [];

  for (const item of includes) {
    if (excluded.has(item.id)) {
      continue;
    }
    const meta = getItemMeta(item);
    if (!meta) {
      continue;
    }
    const parsed = deps.index.get(meta.uri);
    if (!parsed) {
      continue;
    }
    fileSet.set(parsed.uri.toString(), parsed);

    const relPath = path.relative(cwd, parsed.uri.fsPath);
    if (meta.kind === "test") {
      selectors.push(`${relPath}::${meta.testName}`);
    } else if (meta.kind === "step") {
      selectors.push(`${relPath}::${meta.testName}::${meta.stepIndex}`);
    }
    // kind === "file": no selector needed; the positional file arg is enough
  }

  return { filesToRun: Array.from(fileSet.values()), selectors };
}

function handleNdjsonEvent(
  event: NdjsonEvent,
  run: vscode.TestRun,
  parsedByPath: Map<string, ParsedFile>,
  itemsById: Map<string, vscode.TestItem>,
  streamed: Set<string>,
  streamFailed: Set<string>,
): void {
  switch (event.event) {
    case "file_started": {
      run.appendOutput(`[tarn] ▶ ${event.file}\r\n`);
      return;
    }
    case "step_finished": {
      if (event.phase !== "test") {
        // setup/teardown steps don't have discovered TestItems in this
        // phase's current shape; surface them as output lines.
        run.appendOutput(
          `[tarn]   ${statusSigil(event.status)} ${event.phase}: ${event.step} (${event.duration_ms}ms)\r\n`,
        );
        return;
      }
      const parsed = resolveParsedFromEventFile(event.file, parsedByPath);
      if (!parsed) {
        return;
      }
      const stepId = ids.step(parsed.uri, event.test, event.step_index);
      const item = itemsById.get(stepId);
      if (!item) {
        return;
      }
      run.started(item);
      run.appendOutput(
        `[tarn]   ${statusSigil(event.status)} ${event.test} / ${event.step} (${event.duration_ms}ms)\r\n`,
      );
      if (event.status === "PASSED") {
        run.passed(item, event.duration_ms);
        streamed.add(stepId);
      } else {
        // Defer failed state to the final report pass so the
        // TestMessage gets the rich expected/actual/diff payload.
        streamFailed.add(stepId);
      }
      return;
    }
    case "test_finished": {
      const parsed = resolveParsedFromEventFile(event.file, parsedByPath);
      if (!parsed) {
        return;
      }
      const testId = ids.test(parsed.uri, event.test);
      const item = itemsById.get(testId);
      if (!item) {
        return;
      }
      if (event.status === "PASSED") {
        run.passed(item, event.duration_ms);
        streamed.add(testId);
      }
      return;
    }
    case "file_finished": {
      const parsed = resolveParsedFromEventFile(event.file, parsedByPath);
      if (!parsed) {
        return;
      }
      const fileId = ids.file(parsed.uri);
      const item = itemsById.get(fileId);
      if (item && event.status === "PASSED") {
        run.passed(item, event.duration_ms);
        streamed.add(fileId);
      }
      return;
    }
    case "done": {
      run.appendOutput(
        `[tarn] ✓ ${event.summary.steps.passed}/${event.summary.steps.total} steps passed\r\n`,
      );
      return;
    }
  }
}

function statusSigil(status: "PASSED" | "FAILED"): string {
  return status === "PASSED" ? "✓" : "✗";
}

function resolveParsedFromEventFile(
  filePath: string,
  parsedByPath: Map<string, ParsedFile>,
): ParsedFile | undefined {
  // Tarn emits relative paths matching what we passed on the command line.
  // Try an exact match against fsPath entries first, then a suffix match.
  for (const [fsPath, parsed] of parsedByPath) {
    if (fsPath.endsWith(filePath) || filePath.endsWith(fsPath)) {
      return parsed;
    }
    if (path.basename(fsPath) === path.basename(filePath)) {
      return parsed;
    }
  }
  return undefined;
}

function collectFailedItemIds(
  itemsById: Map<string, vscode.TestItem>,
  report: import("../util/schemaGuards").Report,
  parsedByPath: Map<string, ParsedFile>,
): Set<string> {
  const failed = new Set<string>();
  for (const fileResult of report.files) {
    const parsed =
      parsedByPath.get(fileResult.file) ??
      Array.from(parsedByPath.values()).find((p) =>
        p.uri.fsPath.endsWith(fileResult.file),
      );
    if (!parsed) continue;
    for (const testResult of fileResult.tests) {
      if (testResult.status !== "FAILED") continue;
      testResult.steps.forEach((step, index) => {
        if (step.status === "FAILED") {
          const stepId = ids.step(parsed.uri, testResult.name, index);
          if (itemsById.has(stepId)) {
            failed.add(stepId);
          }
        }
      });
      const testId = ids.test(parsed.uri, testResult.name);
      if (itemsById.has(testId)) {
        failed.add(testId);
      }
    }
  }
  return failed;
}

function enqueueFileItems(
  parsed: ParsedFile,
  run: vscode.TestRun,
  itemsById: Map<string, vscode.TestItem>,
): void {
  const top = itemsById.get(ids.file(parsed.uri));
  if (!top) {
    return;
  }
  enqueueRecursive(top, run);
}

function enqueueRecursive(item: vscode.TestItem, run: vscode.TestRun): void {
  run.enqueued(item);
  item.children.forEach((child) => enqueueRecursive(child, run));
}

function collectAllTestItems(
  controller: vscode.TestController,
): Map<string, vscode.TestItem> {
  const map = new Map<string, vscode.TestItem>();
  const visit = (item: vscode.TestItem) => {
    map.set(item.id, item);
    item.children.forEach(visit);
  };
  controller.items.forEach(visit);
  return map;
}

function primaryWorkspaceFolder(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function markAllErrored(
  files: ParsedFile[],
  itemsById: Map<string, vscode.TestItem>,
  run: vscode.TestRun,
  message: string,
): void {
  for (const parsed of files) {
    const item = itemsById.get(ids.file(parsed.uri));
    if (item) {
      run.errored(item, new vscode.TestMessage(message));
    }
  }
}
