import * as assert from "assert";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

interface TarnExtensionApiShape {
  readonly commands: readonly string[];
  readonly testing: {
    readonly importHurl: (
      source: string,
      dest: string,
      cwd: string,
    ) => Promise<{ success: boolean; exitCode: number | null; stderr: string }>;
  };
}

async function getApi(): Promise<TarnExtensionApiShape> {
  const ext = vscode.extensions.getExtension<TarnExtensionApiShape>(EXTENSION_ID);
  assert.ok(ext, `extension ${EXTENSION_ID} not found`);
  const api = await ext!.activate();
  assert.ok(api, "extension activated but returned no API");
  return api;
}

const HURL_FIXTURE = `GET https://example.com/api/users
HTTP 200
`;

describe("Import Hurl wizard (tarn.importHurl)", () => {
  let api: TarnExtensionApiShape;
  let tmpDir: string;

  before(async function () {
    this.timeout(60000);
    api = await getApi();
    tmpDir = await fs.promises.mkdtemp(path.join(os.tmpdir(), "tarn-vscode-hurl-"));
  });

  after(async () => {
    if (tmpDir) {
      await fs.promises.rm(tmpDir, { recursive: true, force: true }).catch(() => {});
    }
  });

  it("registers the tarn.importHurl command", async () => {
    const commands = await vscode.commands.getCommands(true);
    assert.ok(
      commands.includes("tarn.importHurl"),
      "tarn.importHurl should be registered",
    );
  });

  it("converts a .hurl file to a .tarn.yaml via the backend", async function () {
    this.timeout(15000);
    const source = path.join(tmpDir, "sample.hurl");
    const dest = path.join(tmpDir, "sample.tarn.yaml");
    await fs.promises.writeFile(source, HURL_FIXTURE, "utf8");

    const outcome = await api.testing.importHurl(source, dest, tmpDir);
    assert.strictEqual(outcome.success, true, `import failed: ${outcome.stderr}`);
    assert.strictEqual(outcome.exitCode, 0);

    const written = await fs.promises.readFile(dest, "utf8");
    assert.ok(written.includes("https://example.com/api/users"), written);
    assert.ok(written.includes("method: GET"), written);
    assert.ok(written.includes("status: 200"), written);
  });

  it("returns success=false when the source file does not exist", async function () {
    this.timeout(15000);
    const source = path.join(tmpDir, "does-not-exist.hurl");
    const dest = path.join(tmpDir, "does-not-exist.tarn.yaml");
    const outcome = await api.testing.importHurl(source, dest, tmpDir);
    assert.strictEqual(outcome.success, false);
  });
});
