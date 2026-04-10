import * as vscode from "vscode";

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
  };
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
