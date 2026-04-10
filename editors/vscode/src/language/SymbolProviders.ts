import * as path from "path";
import * as vscode from "vscode";
import type { EnvironmentsView } from "../views/EnvironmentsView";
import {
  buildCaptureIndex,
  findCaptureReferences,
  type CaptureDeclaration,
} from "./completion/captures";
import { findHoverToken, type HoverToken } from "./completion/hoverToken";

/**
 * Detect what symbol the cursor points at: an `env.KEY` interpolation,
 * a `capture.NAME` interpolation, or a capture declaration key inside
 * a `capture: { ... }` block. Returns `undefined` when the cursor is
 * not on any Tarn symbol.
 */
export type CursorSymbol =
  | { kind: "env"; key: string; tokenRange: vscode.Range }
  | { kind: "capture-ref"; name: string; tokenRange: vscode.Range }
  | { kind: "capture-decl"; name: string; declaration: CaptureDeclaration };

export function cursorSymbol(
  document: vscode.TextDocument,
  position: vscode.Position,
): CursorSymbol | undefined {
  const lineText = document.lineAt(position.line).text;
  const token = findHoverToken(lineText, position.character);
  const hit = hoverTokenToSymbol(token, position.line);
  if (hit) {
    return hit;
  }

  // Not inside an interpolation — might be sitting on a capture
  // declaration key like `auth_token: "$.token"` inside a capture:
  // block. Use the CST index to resolve by offset.
  const source = document.getText();
  const offset = document.offsetAt(position);
  const index = buildCaptureIndex(source);
  if (!index) {
    return undefined;
  }
  const decl = index.findDeclarationAt(offset);
  if (decl) {
    return { kind: "capture-decl", name: decl.name, declaration: decl };
  }
  return undefined;
}

function hoverTokenToSymbol(
  token: HoverToken,
  line: number,
): CursorSymbol | undefined {
  if (token.kind === "env" && token.identifier) {
    return {
      kind: "env",
      key: token.identifier,
      tokenRange: new vscode.Range(
        new vscode.Position(line, token.rangeStart),
        new vscode.Position(line, token.rangeEnd),
      ),
    };
  }
  if (token.kind === "capture" && token.identifier) {
    return {
      kind: "capture-ref",
      name: token.identifier,
      tokenRange: new vscode.Range(
        new vscode.Position(line, token.rangeStart),
        new vscode.Position(line, token.rangeEnd),
      ),
    };
  }
  return undefined;
}

// ---------------------------------------------------------------------------
// Definition
// ---------------------------------------------------------------------------

export class TarnDefinitionProvider implements vscode.DefinitionProvider {
  constructor(private readonly environmentsView: EnvironmentsView) {}

  async provideDefinition(
    document: vscode.TextDocument,
    position: vscode.Position,
  ): Promise<vscode.Location | vscode.Location[] | undefined> {
    const symbol = cursorSymbol(document, position);
    if (!symbol) {
      return undefined;
    }

    if (symbol.kind === "env") {
      return this.envDefinitions(symbol.key);
    }

    if (symbol.kind === "capture-ref") {
      return captureDeclarationsAsLocations(document, symbol.name);
    }

    if (symbol.kind === "capture-decl") {
      // Clicking "go to definition" on the definition jumps back to
      // itself — match the VS Code built-in behavior for other
      // languages (stays put).
      const range = rangeFromByteOffsets(
        document,
        symbol.declaration.keyStart,
        symbol.declaration.keyEnd,
      );
      return new vscode.Location(document.uri, range);
    }

    return undefined;
  }

  private async envDefinitions(key: string): Promise<vscode.Location[]> {
    const entries = await this.environmentsView.getEntries();
    const declaring = entries.filter((e) =>
      Object.prototype.hasOwnProperty.call(e.vars, key),
    );
    if (declaring.length === 0) {
      return [];
    }
    const folder = vscode.workspace.workspaceFolders?.[0];
    if (!folder) {
      return [];
    }

    const locations: vscode.Location[] = [];
    const seenUris = new Set<string>();
    for (const entry of declaring) {
      const sourceUri = path.isAbsolute(entry.source_file)
        ? vscode.Uri.file(entry.source_file)
        : vscode.Uri.joinPath(folder.uri, entry.source_file);
      const uriKey = sourceUri.toString();
      if (seenUris.has(uriKey)) {
        continue;
      }
      seenUris.add(uriKey);
      const range = await findKeyRangeInFile(sourceUri, key);
      locations.push(new vscode.Location(sourceUri, range));
    }
    return locations;
  }
}

async function findKeyRangeInFile(
  uri: vscode.Uri,
  key: string,
): Promise<vscode.Range> {
  try {
    const bytes = await vscode.workspace.fs.readFile(uri);
    const text = Buffer.from(bytes).toString("utf8");
    const lines = text.split(/\r?\n/);
    const pattern = new RegExp(`^(\\s*)${escapeRegExp(key)}\\s*:`);
    for (let i = 0; i < lines.length; i++) {
      const match = pattern.exec(lines[i]);
      if (match) {
        const col = match[1].length;
        return new vscode.Range(
          new vscode.Position(i, col),
          new vscode.Position(i, col + key.length),
        );
      }
    }
  } catch {
    // fall through to a line-zero range
  }
  return new vscode.Range(new vscode.Position(0, 0), new vscode.Position(0, 0));
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function captureDeclarationsAsLocations(
  document: vscode.TextDocument,
  name: string,
): vscode.Location[] {
  const index = buildCaptureIndex(document.getText());
  if (!index) {
    return [];
  }
  const decls = index.findByName(name);
  return decls.map((decl) => {
    const range = rangeFromByteOffsets(document, decl.keyStart, decl.keyEnd);
    return new vscode.Location(document.uri, range);
  });
}

function rangeFromByteOffsets(
  document: vscode.TextDocument,
  start: number,
  end: number,
): vscode.Range {
  return new vscode.Range(document.positionAt(start), document.positionAt(end));
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

export class TarnReferencesProvider implements vscode.ReferenceProvider {
  provideReferences(
    document: vscode.TextDocument,
    position: vscode.Position,
    context: vscode.ReferenceContext,
  ): vscode.ProviderResult<vscode.Location[]> {
    const symbol = cursorSymbol(document, position);
    if (!symbol) {
      return [];
    }

    let name: string | undefined;
    let includeDeclaration = context.includeDeclaration;
    if (symbol.kind === "capture-ref") {
      name = symbol.name;
    } else if (symbol.kind === "capture-decl") {
      name = symbol.name;
      includeDeclaration = true;
    } else {
      // Env references aren't file-scoped, so they're out of scope for
      // v1. Return nothing rather than lying.
      return [];
    }
    if (!name) {
      return [];
    }

    const source = document.getText();
    const locations: vscode.Location[] = [];

    for (const ref of findCaptureReferences(source, name)) {
      locations.push(
        new vscode.Location(
          document.uri,
          rangeFromByteOffsets(document, ref.nameStart, ref.nameEnd),
        ),
      );
    }

    if (includeDeclaration) {
      const index = buildCaptureIndex(source);
      if (index) {
        for (const decl of index.findByName(name)) {
          locations.push(
            new vscode.Location(
              document.uri,
              rangeFromByteOffsets(document, decl.keyStart, decl.keyEnd),
            ),
          );
        }
      }
    }

    return locations;
  }
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

export class TarnRenameProvider implements vscode.RenameProvider {
  prepareRename(
    document: vscode.TextDocument,
    position: vscode.Position,
  ): vscode.ProviderResult<vscode.Range | { range: vscode.Range; placeholder: string }> {
    const symbol = cursorSymbol(document, position);
    if (!symbol) {
      throw new Error("Rename is only supported on capture declarations or references.");
    }
    if (symbol.kind === "env") {
      throw new Error(
        "Renaming env keys is not supported — edit the source env file directly.",
      );
    }

    const source = document.getText();
    const index = buildCaptureIndex(source);
    if (!index) {
      throw new Error(
        "Cannot rename: this file has YAML parse errors. Fix them first.",
      );
    }

    const name =
      symbol.kind === "capture-decl" ? symbol.name : symbol.name;
    const declarations = index.findByName(name);
    if (declarations.length === 0) {
      throw new Error(
        `Rename rejected: capture '${name}' is not declared in this file. It may come from an \`include:\` directive; edit the included file instead.`,
      );
    }

    if (symbol.kind === "capture-decl") {
      return {
        range: rangeFromByteOffsets(
          document,
          symbol.declaration.keyStart,
          symbol.declaration.keyEnd,
        ),
        placeholder: name,
      };
    }

    // capture-ref: return the range of just the identifier, not the
    // whole `{{ capture.name }}` token, so VS Code's rename widget
    // shows only the name.
    const refs = findCaptureReferences(source, name);
    const offset = document.offsetAt(position);
    const hit = refs.find((r) => offset >= r.nameStart && offset <= r.nameEnd);
    if (hit) {
      return {
        range: rangeFromByteOffsets(document, hit.nameStart, hit.nameEnd),
        placeholder: name,
      };
    }
    // Fallback: token range from the hover finder.
    return { range: symbol.tokenRange, placeholder: name };
  }

  provideRenameEdits(
    document: vscode.TextDocument,
    position: vscode.Position,
    newName: string,
  ): vscode.ProviderResult<vscode.WorkspaceEdit> {
    if (!isValidCaptureName(newName)) {
      throw new Error(
        "Invalid capture name. Use letters, digits, and underscores; must not start with a digit.",
      );
    }
    const symbol = cursorSymbol(document, position);
    if (!symbol || symbol.kind === "env") {
      throw new Error("Rename is only supported on captures.");
    }
    const name = symbol.kind === "capture-decl" ? symbol.name : symbol.name;

    const source = document.getText();
    const index = buildCaptureIndex(source);
    if (!index) {
      throw new Error("Cannot rename: file has YAML parse errors.");
    }
    const decls = index.findByName(name);
    if (decls.length === 0) {
      throw new Error(
        `Rename rejected: capture '${name}' is not declared in this file.`,
      );
    }

    const edit = new vscode.WorkspaceEdit();
    for (const decl of decls) {
      edit.replace(
        document.uri,
        rangeFromByteOffsets(document, decl.keyStart, decl.keyEnd),
        newName,
      );
    }
    for (const ref of findCaptureReferences(source, name)) {
      edit.replace(
        document.uri,
        rangeFromByteOffsets(document, ref.nameStart, ref.nameEnd),
        newName,
      );
    }
    return edit;
  }
}

function isValidCaptureName(name: string): boolean {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);
}
