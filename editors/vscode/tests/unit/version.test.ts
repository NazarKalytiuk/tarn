import { describe, it, expect } from "vitest";
import * as fs from "node:fs";
import * as path from "node:path";

import {
  checkVersionCompatibility,
  compareSemver,
  parseSemver,
  parseTarnVersion,
  readMinVersionFromPackage,
} from "../../src/version";

// Unit tests for the Tarn CLI <-> extension version alignment check
// (NAZ-288). These are the pure functions — the VS Code-dependent
// `warnIfTarnOutdated` wrapper is covered separately (at runtime it
// bottoms out in these same helpers).

describe("parseTarnVersion", () => {
  it("extracts a plain semver triple from `tarn X.Y.Z`", () => {
    expect(parseTarnVersion("tarn 0.5.0")).toBe("0.5.0");
    expect(parseTarnVersion("tarn 1.2.3\n")).toBe("1.2.3");
    expect(parseTarnVersion("  tarn   12.34.567\n")).toBe("12.34.567");
  });

  it("preserves pre-release suffixes", () => {
    expect(parseTarnVersion("tarn 0.5.0-rc.1")).toBe("0.5.0-rc.1");
    expect(parseTarnVersion("tarn 0.5.0-beta\n")).toBe("0.5.0-beta");
  });

  it("returns null for unrecognized output", () => {
    expect(parseTarnVersion("")).toBeNull();
    expect(parseTarnVersion("tarn")).toBeNull();
    expect(parseTarnVersion("some other binary 1.2.3")).toBeNull();
    expect(parseTarnVersion("tarn v1.2.3")).toBeNull();
  });
});

describe("parseSemver", () => {
  it("parses a plain triple", () => {
    expect(parseSemver("0.5.0")).toEqual([0, 5, 0, ""]);
    expect(parseSemver("12.34.567")).toEqual([12, 34, 567, ""]);
  });

  it("parses a triple with a pre-release suffix", () => {
    expect(parseSemver("0.5.0-rc.1")).toEqual([0, 5, 0, "rc.1"]);
    expect(parseSemver("1.0.0-alpha")).toEqual([1, 0, 0, "alpha"]);
  });

  it("returns null for malformed input", () => {
    expect(parseSemver("1.2")).toBeNull();
    expect(parseSemver("v1.2.3")).toBeNull();
    expect(parseSemver("not-a-version")).toBeNull();
    expect(parseSemver("")).toBeNull();
  });
});

describe("compareSemver", () => {
  it("orders by major, then minor, then patch", () => {
    expect(compareSemver("1.0.0", "2.0.0")).toBe(-1);
    expect(compareSemver("2.0.0", "1.0.0")).toBe(1);
    expect(compareSemver("0.5.0", "0.5.1")).toBe(-1);
    expect(compareSemver("0.5.1", "0.5.0")).toBe(1);
    expect(compareSemver("0.4.0", "0.5.0")).toBe(-1);
    expect(compareSemver("0.5.0", "0.5.0")).toBe(0);
  });

  it("treats a release as higher than any pre-release of the same triple", () => {
    // Per semver 2.0.0 section 11: 1.0.0-alpha < 1.0.0.
    expect(compareSemver("1.0.0-alpha", "1.0.0")).toBe(-1);
    expect(compareSemver("1.0.0", "1.0.0-alpha")).toBe(1);
    expect(compareSemver("0.5.0-rc.1", "0.5.0")).toBe(-1);
  });

  it("orders pre-release suffixes lexically when triples match", () => {
    expect(compareSemver("1.0.0-alpha", "1.0.0-beta")).toBe(-1);
    expect(compareSemver("1.0.0-rc.1", "1.0.0-rc.2")).toBe(-1);
    expect(compareSemver("1.0.0-alpha", "1.0.0-alpha")).toBe(0);
  });

  it("falls back to 'greater' for unparseable versions (non-fatal)", () => {
    // We deliberately never want the version check to trigger a
    // false-negative warning on garbled output — returning 1 means
    // "installed >= min", so the check passes silently.
    expect(compareSemver("nonsense", "0.5.0")).toBe(1);
    expect(compareSemver("0.5.0", "nonsense")).toBe(1);
  });
});

describe("readMinVersionFromPackage", () => {
  it("reads the top-level tarn.minVersion field", () => {
    expect(
      readMinVersionFromPackage({ tarn: { minVersion: "0.5.0" } }),
    ).toBe("0.5.0");
  });

  it("returns null when the field is missing or malformed", () => {
    expect(readMinVersionFromPackage(undefined)).toBeNull();
    expect(readMinVersionFromPackage({})).toBeNull();
    expect(readMinVersionFromPackage({ tarn: null })).toBeNull();
    expect(readMinVersionFromPackage({ tarn: "0.5.0" })).toBeNull();
    expect(readMinVersionFromPackage({ tarn: { minVersion: "" } })).toBeNull();
    expect(readMinVersionFromPackage({ tarn: { minVersion: 5 } })).toBeNull();
  });
});

describe("checkVersionCompatibility", () => {
  it("returns ok when installed matches minVersion exactly", () => {
    const r = checkVersionCompatibility("0.5.0", "0.5.0");
    expect(r.ok).toBe(true);
    expect(r.installed).toBe("0.5.0");
    expect(r.minVersion).toBe("0.5.0");
  });

  it("returns ok when installed is newer than minVersion", () => {
    expect(checkVersionCompatibility("0.5.1", "0.5.0").ok).toBe(true);
    expect(checkVersionCompatibility("0.6.0", "0.5.0").ok).toBe(true);
    expect(checkVersionCompatibility("1.0.0", "0.5.0").ok).toBe(true);
  });

  it("fails when installed is older than minVersion", () => {
    expect(checkVersionCompatibility("0.4.9", "0.5.0").ok).toBe(false);
    expect(checkVersionCompatibility("0.4.0", "0.5.0").ok).toBe(false);
    expect(checkVersionCompatibility("0.0.1", "0.5.0").ok).toBe(false);
  });

  it("passes silently when installed is unknown (null)", () => {
    // The BinaryNotFoundError path already surfaces a more specific
    // error earlier in activation, so we don't want to double-warn.
    const r = checkVersionCompatibility(null, "0.5.0");
    expect(r.ok).toBe(true);
    expect(r.installed).toBeNull();
  });

  it("passes silently when minVersion is absent", () => {
    const r = checkVersionCompatibility("0.5.0", null);
    expect(r.ok).toBe(true);
    expect(r.minVersion).toBe("");
  });
});

// ---------------------------------------------------------------
// CI alignment lint for NAZ-288: the VS Code extension version and
// the Tarn Cargo.toml version MUST bump together. If they drift,
// this test fails the build and blocks the merge. Cheap, auditable,
// runs on every `npm run test:unit` pass.
// ---------------------------------------------------------------

describe("version alignment: editors/vscode and tarn must match", () => {
  const repoRoot = path.resolve(__dirname, "../../../..");
  const extensionPackageJsonPath = path.join(
    repoRoot,
    "editors/vscode/package.json",
  );
  const cargoTomlPath = path.join(repoRoot, "tarn/Cargo.toml");

  function readExtensionVersion(): string {
    const pkg = JSON.parse(fs.readFileSync(extensionPackageJsonPath, "utf8")) as {
      version: string;
    };
    return pkg.version;
  }

  function readExtensionMinVersion(): string | null {
    const pkg = JSON.parse(fs.readFileSync(extensionPackageJsonPath, "utf8")) as {
      tarn?: { minVersion?: string };
    };
    return pkg.tarn?.minVersion ?? null;
  }

  function readCargoVersion(): string {
    const src = fs.readFileSync(cargoTomlPath, "utf8");
    // Match the first version in the [package] section. The [[bin]]
    // and [dependencies] sections have their own `version =` lines,
    // so we restrict ourselves to the first top-level table.
    const pkgSection = src.match(
      /\[package\]([\s\S]*?)(?:\n\[|\n$|$)/,
    );
    if (!pkgSection) {
      throw new Error(`No [package] section found in ${cargoTomlPath}`);
    }
    const versionLine = pkgSection[1].match(/^version\s*=\s*"([^"]+)"/m);
    if (!versionLine) {
      throw new Error(`No version = "..." in [package] of ${cargoTomlPath}`);
    }
    return versionLine[1];
  }

  it("editors/vscode/package.json major.minor matches tarn/Cargo.toml major.minor", () => {
    // The NAZ-288 coordinated-release policy (documented in the
    // extension CHANGELOG under "Version alignment policy") states:
    //
    //   "Extension X.Y.* tracks Tarn X.Y.*: the minor number is
    //    always identical, so a user on extension 0.5.x knows
    //    they can run any Tarn 0.5.x. Patch numbers may diverge
    //    for bug-fix releases on one side without a matching
    //    release on the other."
    //
    // Earlier iterations of this lint compared the full version
    // triple, which was stricter than the documented policy and
    // broke the moment an L-phase ticket patch-bumped `tarn` for
    // a `tarn-lsp`-only fix without a matching extension release.
    // We relax the comparison to major.minor to match the stated
    // policy exactly: a minor mismatch still fails the build, a
    // patch drift does not.
    const extensionVersion = readExtensionVersion();
    const cargoVersion = readCargoVersion();
    const extParsed = parseSemver(extensionVersion);
    const cargoParsed = parseSemver(cargoVersion);
    expect(extParsed, `editors/vscode/package.json version (${extensionVersion}) is not valid semver`)
      .not.toBeNull();
    expect(cargoParsed, `tarn/Cargo.toml version (${cargoVersion}) is not valid semver`)
      .not.toBeNull();
    const [extMajor, extMinor] = extParsed!;
    const [cargoMajor, cargoMinor] = cargoParsed!;
    expect(
      `${extMajor}.${extMinor}`,
      `editors/vscode/package.json major.minor (${extMajor}.${extMinor}) must equal ` +
        `tarn/Cargo.toml [package] major.minor (${cargoMajor}.${cargoMinor}). ` +
        `Bump both in the same commit per the NAZ-288 coordinated-release policy. ` +
        `Patch numbers may diverge.`,
    ).toBe(`${cargoMajor}.${cargoMinor}`);
  });

  it("tarn.minVersion is declared and parseable", () => {
    const minVersion = readExtensionMinVersion();
    expect(minVersion).not.toBeNull();
    expect(parseSemver(minVersion!)).not.toBeNull();
  });

  it("tarn.minVersion is less than or equal to the extension version", () => {
    // Rationale: the minimum Tarn we require must not exceed the
    // version we're currently shipping. Otherwise a user who
    // installs the VSIX and `tarn` from the same coordinated release
    // still sees a mismatch warning.
    const extensionVersion = readExtensionVersion();
    const minVersion = readExtensionMinVersion();
    expect(minVersion).not.toBeNull();
    expect(compareSemver(minVersion!, extensionVersion)).toBeLessThanOrEqual(0);
  });
});
