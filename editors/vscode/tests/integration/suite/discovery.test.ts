import * as assert from "assert";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

interface TarnExtensionApi {
  readonly testControllerId: string;
  readonly indexedFileCount: number;
  readonly commands: readonly string[];
}

async function waitUntil<T>(
  predicate: () => T | undefined | Promise<T | undefined>,
  timeoutMs = 10000,
  stepMs = 100,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await predicate();
    if (value !== undefined) {
      return value;
    }
    await new Promise((r) => setTimeout(r, stepMs));
  }
  throw new Error("waitUntil timed out");
}

describe("Tarn extension: discovery", () => {
  let api: TarnExtensionApi;

  before(async function () {
    this.timeout(60000);
    const ext = vscode.extensions.getExtension<TarnExtensionApi>(EXTENSION_ID);
    assert.ok(ext, `extension ${EXTENSION_ID} not found`);
    const exported = await ext!.activate();
    assert.ok(exported, "extension activated but did not return an API");
    api = exported;
  });

  it("activates and exposes its public API", () => {
    assert.strictEqual(api.testControllerId, "tarn");
    assert.ok(Array.isArray(api.commands));
    assert.ok(api.commands.includes("tarn.runAll"));
  });

  it("discovers the fixture .tarn.yaml file on startup", async function () {
    this.timeout(15000);
    await waitUntil(() => (api.indexedFileCount > 0 ? true : undefined));
    assert.ok(api.indexedFileCount >= 1, `expected >=1 indexed file, got ${api.indexedFileCount}`);
  });

  it("provides document symbols for a discovered file", async function () {
    this.timeout(15000);
    const uris = await vscode.workspace.findFiles("**/*.tarn.yaml");
    assert.ok(uris.length > 0, "fixture not found via findFiles");
    const doc = await vscode.workspace.openTextDocument(uris[0]);
    await vscode.window.showTextDocument(doc);
    const symbols = await waitUntil<vscode.DocumentSymbol[]>(
      async () => {
        const result = (await vscode.commands.executeCommand(
          "vscode.executeDocumentSymbolProvider",
          doc.uri,
        )) as vscode.DocumentSymbol[] | undefined;
        return result && result.length > 0 ? result : undefined;
      },
      10000,
      200,
    );
    assert.ok(symbols.length > 0, "no document symbols returned");
  });

  it("registers every Tarn command the API advertises", async () => {
    const allCommands = await vscode.commands.getCommands(true);
    for (const expected of api.commands) {
      assert.ok(allCommands.includes(expected), `missing command: ${expected}`);
    }
  });
});
