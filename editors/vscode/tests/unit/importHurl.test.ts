import { describe, it, expect } from "vitest";
import * as path from "path";
import { defaultHurlDestination } from "../../src/commands/importHurl";

describe("defaultHurlDestination", () => {
  it("swaps the .hurl extension for .tarn.yaml in the same directory", () => {
    expect(defaultHurlDestination("/src/foo.hurl")).toBe(
      path.normalize("/src/foo.tarn.yaml"),
    );
  });

  it("preserves dotted base names when stripping .hurl", () => {
    expect(defaultHurlDestination("/tests/foo.bar.hurl")).toBe(
      path.normalize("/tests/foo.bar.tarn.yaml"),
    );
  });

  it("treats non-.hurl files by just appending .tarn.yaml", () => {
    // Unusual but defensive — we should never mangle the filename.
    expect(defaultHurlDestination("/src/noext")).toBe(
      path.normalize("/src/noext.tarn.yaml"),
    );
  });

  it("is case-insensitive for the .hurl suffix", () => {
    expect(defaultHurlDestination("/src/API.HURL")).toBe(
      path.normalize("/src/API.tarn.yaml"),
    );
  });

  it("handles deeply nested paths", () => {
    expect(defaultHurlDestination("/a/b/c/d/e.hurl")).toBe(
      path.normalize("/a/b/c/d/e.tarn.yaml"),
    );
  });
});
