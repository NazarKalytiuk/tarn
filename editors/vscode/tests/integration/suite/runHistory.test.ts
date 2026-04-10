import * as assert from "assert";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

interface RunHistoryEntryShape {
  id: string;
  timestamp: number;
  label: string;
  environment: string | null;
  tags: string[];
  status: "PASSED" | "FAILED" | "CANCELLED" | "ERRORED";
  passed: number;
  failed: number;
  total: number;
  durationMs: number;
  files: string[];
  selectors: string[];
  dryRun: boolean;
  pinned: boolean;
}

interface RunHistoryFilterShape {
  kind: "all" | "passed" | "failed" | "env" | "tag";
  value?: string;
}

interface TarnExtensionApiShape {
  readonly commands: readonly string[];
  readonly testing: {
    readonly history: {
      readonly add: (entry: RunHistoryEntryShape) => Promise<void>;
      readonly all: () => ReadonlyArray<RunHistoryEntryShape>;
      readonly clear: () => Promise<void>;
      readonly setFilter: (filter: RunHistoryFilterShape) => void;
      readonly getFilter: () => RunHistoryFilterShape;
    };
  };
}

async function getApi(): Promise<TarnExtensionApiShape> {
  const ext = vscode.extensions.getExtension<TarnExtensionApiShape>(EXTENSION_ID);
  assert.ok(ext, `extension ${EXTENSION_ID} not found`);
  const api = await ext!.activate();
  assert.ok(api, "extension activated but returned no API");
  return api;
}

function makeEntry(overrides: Partial<RunHistoryEntryShape> = {}): RunHistoryEntryShape {
  return {
    id: overrides.id ?? `e-${Math.random().toString(36).slice(2)}`,
    timestamp: Date.now(),
    label: "1/1 steps",
    environment: null,
    tags: [],
    status: "PASSED",
    passed: 1,
    failed: 0,
    total: 1,
    durationMs: 100,
    files: [],
    selectors: [],
    dryRun: false,
    pinned: false,
    ...overrides,
  };
}

describe("RunHistory pin / filter / rerun wiring", () => {
  let api: TarnExtensionApiShape;

  before(async function () {
    this.timeout(60000);
    api = await getApi();
    await api.testing.history.clear();
  });

  after(async () => {
    await api.testing.history.clear();
  });

  it("registers the new history commands", async () => {
    const commands = await vscode.commands.getCommands(true);
    for (const cmd of [
      "tarn.pinHistoryEntry",
      "tarn.unpinHistoryEntry",
      "tarn.filterHistory",
      "tarn.rerunFromHistory",
    ]) {
      assert.ok(commands.includes(cmd), `missing command: ${cmd}`);
    }
  });

  it("tarn.pinHistoryEntry and tarn.unpinHistoryEntry toggle the pinned flag", async () => {
    await api.testing.history.clear();
    const entry = makeEntry({ id: "pinned-target" });
    await api.testing.history.add(entry);
    await vscode.commands.executeCommand("tarn.pinHistoryEntry", entry);
    const afterPin = api.testing.history.all();
    const pinnedEntry = afterPin.find((e) => e.id === "pinned-target");
    assert.ok(pinnedEntry, "expected the pinned entry to still exist");
    assert.strictEqual(pinnedEntry!.pinned, true);

    await vscode.commands.executeCommand("tarn.unpinHistoryEntry", entry);
    const afterUnpin = api.testing.history.all();
    const unpinnedEntry = afterUnpin.find((e) => e.id === "pinned-target");
    assert.strictEqual(unpinnedEntry!.pinned, false);
  });

  it("clear() keeps pinned entries in place", async () => {
    await api.testing.history.clear();
    await api.testing.history.add(makeEntry({ id: "keep", pinned: true }));
    await api.testing.history.add(makeEntry({ id: "drop" }));
    await api.testing.history.clear();
    const remaining = api.testing.history.all();
    assert.deepStrictEqual(
      remaining.map((e) => e.id),
      ["keep"],
    );
  });

  it("setFilter / getFilter round-trip through the tree provider", () => {
    api.testing.history.setFilter({ kind: "failed" });
    assert.deepStrictEqual(api.testing.history.getFilter(), { kind: "failed" });
    api.testing.history.setFilter({ kind: "env", value: "staging" });
    assert.deepStrictEqual(api.testing.history.getFilter(), {
      kind: "env",
      value: "staging",
    });
    api.testing.history.setFilter({ kind: "all" });
    assert.deepStrictEqual(api.testing.history.getFilter(), { kind: "all" });
  });

  it("tarn.rerunFromHistory ignores missing entries gracefully", async function () {
    this.timeout(10000);
    // Passing an unknown id must not throw. VS Code swallows
    // info messages in tests, but we can assert that the command
    // returns without error.
    await vscode.commands.executeCommand("tarn.rerunFromHistory", {
      id: "does-not-exist",
    });
  });
});
