import * as vscode from "vscode";

export type CookieJarMode = "default" | "per-test";
export type BackendKind = "cli" | "mcp";

export interface TarnConfig {
  binaryPath: string;
  testFileGlob: string;
  excludeGlobs: string[];
  defaultEnvironment: string | null;
  defaultTags: string[];
  parallel: boolean;
  jsonMode: "verbose" | "compact";
  requestTimeoutMs: number;
  showCodeLens: boolean;
  statusBarEnabled: boolean;
  validateOnSave: boolean;
  notificationsFailure: "always" | "focused" | "off";
  cookieJarMode: CookieJarMode;
}

export function readConfig(scope?: vscode.Uri): TarnConfig {
  const cfg = vscode.workspace.getConfiguration("tarn", scope);
  return {
    binaryPath: cfg.get<string>("binaryPath", "tarn"),
    testFileGlob: cfg.get<string>("testFileGlob", "**/*.tarn.yaml"),
    excludeGlobs: cfg.get<string[]>("excludeGlobs", [
      "**/target/**",
      "**/node_modules/**",
      "**/.git/**",
    ]),
    defaultEnvironment: cfg.get<string | null>("defaultEnvironment", null),
    defaultTags: cfg.get<string[]>("defaultTags", []),
    parallel: cfg.get<boolean>("parallel", true),
    jsonMode: cfg.get<"verbose" | "compact">("jsonMode", "verbose"),
    requestTimeoutMs: cfg.get<number>("requestTimeoutMs", 120000),
    showCodeLens: cfg.get<boolean>("showCodeLens", true),
    statusBarEnabled: cfg.get<boolean>("statusBar.enabled", true),
    validateOnSave: cfg.get<boolean>("validateOnSave", true),
    notificationsFailure: cfg.get<"always" | "focused" | "off">(
      "notifications.failure",
      "focused",
    ),
    cookieJarMode: normalizeCookieJarMode(
      cfg.get<string>("cookieJarMode", "default"),
    ),
  };
}

/**
 * Read the `tarn.experimentalLspClient` feature flag.
 *
 * Phase V1 (NAZ-309) introduces a side-by-side `vscode-languageclient`
 * host that talks to the `tarn-lsp` Rust binary. The direct providers
 * continue to run regardless of this flag; the flag only controls
 * whether the extension additionally spawns `tarn-lsp` and registers
 * its language features. Default is `false` — Phase V2 feature
 * tickets flip it via workspace settings in their own integration
 * tests, and Phase V3 deletes the flag when migration is complete.
 *
 * `window` scope (not `resource`) because spawning or killing an LSP
 * server per-folder is both unnecessary and expensive for a feature
 * that is inherently per-VS-Code-window.
 */
export function getExperimentalLspClient(scope?: vscode.Uri): boolean {
  const cfg = vscode.workspace.getConfiguration("tarn", scope);
  return cfg.get<boolean>("experimentalLspClient", false);
}

/**
 * Read the `tarn.backend` setting (NAZ-279).
 *
 * Controls which backend the extension uses to run tests:
 *
 * - `"cli"` (default): spawn the `tarn` CLI per command. Every run
 *   starts a fresh process; NDJSON streaming is available.
 * - `"mcp"`: keep a `tarn-mcp` process alive per workspace and dispatch
 *   each command as a JSON-RPC request over stdio. NDJSON streaming is
 *   not supported — the backend degrades to returning only the final
 *   JSON report.
 *
 * Unknown values fall back to `"cli"` so a typo in user settings never
 * leaves the extension without a working backend.
 */
export function readBackendKind(scope?: vscode.Uri): BackendKind {
  const cfg = vscode.workspace.getConfiguration("tarn", scope);
  const raw = cfg.get<string>("backend", "cli");
  return raw === "mcp" ? "mcp" : "cli";
}

/**
 * Read the raw `tarn.mcpPath` setting value. Unlike {@link readConfig},
 * this helper returns `undefined` when the key is absent or blank so
 * {@link resolveMcpCommand} can distinguish "user did not override"
 * from "user explicitly set an empty string" (both collapse to the
 * bare `"tarn-mcp"` command resolved via `$PATH`).
 */
export function readMcpPath(scope?: vscode.Uri): string | undefined {
  const cfg = vscode.workspace.getConfiguration("tarn", scope);
  return cfg.get<string>("mcpPath");
}

/**
 * Narrow a raw `tarn.cookieJarMode` value to a known mode. Unknown or
 * malformed values fall back to `"default"` so a typo in user settings
 * never breaks the runner — the worst case is honoring the file's
 * declared `cookies:` mode, which is the safe default.
 */
export function normalizeCookieJarMode(raw: string | undefined): CookieJarMode {
  return raw === "per-test" ? "per-test" : "default";
}

export function buildExcludeGlob(globs: string[]): string | undefined {
  if (globs.length === 0) {
    return undefined;
  }
  if (globs.length === 1) {
    return globs[0];
  }
  return `{${globs.join(",")}}`;
}
