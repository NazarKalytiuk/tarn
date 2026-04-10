import * as assert from "assert";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

interface EnvEntryShape {
  readonly name: string;
  readonly source_file: string;
  readonly vars: Readonly<Record<string, string>>;
}

interface TarnExtensionApiShape {
  readonly commands: readonly string[];
  readonly testing: {
    readonly reloadEnvironments: () => Promise<void>;
    readonly listEnvironments: () => Promise<ReadonlyArray<EnvEntryShape>>;
    readonly getActiveEnvironment: () => string | null;
  };
}

async function getApi(): Promise<TarnExtensionApiShape> {
  const ext = vscode.extensions.getExtension<TarnExtensionApiShape>(EXTENSION_ID);
  assert.ok(ext, `extension ${EXTENSION_ID} not found`);
  const api = await ext!.activate();
  assert.ok(api, "extension activated but returned no API");
  return api;
}

describe("EnvironmentsView backed by tarn env --json", () => {
  let api: TarnExtensionApiShape;

  before(async function () {
    this.timeout(60000);
    api = await getApi();
    await api.testing.reloadEnvironments();
  });

  it("loads the environments declared in tarn.config.yaml", async function () {
    this.timeout(15000);
    const entries = await api.testing.listEnvironments();
    const names = entries.map((e) => e.name);
    assert.deepStrictEqual(
      names,
      ["production", "staging"],
      `expected alphabetical order, got ${JSON.stringify(names)}`,
    );
    for (const entry of entries) {
      assert.ok(entry.source_file.length > 0, `missing source_file on ${entry.name}`);
      assert.ok(
        typeof entry.vars === "object" && entry.vars !== null,
        `vars should be an object on ${entry.name}`,
      );
    }
  });

  it("redacts configured secret vars", async function () {
    this.timeout(15000);
    const entries = await api.testing.listEnvironments();
    const staging = entries.find((e) => e.name === "staging");
    assert.ok(staging, "staging environment not found");
    assert.strictEqual(
      staging!.vars.api_token,
      "***",
      `expected api_token to be redacted, got: ${staging!.vars.api_token}`,
    );
    assert.strictEqual(staging!.vars.base_url, "https://staging.example.com");
  });

  it("registers the Tarn: Reload Environments command", async function () {
    const allCommands = await vscode.commands.getCommands(true);
    assert.ok(
      allCommands.includes("tarn.reloadEnvironments"),
      "tarn.reloadEnvironments command should be registered",
    );
    assert.ok(
      allCommands.includes("tarn.setEnvironmentFromTree"),
      "tarn.setEnvironmentFromTree command should be registered",
    );
    assert.ok(
      allCommands.includes("tarn.openEnvironmentSource"),
      "tarn.openEnvironmentSource command should be registered",
    );
    assert.ok(
      allCommands.includes("tarn.copyEnvironmentAsFlag"),
      "tarn.copyEnvironmentAsFlag command should be registered",
    );
  });

  it("tarn.setEnvironmentFromTree toggles the active environment", async function () {
    this.timeout(15000);
    assert.strictEqual(
      api.testing.getActiveEnvironment(),
      null,
      "active env should start as null",
    );

    await vscode.commands.executeCommand("tarn.setEnvironmentFromTree", "staging");
    assert.strictEqual(api.testing.getActiveEnvironment(), "staging");

    // Running the same command again toggles off.
    await vscode.commands.executeCommand("tarn.setEnvironmentFromTree", "staging");
    assert.strictEqual(
      api.testing.getActiveEnvironment(),
      null,
      "re-invoking should clear the active env",
    );
  });

  it("tarn.copyEnvironmentAsFlag writes to the clipboard", async function () {
    this.timeout(15000);
    await vscode.commands.executeCommand("tarn.copyEnvironmentAsFlag", "production");
    const clipboard = await vscode.env.clipboard.readText();
    assert.strictEqual(clipboard, "--env production");
  });
});
