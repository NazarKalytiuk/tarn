import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

async function ensureActivated(): Promise<void> {
  const ext = vscode.extensions.getExtension(EXTENSION_ID);
  assert.ok(ext, `extension ${EXTENSION_ID} not found`);
  await ext!.activate();
}

function workspaceRoot(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    throw new Error("no workspace folder available");
  }
  return folder.uri.fsPath;
}

function writeFixture(relativePath: string, content: string): vscode.Uri {
  const absolute = path.join(workspaceRoot(), relativePath);
  fs.mkdirSync(path.dirname(absolute), { recursive: true });
  fs.writeFileSync(absolute, content, "utf8");
  return vscode.Uri.file(absolute);
}

async function definitionsAt(
  uri: vscode.Uri,
  line: number,
  character: number,
): Promise<vscode.Location[]> {
  const position = new vscode.Position(line, character);
  const result = (await vscode.commands.executeCommand(
    "vscode.executeDefinitionProvider",
    uri,
    position,
  )) as vscode.Location[] | vscode.LocationLink[] | undefined;
  if (!result) return [];
  const locations: vscode.Location[] = [];
  for (const entry of result) {
    if ("targetUri" in entry) {
      locations.push(new vscode.Location(entry.targetUri, entry.targetRange));
    } else {
      locations.push(entry);
    }
  }
  return locations;
}

async function referencesAt(
  uri: vscode.Uri,
  line: number,
  character: number,
): Promise<vscode.Location[]> {
  const position = new vscode.Position(line, character);
  const result = (await vscode.commands.executeCommand(
    "vscode.executeReferenceProvider",
    uri,
    position,
  )) as vscode.Location[] | undefined;
  return result ?? [];
}

async function prepareRenameAt(
  uri: vscode.Uri,
  line: number,
  character: number,
): Promise<{ placeholder: string; range: vscode.Range }> {
  const position = new vscode.Position(line, character);
  const result = (await vscode.commands.executeCommand(
    "vscode.prepareRename",
    uri,
    position,
  )) as { placeholder: string; range: vscode.Range } | vscode.Range;
  if ("placeholder" in result) {
    return result;
  }
  return { placeholder: "", range: result };
}

async function renameAt(
  uri: vscode.Uri,
  line: number,
  character: number,
  newName: string,
): Promise<vscode.WorkspaceEdit> {
  const position = new vscode.Position(line, character);
  return (await vscode.commands.executeCommand(
    "vscode.executeDocumentRenameProvider",
    uri,
    position,
    newName,
  )) as vscode.WorkspaceEdit;
}

describe("Tarn symbol providers: definition, references, rename", () => {
  const createdFiles: vscode.Uri[] = [];

  before(async function () {
    this.timeout(60000);
    await ensureActivated();
  });

  afterEach(() => {
    for (const uri of createdFiles) {
      try {
        fs.unlinkSync(uri.fsPath);
      } catch {
        /* ignore */
      }
    }
    createdFiles.length = 0;
  });

  it("go-to-definition on {{ capture.x }} jumps to the capture declaration", async function () {
    this.timeout(15000);
    const uri = writeFixture(
      "symbols-capture-def.tarn.yaml",
      `name: Symbols capture def
tests:
  crud:
    steps:
      - name: login
        request:
          method: POST
          url: "http://localhost/auth"
        capture:
          auth_token: "$.token"
      - name: fetch
        request:
          method: GET
          url: "http://localhost/users/{{ capture.auth_token }}"
`,
    );
    createdFiles.push(uri);
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc);

    const lines = doc.getText().split("\n");
    const refLine = lines.findIndex((l) => l.includes("{{ capture.auth_token }}"));
    const refCol = lines[refLine].indexOf("auth_token") + 3;
    const locations = await definitionsAt(uri, refLine, refCol);

    assert.strictEqual(locations.length, 1, `expected 1 location, got ${locations.length}`);
    const decl = locations[0];
    const declLineText = lines[decl.range.start.line];
    assert.ok(
      declLineText.includes("auth_token: "),
      `definition landed on wrong line: ${declLineText}`,
    );
  });

  it("go-to-definition on {{ env.base_url }} lists the source file locations", async function () {
    this.timeout(15000);
    const uri = writeFixture(
      "symbols-env-def.tarn.yaml",
      `name: Symbols env def
tests:
  t:
    steps:
      - name: ping
        request:
          method: GET
          url: "{{ env.base_url }}/health"
`,
    );
    createdFiles.push(uri);
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc);

    const lines = doc.getText().split("\n");
    const refLine = lines.findIndex((l) => l.includes("{{ env.base_url }}"));
    const refCol = lines[refLine].indexOf("base_url") + 3;
    const locations = await definitionsAt(uri, refLine, refCol);

    assert.ok(
      locations.length >= 1,
      `expected at least one env source location, got ${locations.length}`,
    );
    // Fixture declares base_url in both staging and production
    // env files in tarn.config.yaml.
    const sourceFiles = locations.map((l) =>
      path.basename(l.uri.fsPath),
    );
    assert.ok(
      sourceFiles.some((n) => n.includes("env")) ||
        sourceFiles.includes("tarn.config.yaml"),
      `expected env-related source files, got: ${sourceFiles.join(", ")}`,
    );
  });

  it("find-all-references on a capture returns every in-file usage plus the declaration", async function () {
    this.timeout(15000);
    const uri = writeFixture(
      "symbols-refs.tarn.yaml",
      `name: Symbols refs
tests:
  crud:
    steps:
      - name: login
        request:
          method: POST
          url: "http://localhost/auth"
        capture:
          auth_token: "$.token"
      - name: fetch
        request:
          method: GET
          url: "http://localhost/users/{{ capture.auth_token }}"
      - name: delete
        request:
          method: DELETE
          url: "http://localhost/users/{{ capture.auth_token }}"
`,
    );
    createdFiles.push(uri);
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc);

    const lines = doc.getText().split("\n");
    const declLine = lines.findIndex((l) => l.includes("auth_token: "));
    const declCol = lines[declLine].indexOf("auth_token") + 3;
    const locations = await referencesAt(uri, declLine, declCol);

    // We expect 2 interpolation references + 1 declaration.
    assert.strictEqual(
      locations.length,
      3,
      `expected 3 locations, got ${locations.length}: ${locations
        .map((l) => `${l.range.start.line}:${l.range.start.character}`)
        .join(", ")}`,
    );
    for (const loc of locations) {
      assert.strictEqual(loc.uri.toString(), uri.toString());
    }
  });

  it("rename of a capture updates the declaration and every reference", async function () {
    this.timeout(15000);
    const uri = writeFixture(
      "symbols-rename.tarn.yaml",
      `name: Symbols rename
tests:
  crud:
    steps:
      - name: login
        request:
          method: POST
          url: "http://localhost/auth"
        capture:
          token: "$.token"
      - name: fetch
        request:
          method: GET
          url: "http://localhost/users/{{ capture.token }}"
      - name: delete
        request:
          method: DELETE
          url: "http://localhost/users/{{ capture.token }}"
`,
    );
    createdFiles.push(uri);
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc);

    const lines = doc.getText().split("\n");
    const declLine = lines.findIndex((l) => l.includes("token: "));
    const declCol = lines[declLine].indexOf("token") + 2;

    const edit = await renameAt(uri, declLine, declCol, "session");
    assert.ok(edit, "rename should return a WorkspaceEdit");
    const textEdits = edit.get(uri);
    assert.strictEqual(
      textEdits.length,
      3,
      `expected 3 edits (1 declaration + 2 refs), got ${textEdits.length}`,
    );

    // Apply the edit and verify the resulting text.
    await vscode.workspace.applyEdit(edit);
    const newText = doc.getText();
    assert.ok(newText.includes("session: "), "declaration should be renamed");
    assert.ok(
      newText.includes("{{ capture.session }}"),
      "references should be renamed",
    );
    assert.ok(
      !newText.includes("{{ capture.token }}"),
      "old references should be gone",
    );
    // The edit only mutates the in-memory document; the next test
    // overwrites the fixture file on disk, so test isolation is
    // preserved without any explicit revert.
  });

  it("rename rejects invalid capture names", async function () {
    this.timeout(15000);
    const uri = writeFixture(
      "symbols-rename-invalid.tarn.yaml",
      `name: Symbols rename invalid
tests:
  t:
    steps:
      - name: login
        request:
          method: POST
          url: "http://localhost/auth"
        capture:
          token: "$.token"
`,
    );
    createdFiles.push(uri);
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc);

    const lines = doc.getText().split("\n");
    const declLine = lines.findIndex((l) => l.includes("token: "));
    const declCol = lines[declLine].indexOf("token") + 2;

    let caught = false;
    try {
      await renameAt(uri, declLine, declCol, "1bad-name");
    } catch {
      caught = true;
    }
    assert.strictEqual(caught, true, "expected rename to reject invalid name");
  });

  it("prepareRename on a capture returns the identifier placeholder", async function () {
    this.timeout(15000);
    const uri = writeFixture(
      "symbols-prepare-rename.tarn.yaml",
      `name: Symbols prepare
tests:
  t:
    steps:
      - name: login
        request:
          method: POST
          url: "http://localhost/auth"
        capture:
          auth_token: "$.token"
      - name: use
        request:
          method: GET
          url: "http://localhost/{{ capture.auth_token }}"
`,
    );
    createdFiles.push(uri);
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc);

    const lines = doc.getText().split("\n");
    const refLine = lines.findIndex((l) => l.includes("{{ capture.auth_token }}"));
    const refCol = lines[refLine].indexOf("auth_token") + 2;
    const prep = await prepareRenameAt(uri, refLine, refCol);
    assert.strictEqual(prep.placeholder, "auth_token");
    // Rename range should cover only the identifier, not the whole
    // `{{ capture.auth_token }}` token.
    const renamedText = doc.getText(prep.range);
    assert.strictEqual(renamedText, "auth_token");
  });
});
