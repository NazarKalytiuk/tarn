import { describe, it, expect } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";

/**
 * Golden-snapshot test for the extension's public API surface.
 *
 * Purpose: any change to the `TarnExtensionApi` or
 * `TarnExtensionTestingApi` interface in `src/api.ts` — adding a
 * field, removing a field, renaming a field, widening a parameter,
 * narrowing a return, etc. — must be accompanied by an explicit
 * update to the golden snapshot at
 * `tests/golden/api.snapshot.txt`. The test fails loudly if the
 * normalized declaration in `src/api.ts` drifts from the snapshot
 * without the snapshot being updated in the same commit.
 *
 * This is the CI-enforced "did you mean to change the public API?"
 * gate referenced by NAZ-285 and by the Public API section of
 * `docs/VSCODE_EXTENSION.md`. Adding or removing a `@stability`
 * annotation in the JSDoc is intentionally part of the normalized
 * snapshot too — stability annotations are load-bearing metadata,
 * not decoration.
 *
 * Normalization rules (see `normalizeApiDeclaration` below):
 *   - Strip block comments that do NOT mention `@stability`.
 *   - Strip line comments that do NOT mention `@stability`.
 *   - A block comment is either kept whole or stripped whole: we
 *     never touch individual lines inside a JSDoc. This keeps the
 *     file-level semver-policy comment in the snapshot so that any
 *     change to the policy prose also trips the test.
 *   - Strip blank lines.
 *   - Collapse runs of whitespace inside a line to a single space.
 *   - Trim each line.
 *   - Keep `import type ...` and `export interface ...` lines
 *     exactly (after whitespace normalization) so renames of the
 *     imported source modules are also caught as breaking changes.
 */

const apiFilePath = path.resolve(__dirname, "../../src/api.ts");
const goldenFilePath = path.resolve(
  __dirname,
  "../golden/api.snapshot.txt",
);

function normalizeApiDeclaration(source: string): string {
  // Normalize line endings first so a Windows checkout (CRLF) produces
  // the same normalized output as a Linux/macOS checkout (LF). Without
  // this, the trailing \r on every line survives split("\n") and trips
  // the snapshot comparison on windows-latest in CI.
  const lfSource = source.replace(/\r\n/g, "\n");
  // Strip block comments that do NOT mention @stability. A multi-line
  // block comment either keeps every line (if any line mentions
  // @stability) or is stripped entirely.
  const withoutBlockComments = lfSource.replace(
    /\/\*[\s\S]*?\*\//g,
    (match) => (match.includes("@stability") ? match : ""),
  );

  const lines = withoutBlockComments.split("\n");
  const cleaned: string[] = [];
  for (const rawLine of lines) {
    // Strip line comments that do NOT mention @stability.
    const withoutLineComment = rawLine.replace(/\/\/.*$/, (match) =>
      match.includes("@stability") ? match : "",
    );
    const collapsed = withoutLineComment.replace(/\s+/g, " ").trim();
    if (collapsed.length === 0) continue;
    cleaned.push(collapsed);
  }
  return cleaned.join("\n") + "\n";
}

describe("TarnExtensionApi surface", () => {
  it("matches the golden snapshot (src/api.ts has not drifted)", () => {
    const apiSource = fs.readFileSync(apiFilePath, "utf8");
    // Normalize the golden's line endings too, for the same reason we
    // strip \r from the api source: Windows git checkouts may rewrite
    // text files to CRLF on checkout even though the committed file is
    // LF-only. The test must agree with itself regardless of platform.
    const golden = fs
      .readFileSync(goldenFilePath, "utf8")
      .replace(/\r\n/g, "\n");

    const normalized = normalizeApiDeclaration(apiSource);

    if (normalized !== golden) {
      const hint = [
        "",
        "The public API declaration in editors/vscode/src/api.ts has",
        "drifted from editors/vscode/tests/golden/api.snapshot.txt.",
        "",
        "If this is an intentional API change:",
        "  1. Decide the stability level of every new/changed field",
        "     (@stability stable | preview | internal).",
        "  2. Update the 'Public API' section in docs/VSCODE_EXTENSION.md",
        "     and editors/vscode/docs/API.md to match.",
        "  3. Add a CHANGELOG entry noting the change and its semver",
        "     impact (major for breaking stable, minor for additive,",
        "     any for preview/internal).",
        "  4. Regenerate the golden with:",
        "        node -e \"const fs=require('fs');const p=require('path');" +
          "const src=fs.readFileSync(p.resolve('src/api.ts'),'utf8');" +
          "const out=src.replace(/\\/\\*[\\s\\S]*?\\*\\//g,(m)=>m.includes('@stability')?m:'')" +
          ".split('\\n').map((l)=>l.replace(/\\/\\/.*$/,(m)=>m.includes('@stability')?m:'')" +
          ".replace(/\\s+/g,' ').trim()).filter((l)=>l.length>0).join('\\n')+'\\n';" +
          "fs.writeFileSync(p.resolve('tests/golden/api.snapshot.txt'),out);\"",
        "",
        "If you did NOT intend to change the public API, revert your",
        "edit to src/api.ts.",
        "",
      ].join("\n");
      // Surface the diff inline so CI logs show exactly what changed.
      expect(normalized, hint).toBe(golden);
    }
  });

  it("mentions every stability tier in the semver policy block", () => {
    const apiSource = fs.readFileSync(apiFilePath, "utf8");
    // The file-level block comment must document all three tiers.
    expect(apiSource).toMatch(/@stability stable/);
    expect(apiSource).toMatch(/@stability preview/);
    expect(apiSource).toMatch(/@stability internal/);
  });

  it("annotates every field of TarnExtensionApi with @stability", () => {
    const apiSource = fs.readFileSync(apiFilePath, "utf8");
    // Pull the TarnExtensionApi interface body out.
    const match = apiSource.match(
      /export interface TarnExtensionApi \{([\s\S]*?)\n\}/,
    );
    expect(
      match,
      "api.ts must export an interface named TarnExtensionApi",
    ).toBeTruthy();
    const body = match![1];

    // Every `readonly <field>` declaration in the interface body
    // must be preceded by a JSDoc block that contains @stability.
    // We verify by scanning the body for `readonly` tokens and
    // checking the preceding 30 lines for an @stability tag that
    // isn't separated by a blank line past another `readonly`.
    const bodyLines = body.split("\n");
    for (let i = 0; i < bodyLines.length; i += 1) {
      const line = bodyLines[i].trim();
      if (!line.startsWith("readonly ")) continue;
      // Walk backwards until we either hit another `readonly` (no
      // annotation) or find an @stability tag.
      let found = false;
      for (let j = i - 1; j >= 0; j -= 1) {
        const prev = bodyLines[j].trim();
        if (prev.startsWith("readonly ")) break;
        if (prev.includes("@stability")) {
          found = true;
          break;
        }
      }
      expect(
        found,
        `TarnExtensionApi field missing @stability annotation: ${line}`,
      ).toBe(true);
    }
  });

  it("marks the testing sub-object as @stability internal", () => {
    const apiSource = fs.readFileSync(apiFilePath, "utf8");
    // The `testing` field must carry `@stability internal` in its
    // JSDoc. We look for the `readonly testing:` line and walk
    // backwards to find @stability internal within its comment
    // block.
    const lines = apiSource.split("\n");
    const idx = lines.findIndex((l) => /readonly testing:/.test(l));
    expect(
      idx,
      "TarnExtensionApi must expose a `testing` sub-object",
    ).toBeGreaterThan(-1);
    const window = lines.slice(Math.max(0, idx - 20), idx).join("\n");
    expect(window).toMatch(/@stability internal/);
  });
});
