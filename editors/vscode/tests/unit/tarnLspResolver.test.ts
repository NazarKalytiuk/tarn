import { describe, it, expect } from "vitest";
import * as path from "path";
import { resolveTarnLspCommand } from "../../src/lsp/tarnLspResolver";

/**
 * Unit tests for the pure `resolveTarnLspCommand` helper (Phase V1
 * / NAZ-309). Mirrors the shape of the `tarn` binary-resolver
 * flow in `src/backend/binaryResolver.ts`: setting → absolute path
 * OR setting → PATH-resolvable command, with sensible defaults
 * when the user has no explicit override.
 *
 * The impure `resolveTarnLspBinary` wrapper (which `await`s an
 * `fs.access` on absolute paths) is covered by the integration
 * test that actually points at a real `target/debug/tarn-lsp`.
 */

describe("resolveTarnLspCommand: pure setting → command mapping", () => {
  it("falls back to the bare 'tarn-lsp' command when the setting is undefined", () => {
    expect(resolveTarnLspCommand(undefined)).toBe("tarn-lsp");
  });

  it("falls back to the bare 'tarn-lsp' command when the setting is empty", () => {
    expect(resolveTarnLspCommand("")).toBe("tarn-lsp");
    expect(resolveTarnLspCommand("   ")).toBe("tarn-lsp");
  });

  it("returns a bare non-empty setting unchanged (resolved via $PATH at spawn)", () => {
    expect(resolveTarnLspCommand("tarn-lsp")).toBe("tarn-lsp");
    expect(resolveTarnLspCommand("tarn-lsp-nightly")).toBe("tarn-lsp-nightly");
  });

  it("normalizes absolute paths via path.resolve", () => {
    const abs = path.resolve("/tmp/tarn-lsp");
    expect(resolveTarnLspCommand(abs)).toBe(abs);
  });

  it("trims whitespace around a configured value", () => {
    expect(resolveTarnLspCommand("  tarn-lsp  ")).toBe("tarn-lsp");
    const abs = path.resolve("/tmp/tarn-lsp");
    expect(resolveTarnLspCommand(`  ${abs}  `)).toBe(abs);
  });
});
