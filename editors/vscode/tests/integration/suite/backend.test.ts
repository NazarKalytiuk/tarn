import * as assert from "assert";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

// Match the public extension API shape. We intentionally redeclare the
// types here so the integration suite stays a closed compilation unit
// (rootDir = tests/integration/).
interface NdjsonEventStepFinished {
  event: "step_finished";
  file: string;
  phase: "setup" | "test" | "teardown";
  test: string;
  step: string;
  step_index: number;
  status: "PASSED" | "FAILED";
  duration_ms: number;
}

interface NdjsonEventTestFinished {
  event: "test_finished";
  file: string;
  test: string;
  status: "PASSED" | "FAILED";
  duration_ms: number;
}

type NdjsonEvent =
  | { event: "file_started"; file: string; file_name: string }
  | NdjsonEventStepFinished
  | NdjsonEventTestFinished
  | { event: "file_finished"; file: string; status: "PASSED" | "FAILED" }
  | { event: "done"; summary: { status: "PASSED" | "FAILED" } };

interface RunOptions {
  files: string[];
  cwd: string;
  dryRun?: boolean;
  streamNdjson?: boolean;
  selectors?: string[];
  onEvent?: (event: NdjsonEvent) => void;
  token: vscode.CancellationToken;
}

interface RunOutcome {
  report: unknown;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  cancelled: boolean;
}

interface TarnBackendShape {
  run(options: RunOptions): Promise<RunOutcome>;
}

interface TarnExtensionApiShape {
  readonly commands: readonly string[];
  readonly testing: { readonly backend: TarnBackendShape };
}

function workspaceRoot(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    throw new Error("no workspace folder available in the test host");
  }
  return folder.uri.fsPath;
}

async function getApi(): Promise<TarnExtensionApiShape> {
  const ext = vscode.extensions.getExtension<TarnExtensionApiShape>(EXTENSION_ID);
  assert.ok(ext, `extension ${EXTENSION_ID} not found`);
  const api = await ext!.activate();
  assert.ok(api, "extension activated but returned no API");
  return api;
}

describe("Backend: --ndjson + --select", () => {
  const fixtureFile = "tests/dry.tarn.yaml";
  let api: TarnExtensionApiShape;

  before(async function () {
    this.timeout(60000);
    api = await getApi();
  });

  it("streams NDJSON events and emits done on a dry run", async function () {
    this.timeout(30000);
    const cwd = workspaceRoot();
    const events: NdjsonEvent[] = [];
    const cts = new vscode.CancellationTokenSource();
    try {
      const outcome = await api.testing.backend.run({
        files: [fixtureFile],
        cwd,
        dryRun: true,
        streamNdjson: true,
        token: cts.token,
        onEvent: (event) => events.push(event),
      });
      assert.strictEqual(outcome.cancelled, false);
      assert.ok(outcome.report, "expected a parsed final JSON report");
      assert.ok(events.length > 0, "expected at least one NDJSON event");

      const names = events.map((e) => e.event);
      assert.ok(names.includes("file_started"), `missing file_started: ${names.join(",")}`);
      assert.ok(names.includes("step_finished"), `missing step_finished: ${names.join(",")}`);
      assert.ok(names.includes("test_finished"), `missing test_finished: ${names.join(",")}`);
      assert.ok(names.includes("file_finished"), `missing file_finished: ${names.join(",")}`);
      assert.strictEqual(names[names.length - 1], "done", "done must be last");
    } finally {
      cts.dispose();
    }
  });

  it("honors --select to run only one test", async function () {
    this.timeout(30000);
    const cwd = workspaceRoot();
    const events: NdjsonEvent[] = [];
    const cts = new vscode.CancellationTokenSource();
    try {
      await api.testing.backend.run({
        files: [fixtureFile],
        cwd,
        dryRun: true,
        streamNdjson: true,
        selectors: [`${fixtureFile}::beta`],
        token: cts.token,
        onEvent: (event) => events.push(event),
      });

      const testNames = events
        .filter((e): e is NdjsonEventTestFinished => e.event === "test_finished")
        .map((e) => e.test);
      assert.deepStrictEqual(testNames, ["beta"], "expected only beta test to run");

      const stepNames = events
        .filter((e): e is NdjsonEventStepFinished => e.event === "step_finished")
        .filter((e) => e.phase === "test")
        .map((e) => `${e.test}/${e.step}`);
      assert.deepStrictEqual(
        stepNames,
        ["beta/only beta"],
        `expected only the beta step, got: ${stepNames.join(",")}`,
      );
    } finally {
      cts.dispose();
    }
  });

  it("honors --select to run only one step of a test", async function () {
    this.timeout(30000);
    const cwd = workspaceRoot();
    const events: NdjsonEvent[] = [];
    const cts = new vscode.CancellationTokenSource();
    try {
      await api.testing.backend.run({
        files: [fixtureFile],
        cwd,
        dryRun: true,
        streamNdjson: true,
        selectors: [`${fixtureFile}::alpha::1`],
        token: cts.token,
        onEvent: (event) => events.push(event),
      });

      const stepNames = events
        .filter((e): e is NdjsonEventStepFinished => e.event === "step_finished")
        .filter((e) => e.phase === "test")
        .map((e) => `${e.test}/${e.step}`);
      assert.deepStrictEqual(
        stepNames,
        ["alpha/second alpha"],
        `expected only alpha::1 step, got: ${stepNames.join(",")}`,
      );
    } finally {
      cts.dispose();
    }
  });
});
