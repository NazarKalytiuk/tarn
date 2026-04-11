import * as assert from "assert";
import * as cp from "child_process";
import * as fs from "fs";
import * as net from "net";
import * as path from "path";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

// Redeclared minimal shapes — integration tests are a closed
// compilation unit (rootDir = tests/integration/) and cannot import
// from ../../src. These types cover only the fields the test reads or
// passes back through `buildFailureMessagesForStep`.

interface TarnLocation {
  file: string;
  line: number;
  column: number;
}

interface AssertionFailure {
  assertion: string;
  expected?: string;
  actual?: string;
  message?: string;
  diff?: string | null;
  location?: TarnLocation;
}

interface StepAssertions {
  total: number;
  passed: number;
  failed: number;
  details?: AssertionFailure[];
  failures?: AssertionFailure[];
}

interface ReportStep {
  name: string;
  status: "PASSED" | "FAILED";
  duration_ms: number;
  failure_category?: string;
  error_code?: string;
  location?: TarnLocation;
  assertions?: StepAssertions;
  request?: unknown;
  response?: unknown;
}

interface ReportTest {
  name: string;
  description?: string | null;
  status: "PASSED" | "FAILED";
  duration_ms: number;
  steps: ReportStep[];
}

interface ReportFile {
  file: string;
  name: string;
  status: "PASSED" | "FAILED";
  duration_ms: number;
  summary: { total: number; passed: number; failed: number };
  tests: ReportTest[];
}

interface Report {
  duration_ms: number;
  files: ReportFile[];
  summary: {
    files: number;
    tests: number;
    steps: { total: number; passed: number; failed: number };
    status: "PASSED" | "FAILED";
  };
}

interface RunOptions {
  files: string[];
  cwd: string;
  vars?: Record<string, string>;
  token: vscode.CancellationToken;
}

interface RunOutcome {
  report: Report | undefined;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  cancelled: boolean;
}

interface TarnBackendShape {
  run(options: RunOptions): Promise<RunOutcome>;
}

interface TarnExtensionApiShape {
  readonly testing: {
    readonly backend: TarnBackendShape;
    readonly buildFailureMessagesForStep: (
      step: ReportStep,
      fileUri: vscode.Uri,
      astFallback: vscode.Range | null,
    ) => vscode.TestMessage[];
  };
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

function allocateFreePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        server.close(() => reject(new Error("expected TCP address info")));
        return;
      }
      const port = address.port;
      server.close(() => resolve(port));
    });
  });
}

async function waitForServerReady(port: number, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastErr: unknown;
  while (Date.now() < deadline) {
    try {
      await new Promise<void>((resolve, reject) => {
        const socket = net.createConnection({ port, host: "127.0.0.1" });
        socket.once("connect", () => {
          socket.end();
          resolve();
        });
        socket.once("error", (err) => {
          socket.destroy();
          reject(err);
        });
      });
      return;
    } catch (err) {
      lastErr = err;
      await new Promise((r) => setTimeout(r, 50));
    }
  }
  throw new Error(
    `demo-server on port ${port} did not become ready within ${timeoutMs}ms (last error: ${String(lastErr)})`,
  );
}

function demoServerPath(): string {
  // Compiled layout: editors/vscode/tests/integration/out/suite/<file>.js.
  // Six ../ walks from out/suite back to the repository root.
  return path.resolve(__dirname, "../../../../../../target/debug/demo-server");
}

const FIXTURE_RELATIVE = "tests/location-drift.tarn.yaml";
// Deterministic layout, verified by running `tarn run` directly:
//   - line 10 (1-based) is the step's `- name: GET /health` key
//   - line 15 (1-based) is the `status: 404` assertion operator
// Tarn T55 reports 1-based line/column; VS Code uses 0-based, so the
// expected 0-based values are line 9 for the step anchor and line 14
// for the assertion anchor.
const FIXTURE_CONTENTS = `version: "1"
name: "Location drift fixture (NAZ-281)"
description: |
  Deterministic single failing step. Tarn T55 reports a location for
  the step and for the failing assertion.

tests:
  always_fails:
    steps:
      - name: GET /health
        request:
          method: GET
          url: "{{ env.base_url }}/health"
        assert:
          status: 404
`;
const EXPECTED_STEP_LINE_0BASED = 9; // "- name: GET /health" — Tarn reports line 10 (1-based).
const EXPECTED_STEP_COLUMN_0BASED = 8; // Tarn reports column 9 (1-based).
const EXPECTED_ASSERTION_LINE_0BASED = 14; // "status: 404" — Tarn reports line 15 (1-based).
const EXPECTED_ASSERTION_COLUMN_0BASED = 10; // Tarn reports column 11 (1-based).

describe("ResultMapper: consume Tarn-reported location metadata (NAZ-281)", () => {
  let api: TarnExtensionApiShape;
  let serverProcess: cp.ChildProcess | undefined;
  let port: number;
  let fixtureAbsolute: string;

  before(async function () {
    this.timeout(60000);
    api = await getApi();

    const binary = demoServerPath();
    if (!fs.existsSync(binary)) {
      throw new Error(
        `demo-server binary not found at ${binary}. Run \`cargo build -p demo-server\` from the repo root first.`,
      );
    }

    port = await allocateFreePort();
    serverProcess = cp.spawn(binary, [], {
      env: { ...process.env, PORT: String(port) },
      stdio: ["ignore", "pipe", "pipe"],
    });
    serverProcess.on("error", (err) => {
      // eslint-disable-next-line no-console
      console.error(`[resultMapperLocation] demo-server spawn error:`, err);
    });

    await waitForServerReady(port, 10000);

    fixtureAbsolute = path.join(workspaceRoot(), FIXTURE_RELATIVE);
    fs.mkdirSync(path.dirname(fixtureAbsolute), { recursive: true });
    fs.writeFileSync(fixtureAbsolute, FIXTURE_CONTENTS, "utf8");
  });

  after(async function () {
    this.timeout(10000);
    try {
      if (fs.existsSync(fixtureAbsolute)) {
        fs.unlinkSync(fixtureAbsolute);
      }
    } catch {
      /* ignore */
    }
    if (serverProcess && !serverProcess.killed) {
      serverProcess.kill("SIGTERM");
      await new Promise<void>((resolve) => {
        const timer = setTimeout(() => {
          serverProcess?.kill("SIGKILL");
          resolve();
        }, 3000);
        serverProcess?.once("exit", () => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
  });

  async function runFailingFixture(): Promise<RunOutcome> {
    const cts = new vscode.CancellationTokenSource();
    try {
      return await api.testing.backend.run({
        files: [FIXTURE_RELATIVE],
        cwd: workspaceRoot(),
        vars: { base_url: `http://127.0.0.1:${port}` },
        token: cts.token,
      });
    } finally {
      cts.dispose();
    }
  }

  it("Tarn reports location metadata on the failing step and assertion", async function () {
    this.timeout(30000);
    const outcome = await runFailingFixture();
    assert.ok(
      outcome.report,
      `expected a JSON report; exit=${outcome.exitCode}, stderr=${outcome.stderr.slice(0, 400)}`,
    );
    const step = outcome.report!.files[0]?.tests[0]?.steps[0];
    assert.ok(step, "expected one failing step in the report");
    assert.strictEqual(step!.status, "FAILED");

    // This is the whole point of NAZ-260: the JSON report carries
    // `location` on both the step and every assertion failure that
    // maps back to a YAML operator key.
    assert.ok(
      step!.location,
      "expected step.location to be populated by Tarn T55",
    );
    assert.strictEqual(step!.location!.line, 10);
    assert.strictEqual(step!.location!.column, 9);
    assert.ok(
      step!.location!.file.endsWith(FIXTURE_RELATIVE),
      `step.location.file should end with ${FIXTURE_RELATIVE}, got ${step!.location!.file}`,
    );

    const failure = step!.assertions?.failures?.[0];
    assert.ok(failure, "expected at least one assertion failure");
    assert.ok(
      failure!.location,
      "expected assertion failure to carry its own location",
    );
    assert.strictEqual(failure!.location!.line, 15);
    assert.strictEqual(failure!.location!.column, 11);
  });

  it("buildFailureMessagesForStep anchors on the JSON location, not the AST range", async function () {
    this.timeout(30000);
    const outcome = await runFailingFixture();
    assert.ok(outcome.report);
    const step = outcome.report!.files[0].tests[0].steps[0];
    assert.strictEqual(step.status, "FAILED");

    // Deliberately pass a wrong AST range. If the mapper fell back to
    // the AST even when the JSON has a location, the TestMessage would
    // land on line 999. After NAZ-281, the JSON location wins — so the
    // message must land on EXPECTED_ASSERTION_LINE_0BASED, the assertion
    // operator key.
    const wrongAstRange = new vscode.Range(
      new vscode.Position(999, 0),
      new vscode.Position(999, 10),
    );
    const fixtureUri = vscode.Uri.file(fixtureAbsolute);
    const messages = api.testing.buildFailureMessagesForStep(
      step,
      fixtureUri,
      wrongAstRange,
    );

    assert.strictEqual(
      messages.length,
      1,
      "expected exactly one failure message for one assertion failure",
    );
    const message = messages[0];
    assert.ok(message.location, "TestMessage should carry a location");
    assert.strictEqual(
      message.location!.range.start.line,
      EXPECTED_ASSERTION_LINE_0BASED,
      `expected the squiggle on line ${EXPECTED_ASSERTION_LINE_0BASED} (the status assertion), got ${message.location!.range.start.line}. The mapper must prefer JSON location over the AST range.`,
    );
    assert.strictEqual(
      message.location!.range.start.character,
      EXPECTED_ASSERTION_COLUMN_0BASED,
      `expected column ${EXPECTED_ASSERTION_COLUMN_0BASED} (= 11 - 1, Tarn is 1-based)`,
    );
    // Same URI identity as the fixture so VS Code renders the
    // diagnostic in the correct editor tab.
    assert.strictEqual(
      message.location!.uri.fsPath,
      fixtureUri.fsPath,
      "TestMessage location should target the fixture file",
    );
  });

  it("survives AST drift: a mid-run edit does not move the diagnostic off the assertion node", async function () {
    this.timeout(30000);
    const outcome = await runFailingFixture();
    assert.ok(outcome.report);
    const step = outcome.report!.files[0].tests[0].steps[0];

    // Simulate the drift scenario from the acceptance criteria: the
    // user edits the file (insert two blank lines at the top) AFTER
    // Tarn has already captured the run-time line numbers. A naive
    // AST-only mapper would now place the squiggle two lines below
    // the real assertion. With JSON location, the diagnostic stays
    // glued to the original assertion node.
    const driftedContents = "\n\n" + FIXTURE_CONTENTS;
    fs.writeFileSync(fixtureAbsolute, driftedContents, "utf8");

    // Reconstruct the AST range the discovery pipeline would have
    // built from the post-edit file. The step's `name:` key moved
    // from line 10 (1-based) to line 12 (1-based), i.e. 0-based 11.
    // If the mapper fell back to this range instead of using the JSON
    // location, the test would see line 11 — not 14.
    const driftedAstRange = new vscode.Range(
      new vscode.Position(11, 8),
      new vscode.Position(11, 23),
    );

    const messages = api.testing.buildFailureMessagesForStep(
      step,
      vscode.Uri.file(fixtureAbsolute),
      driftedAstRange,
    );
    assert.strictEqual(messages.length, 1);

    // The JSON-reported assertion location is still line 14 (1-based)
    // = line 13 (0-based). It reflects the pre-edit YAML Tarn
    // actually executed.
    assert.strictEqual(
      messages[0].location!.range.start.line,
      EXPECTED_ASSERTION_LINE_0BASED,
      "after a mid-run edit, the diagnostic must stay anchored to the JSON-reported line",
    );

    // Restore the fixture so later tests in this describe block still
    // run against the original layout, and so the afterAll cleanup
    // deletes the right file.
    fs.writeFileSync(fixtureAbsolute, FIXTURE_CONTENTS, "utf8");
  });

  it("falls back to the AST range when step.location is absent (older Tarn)", async function () {
    this.timeout(30000);
    // Synthesize an older-Tarn step by copying a real step and
    // stripping every `location` field. The mapper must still produce
    // a message anchored via the AST fallback, proving the fallback
    // branch is reachable and not an orphan.
    const outcome = await runFailingFixture();
    assert.ok(outcome.report);
    const real = outcome.report!.files[0].tests[0].steps[0];
    const legacy: ReportStep = JSON.parse(JSON.stringify(real)) as ReportStep;
    delete legacy.location;
    if (legacy.assertions?.failures) {
      for (const f of legacy.assertions.failures) {
        delete f.location;
      }
    }
    if (legacy.assertions?.details) {
      for (const d of legacy.assertions.details) {
        delete d.location;
      }
    }

    const astRange = new vscode.Range(
      new vscode.Position(EXPECTED_STEP_LINE_0BASED, 8),
      new vscode.Position(EXPECTED_STEP_LINE_0BASED, 23),
    );
    const messages = api.testing.buildFailureMessagesForStep(
      legacy,
      vscode.Uri.file(fixtureAbsolute),
      astRange,
    );
    assert.strictEqual(messages.length, 1);
    assert.ok(messages[0].location);
    assert.strictEqual(
      messages[0].location!.range.start.line,
      EXPECTED_STEP_LINE_0BASED,
      "with no JSON location, the AST fallback must kick in",
    );
  });
});
