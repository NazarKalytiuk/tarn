import { execFile } from "child_process";
import { promisify } from "util";
import * as vscode from "vscode";
import { getOutputChannel } from "./outputChannel";

const execFileAsync = promisify(execFile);

/**
 * Tarn CLI <-> VS Code extension version alignment (NAZ-288).
 *
 * The extension ships against a known-good minimum Tarn CLI version.
 * The minimum is declared once in `package.json` as
 *
 *     {
 *       "tarn": { "minVersion": "X.Y.Z" }
 *     }
 *
 * at the top level (next to `version` and `l10n`, not under
 * `contributes`). Under the coordinated-release policy introduced in
 * Phase 6, the extension version and the Tarn CLI minor track each
 * other: extension `X.Y.*` is tested against Tarn `X.Y.*`, so the
 * minor number always matches. Patch numbers may diverge for bug-fix
 * releases on one side without a matching release on the other, which
 * is why we guard `minVersion` rather than requiring an exact match.
 *
 * At activation time we invoke the configured binary with `--version`,
 * parse the `tarn X.Y.Z` output, and compare against `minVersion`. On
 * mismatch we surface a warning notification with an "Install Tarn"
 * action that routes the user to the install docs. The check is
 * explicitly non-fatal: a user on an older Tarn still gets a working
 * extension, they just get a heads-up that some features may not light
 * up as expected. This keeps the rollout soft — a user who upgrades
 * the extension but not the binary gets a nudge, not a broken editor.
 *
 * The ExtensionContext parameter is accepted so the helper can read
 * `extension.packageJSON` without reaching into the singleton — makes
 * the code unit-testable without a running VS Code host.
 */
export interface TarnVersionCheckResult {
  /** Version string reported by the binary, e.g. `"0.5.0"`. */
  installed: string | null;
  /** Version declared as `tarn.minVersion` in package.json. */
  minVersion: string;
  /** `true` when `installed >= minVersion` (or when no min is declared). */
  ok: boolean;
}

/**
 * Parse a `tarn --version` stdout line into a bare semver triple.
 *
 * Expected input: `"tarn 0.5.0\n"` (the default `clap` derive format).
 * Returns the `"0.5.0"` portion, or `null` if the line is not a
 * recognized `tarn <semver>` signature. Pre-release suffixes like
 * `"0.5.0-rc.1"` are preserved verbatim.
 */
export function parseTarnVersion(stdout: string): string | null {
  const match = stdout.trim().match(/^tarn\s+(\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?)/);
  return match ? match[1] : null;
}

/**
 * Parse a semver triple (optionally with a `-prerelease` suffix) into
 * a `[major, minor, patch, preRelease]` tuple. Returns `null` on
 * malformed input so callers can decide whether to skip the check or
 * log a diagnostic. A missing pre-release segment is normalized to
 * `""` so the comparator can treat a release version as "higher" than
 * any pre-release of the same triple (semver rule).
 */
export function parseSemver(
  version: string,
): [number, number, number, string] | null {
  const match = version.trim().match(
    /^(\d+)\.(\d+)\.(\d+)(?:-([A-Za-z0-9.-]+))?$/,
  );
  if (!match) return null;
  const major = Number(match[1]);
  const minor = Number(match[2]);
  const patch = Number(match[3]);
  if (!Number.isFinite(major) || !Number.isFinite(minor) || !Number.isFinite(patch)) {
    return null;
  }
  return [major, minor, patch, match[4] ?? ""];
}

/**
 * Compare two semver strings. Returns -1 when `a < b`, 0 when equal,
 * and 1 when `a > b`. An unparseable input sorts as "greater" so the
 * check stays non-fatal — if we can't reason about the number we don't
 * want to fire a false-negative warning on every activation.
 */
export function compareSemver(a: string, b: string): number {
  const pa = parseSemver(a);
  const pb = parseSemver(b);
  if (!pa || !pb) return 1;
  for (let i = 0; i < 3; i++) {
    if (pa[i] < pb[i]) return -1;
    if (pa[i] > pb[i]) return 1;
  }
  // Release (empty pre-release) is greater than any pre-release of
  // the same triple, per semver 2.0.0 section 11.
  const preA = pa[3] as string;
  const preB = pb[3] as string;
  if (preA === "" && preB !== "") return 1;
  if (preA !== "" && preB === "") return -1;
  if (preA < preB) return -1;
  if (preA > preB) return 1;
  return 0;
}

/**
 * Read `tarn.minVersion` from the extension's own `package.json`,
 * falling back to `null` if the field is missing or malformed. The
 * lookup is schema-tolerant because `package.json` is authored by
 * hand — we never want a typo to block activation.
 */
export function readMinVersionFromPackage(
  packageJson: Record<string, unknown> | undefined,
): string | null {
  if (!packageJson) return null;
  const tarnSection = packageJson.tarn;
  if (!tarnSection || typeof tarnSection !== "object") return null;
  const minVersion = (tarnSection as Record<string, unknown>).minVersion;
  if (typeof minVersion !== "string" || minVersion.length === 0) return null;
  return minVersion;
}

/**
 * Compare an installed version against a declared minimum. Treats a
 * missing `installed` as "unknown, assume ok" so that a missing binary
 * (already surfaced elsewhere via `BinaryNotFoundError`) doesn't
 * double-warn the user.
 */
export function checkVersionCompatibility(
  installed: string | null,
  minVersion: string | null,
): TarnVersionCheckResult {
  const min = minVersion ?? "";
  if (!min) {
    return { installed, minVersion: "", ok: true };
  }
  if (installed === null) {
    return { installed: null, minVersion: min, ok: true };
  }
  return {
    installed,
    minVersion: min,
    ok: compareSemver(installed, min) >= 0,
  };
}

/**
 * Run `tarn --version` and return the parsed installed version.
 * Returns `null` on any error — the version check is advisory and
 * should never mask the main activation error path.
 */
export async function readInstalledTarnVersion(
  binaryPath: string,
): Promise<string | null> {
  try {
    const { stdout } = await execFileAsync(binaryPath, ["--version"], {
      timeout: 5000,
    });
    return parseTarnVersion(stdout);
  } catch {
    return null;
  }
}

/**
 * Surface a warning to the user when the installed Tarn binary is
 * older than the extension's declared `tarn.minVersion`. Wired into
 * `activate()`; safe to skip (returns silently) when the check
 * returns ok.
 */
export async function warnIfTarnOutdated(
  context: Pick<vscode.ExtensionContext, "extension">,
  binaryPath: string,
): Promise<TarnVersionCheckResult> {
  const packageJson = context.extension.packageJSON as
    | Record<string, unknown>
    | undefined;
  const minVersion = readMinVersionFromPackage(packageJson);
  const installed = await readInstalledTarnVersion(binaryPath);
  const result = checkVersionCompatibility(installed, minVersion);

  if (result.ok || !minVersion || installed === null) {
    // l10n-ignore: debug log only, shown in Tarn output channel for diagnostics.
    getOutputChannel().appendLine(
      `[tarn] version check installed=${installed ?? "unknown"} min=${
        minVersion ?? "none"
      } ok=${result.ok}`,
    );
    return result;
  }

  const installAction = vscode.l10n.t("Install Tarn");
  const choice = await vscode.window.showWarningMessage(
    vscode.l10n.t(
      "Tarn CLI {0} is older than the minimum required by this extension ({1}). Some features may not work correctly. Update Tarn to continue.",
      installed,
      minVersion,
    ),
    installAction,
  );
  if (choice === installAction) {
    await vscode.env.openExternal(
      vscode.Uri.parse("https://github.com/NazarKalytiuk/tarn#install"),
    );
  }
  return result;
}
