import * as assert from "assert";
import * as path from "path";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

interface TarnExtensionApiShape {
  readonly commands: readonly string[];
  readonly testing: {
    readonly showReportHtml: (html: string) => void;
    readonly sendReportMessage: (message: unknown) => Promise<boolean>;
  };
}

async function getApi(): Promise<TarnExtensionApiShape> {
  const ext = vscode.extensions.getExtension<TarnExtensionApiShape>(EXTENSION_ID);
  assert.ok(ext, `extension ${EXTENSION_ID} not found`);
  const api = await ext!.activate();
  assert.ok(api, "extension activated but returned no API");
  return api;
}

// Minimal Tarn-style HTML fixture — enough for the bridge walker to
// find one failing step in `tests/health.tarn.yaml`. The real Tarn
// HTML is much larger, but only the element structure and the DATA
// global matter for navigation.
const FIXTURE_HTML = `<!DOCTYPE html>
<html><head><title>Tarn report</title></head>
<body>
<script>
const DATA = {
  "files": [
    {
      "file": "tests/health.tarn.yaml",
      "name": "Fixture: health check",
      "status": "FAILED",
      "tests": [
        {
          "name": "service_is_up",
          "status": "FAILED",
          "steps": [
            { "name": "GET /status/200", "status": "FAILED", "duration_ms": 50 }
          ]
        }
      ]
    }
  ]
};
</script>
<div id="app">
  <div class="file-card" data-file="0">
    <div class="test-group" data-test="0-0">
      <div class="test-group-body">
        <div class="step">
          <span class="step-icon fail">x</span>
          <div class="step-info"><div class="step-name fail">GET /status/200</div></div>
        </div>
      </div>
    </div>
  </div>
</div>
</body></html>`;

describe("ReportWebview (tarn.openHtmlReport)", () => {
  let api: TarnExtensionApiShape;

  before(async function () {
    this.timeout(60000);
    api = await getApi();
  });

  it("registers the tarn.openHtmlReport command", async () => {
    const commands = await vscode.commands.getCommands(true);
    assert.ok(
      commands.includes("tarn.openHtmlReport"),
      "tarn.openHtmlReport should be registered",
    );
  });

  it("showReportHtml opens a webview panel without throwing", () => {
    api.testing.showReportHtml(FIXTURE_HTML);
    // vscode.window.createWebviewPanel does not expose a public
    // enumerable API, so the strongest assertion we can make here is
    // that the call succeeded. The panel-cleanup path runs on
    // extension deactivate.
    assert.ok(true);
  });

  it("jumpTo messages open the fixture file at the step range", async function () {
    this.timeout(10000);
    await vscode.commands.executeCommand("workbench.action.closeAllEditors");
    const resolved = await api.testing.sendReportMessage({
      type: "jumpTo",
      file: path.join("tests", "health.tarn.yaml"),
      test: "service_is_up",
      stepIndex: 0,
    });
    assert.strictEqual(resolved, true, "expected the jumpTo handler to resolve");
    const editor = vscode.window.activeTextEditor;
    assert.ok(editor, "expected an active editor after jumpTo");
    assert.ok(
      editor!.document.uri.fsPath.endsWith(path.join("tests", "health.tarn.yaml")),
      `unexpected active editor: ${editor!.document.uri.fsPath}`,
    );
    // health.tarn.yaml declares "name: GET /status/200" on line 9
    // (0-indexed 8) — the cursor should land on that line.
    assert.strictEqual(
      editor!.selection.active.line,
      8,
      `expected cursor on line 8, got ${editor!.selection.active.line}`,
    );
  });

  it("jumpTo messages with unknown files are ignored", async () => {
    const resolved = await api.testing.sendReportMessage({
      type: "jumpTo",
      file: "tests/does-not-exist.tarn.yaml",
      test: "x",
      stepIndex: 0,
    });
    assert.strictEqual(resolved, false);
  });

  it("messages with the wrong shape are ignored", async () => {
    assert.strictEqual(await api.testing.sendReportMessage(null), false);
    assert.strictEqual(
      await api.testing.sendReportMessage({ type: "other" }),
      false,
    );
    assert.strictEqual(
      await api.testing.sendReportMessage({ type: "jumpTo" }),
      false,
    );
  });
});
