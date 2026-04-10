import * as assert from "assert";
import * as path from "path";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

interface FixPlanEntry {
  category: string;
  testName: string;
  stepName: string;
  hint: string;
  location?: {
    uri: vscode.Uri;
    range: vscode.Range;
  };
}

interface FixPlanGroup {
  category: string;
  entries: FixPlanEntry[];
}

interface TarnExtensionApiShape {
  readonly commands: readonly string[];
  readonly testing: {
    readonly loadFixPlanFromReport: (report: unknown) => void;
    readonly fixPlanSnapshot: () => ReadonlyArray<FixPlanGroup>;
  };
}

async function getApi(): Promise<TarnExtensionApiShape> {
  const ext = vscode.extensions.getExtension<TarnExtensionApiShape>(EXTENSION_ID);
  assert.ok(ext, `extension ${EXTENSION_ID} not found`);
  const api = await ext!.activate();
  assert.ok(api, "extension activated but returned no API");
  return api;
}

/**
 * Synthetic report that references the existing `tests/health.tarn.yaml`
 * fixture. The test name and step name must match what the fixture
 * declares so the view can resolve the step's range through the
 * WorkspaceIndex. The request URL is never made — we never actually
 * run tarn, we just prime the view's data pipeline.
 */
function buildFailingReport(fixturePath: string): unknown {
  return {
    schema_version: 1,
    version: "1",
    timestamp: "2026-04-10T12:00:00Z",
    duration_ms: 120,
    files: [
      {
        file: fixturePath,
        name: "Fixture: health check",
        status: "FAILED",
        duration_ms: 120,
        summary: { total: 1, passed: 0, failed: 1 },
        setup: [],
        tests: [
          {
            name: "service_is_up",
            description: "Pings the public httpbin 200 endpoint",
            status: "FAILED",
            duration_ms: 120,
            steps: [
              {
                name: "GET /status/200",
                status: "FAILED",
                duration_ms: 120,
                failure_category: "assertion_failed",
                error_code: "assertion_mismatch",
                assertions: {
                  total: 1,
                  passed: 0,
                  failed: 1,
                  failures: [
                    {
                      assertion: "status",
                      passed: false,
                      expected: "200",
                      actual: "500",
                    },
                  ],
                },
                remediation_hints: [
                  "Inspect the response body to see why the server returned 500.",
                  "Confirm that the httpbin.org service is reachable from this network.",
                ],
              },
            ],
          },
        ],
        teardown: [],
      },
    ],
    summary: {
      files: 1,
      tests: 1,
      steps: { total: 1, passed: 0, failed: 1 },
      status: "FAILED",
    },
  };
}

describe("FixPlanView (tarn.fixPlan)", () => {
  let api: TarnExtensionApiShape;
  let fixturePath: string;

  before(async function () {
    this.timeout(60000);
    api = await getApi();
    // Use a path that the WorkspaceIndex resolves via fsPath suffix
    // match. The index parses files on activation from the launched
    // fixture workspace (editors/vscode/tests/integration/fixtures/workspace).
    fixturePath = path.join("tests", "health.tarn.yaml");
    api.testing.loadFixPlanFromReport(buildFailingReport(fixturePath));
  });

  it("registers the tarn.jumpToFailure command", async () => {
    const commands = await vscode.commands.getCommands(true);
    assert.ok(
      commands.includes("tarn.jumpToFailure"),
      "tarn.jumpToFailure should be registered",
    );
  });

  it("groups hints by failure category", () => {
    const snapshot = api.testing.fixPlanSnapshot();
    assert.strictEqual(snapshot.length, 1);
    assert.strictEqual(snapshot[0].category, "assertion_failed");
    assert.strictEqual(snapshot[0].entries.length, 2);
  });

  it("emits one entry per remediation hint with the right test/step names", () => {
    const snapshot = api.testing.fixPlanSnapshot();
    const entries = snapshot[0].entries;
    for (const entry of entries) {
      assert.strictEqual(entry.testName, "service_is_up");
      assert.strictEqual(entry.stepName, "GET /status/200");
    }
    const hints = entries.map((e) => e.hint);
    assert.ok(hints.some((h) => h.includes("500")));
    assert.ok(hints.some((h) => h.includes("httpbin.org")));
  });

  it("attaches a location pointing at the fixture file and step range", () => {
    const snapshot = api.testing.fixPlanSnapshot();
    const entry = snapshot[0].entries[0];
    assert.ok(entry.location, "expected location on the fix plan entry");
    assert.ok(
      entry.location!.uri.fsPath.endsWith(path.join("tests", "health.tarn.yaml")),
      `unexpected uri: ${entry.location!.uri.fsPath}`,
    );
    const range = entry.location!.range;
    // health.tarn.yaml declares "name: GET /status/200" on line 9
    // (0-indexed 8). The range should fall on that line so the jump
    // lands the cursor on the failing step.
    assert.strictEqual(
      range.start.line,
      8,
      `expected the step range to start on line 8, got ${range.start.line}`,
    );
  });

  it("reloading with a passing report empties the view", () => {
    api.testing.loadFixPlanFromReport({
      schema_version: 1,
      version: "1",
      duration_ms: 0,
      files: [],
      summary: {
        files: 0,
        tests: 0,
        steps: { total: 0, passed: 0, failed: 0 },
        status: "PASSED",
      },
    });
    assert.strictEqual(api.testing.fixPlanSnapshot().length, 0);
    // Re-prime so the snapshot is non-empty for any later assertions.
    api.testing.loadFixPlanFromReport(buildFailingReport(fixturePath));
    assert.strictEqual(api.testing.fixPlanSnapshot().length, 1);
  });

  it("tarn.jumpToFailure opens the file at the step's range", async function () {
    this.timeout(10000);
    const snapshot = api.testing.fixPlanSnapshot();
    const loc = snapshot[0].entries[0].location!;
    // Close any existing editors so the test observes a clean state.
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    await vscode.commands.executeCommand(
      "tarn.jumpToFailure",
      loc.uri.toString(),
      [
        loc.range.start.line,
        loc.range.start.character,
        loc.range.end.line,
        loc.range.end.character,
      ],
    );
    const editor = vscode.window.activeTextEditor;
    assert.ok(editor, "expected an active editor after tarn.jumpToFailure");
    assert.ok(
      editor!.document.uri.fsPath.endsWith(path.join("tests", "health.tarn.yaml")),
      `unexpected active editor: ${editor!.document.uri.fsPath}`,
    );
    assert.strictEqual(
      editor!.selection.active.line,
      loc.range.start.line,
      "cursor should land on the step's line",
    );
  });
});
