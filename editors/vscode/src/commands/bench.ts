import * as path from "path";
import * as vscode from "vscode";
import type { TarnBackend } from "../backend/TarnBackend";
import type { WorkspaceIndex, ParsedFile } from "../workspace/WorkspaceIndex";
import type { BenchRunnerPanel } from "../views/BenchRunnerPanel";
import type { TarnTestController } from "../testing/TestController";
import { readConfig } from "../config";
import { getOutputChannel } from "../outputChannel";

const SETTINGS_KEY_PREFIX = "tarn.benchSettings";

interface BenchSettings {
  stepIndex: number;
  testName?: string;
  requests: number;
  concurrency: number;
  rampUp?: string;
}

export interface BenchCommandDeps {
  backend: TarnBackend;
  index: WorkspaceIndex;
  panel: BenchRunnerPanel;
  tarnController: TarnTestController;
  workspaceState: vscode.Memento;
}

export function registerBenchCommand(deps: BenchCommandDeps): vscode.Disposable {
  return vscode.commands.registerCommand("tarn.benchStep", async () => {
    await runBenchWizard(deps);
  });
}

async function runBenchWizard(deps: BenchCommandDeps): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    vscode.window.showInformationMessage(
      "Tarn: open a .tarn.yaml file first to benchmark one of its steps.",
    );
    return;
  }
  const parsed = deps.index.get(editor.document.uri);
  if (!parsed) {
    vscode.window.showInformationMessage(
      "Tarn: the current file is not a discovered Tarn test file.",
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
  const settingsKey = buildSettingsKey(relFile);
  const lastSettings =
    deps.workspaceState.get<BenchSettings>(settingsKey) ?? defaultSettings();

  const stepPick = await pickStep(parsed, lastSettings);
  if (!stepPick) return;

  const requests = await promptPositiveInt(
    "Total number of requests (N)",
    lastSettings.requests,
  );
  if (requests === undefined) return;

  const concurrency = await promptPositiveInt(
    "Concurrency (number of workers)",
    lastSettings.concurrency,
  );
  if (concurrency === undefined) return;

  const rampUp = await promptRampUp(lastSettings.rampUp);
  if (rampUp === undefined) return; // user cancelled

  const newSettings: BenchSettings = {
    stepIndex: stepPick.stepIndex,
    testName: stepPick.testName,
    requests,
    concurrency,
    rampUp: rampUp.length > 0 ? rampUp : undefined,
  };
  await deps.workspaceState.update(settingsKey, newSettings);

  const config = readConfig();
  const activeEnv = deps.tarnController.state.activeEnvironment;
  const cts = new vscode.CancellationTokenSource();
  const out = getOutputChannel();
  out.appendLine(
    `[tarn] bench ${relFile} step=${stepPick.stepIndex} (${stepPick.label}) n=${requests} c=${concurrency}${
      newSettings.rampUp ? ` ramp-up=${newSettings.rampUp}` : ""
    }`,
  );

  const outcome = await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: `Tarn: benchmarking ${stepPick.label}…`,
      cancellable: true,
    },
    async (_progress, token) => {
      token.onCancellationRequested(() => cts.cancel());
      return deps.backend.runBench({
        file: relFile,
        cwd: folder.uri.fsPath,
        stepIndex: stepPick.stepIndex,
        requests,
        concurrency,
        rampUp: newSettings.rampUp,
        environment: activeEnv ?? config.defaultEnvironment,
        token: cts.token,
      });
    },
  );
  cts.dispose();

  if (!outcome.result) {
    const detail =
      outcome.stderr.trim() ||
      outcome.stdout.trim() ||
      `tarn bench exited with code ${outcome.exitCode}`;
    vscode.window.showErrorMessage(`Tarn bench failed: ${truncate(detail, 200)}`);
    if (outcome.stderr) out.appendLine(outcome.stderr);
    return;
  }

  deps.panel.show({
    result: outcome.result,
    file: relFile,
    testName: stepPick.testName,
  });
}

interface StepPick {
  stepIndex: number;
  testName: string | undefined;
  label: string;
}

async function pickStep(
  parsed: ParsedFile,
  last: BenchSettings,
): Promise<StepPick | undefined> {
  type Item = vscode.QuickPickItem & { step: StepPick };
  const items: Item[] = [];
  for (const setup of parsed.ranges.setup) {
    items.push({
      label: `setup / ${setup.name}`,
      description: `step ${setup.index}`,
      step: { stepIndex: setup.index, testName: undefined, label: `setup/${setup.name}` },
    });
  }
  for (const test of parsed.ranges.tests) {
    for (const step of test.steps) {
      items.push({
        label: `${test.name} / ${step.name}`,
        description: `step ${step.index}`,
        step: {
          stepIndex: step.index,
          testName: test.name,
          label: `${test.name}/${step.name}`,
        },
      });
    }
  }
  for (const teardown of parsed.ranges.teardown) {
    items.push({
      label: `teardown / ${teardown.name}`,
      description: `step ${teardown.index}`,
      step: {
        stepIndex: teardown.index,
        testName: undefined,
        label: `teardown/${teardown.name}`,
      },
    });
  }
  if (items.length === 0) {
    vscode.window.showInformationMessage(
      "Tarn: the current file has no steps to benchmark.",
    );
    return undefined;
  }
  // Hoist the previously selected step to the top so repeat runs
  // are one quick-pick away.
  const sorted = [...items].sort((a, b) => {
    const aMatch =
      a.step.stepIndex === last.stepIndex && a.step.testName === last.testName;
    const bMatch =
      b.step.stepIndex === last.stepIndex && b.step.testName === last.testName;
    if (aMatch === bMatch) return 0;
    return aMatch ? -1 : 1;
  });
  const picked = await vscode.window.showQuickPick(sorted, {
    placeHolder: "Select step to benchmark",
    matchOnDescription: true,
  });
  return picked?.step;
}

async function promptPositiveInt(
  prompt: string,
  defaultValue: number,
): Promise<number | undefined> {
  const raw = await vscode.window.showInputBox({
    prompt,
    value: String(defaultValue),
    validateInput: (input) => {
      const n = Number(input);
      if (!Number.isFinite(n) || !Number.isInteger(n) || n <= 0) {
        return "Enter a positive integer";
      }
      return undefined;
    },
  });
  if (raw === undefined) return undefined;
  return Number(raw);
}

async function promptRampUp(defaultValue: string | undefined): Promise<string | undefined> {
  return vscode.window.showInputBox({
    prompt: 'Ramp-up duration (e.g. "5s", "500ms") — leave empty to skip',
    value: defaultValue ?? "",
    validateInput: (input) => {
      if (input.trim().length === 0) return undefined;
      if (/^\d+(ms|s|m)?$/.test(input.trim())) return undefined;
      return "Use a format like 5s, 500ms, or 2m";
    },
  });
}

export function buildSettingsKey(relativeFile: string): string {
  return `${SETTINGS_KEY_PREFIX}:${relativeFile}`;
}

function defaultSettings(): BenchSettings {
  return { stepIndex: 0, requests: 100, concurrency: 10 };
}

function truncate(s: string, n: number): string {
  if (s.length <= n) return s;
  return `${s.slice(0, n - 1)}…`;
}
