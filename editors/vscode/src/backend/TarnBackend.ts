import * as vscode from "vscode";
import type { Report } from "../util/schemaGuards";

export interface RunOptions {
  files: string[];
  cwd: string;
  environment?: string | null;
  tags?: string[];
  vars?: Record<string, string>;
  dryRun?: boolean;
  parallel?: boolean;
  jsonMode?: "verbose" | "compact";
  token: vscode.CancellationToken;
}

export interface RunOutcome {
  report: Report | undefined;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  cancelled: boolean;
}

export interface TarnBackend {
  run(options: RunOptions): Promise<RunOutcome>;
  validate(files: string[], cwd: string, token: vscode.CancellationToken): Promise<{ exitCode: number | null; stdout: string; stderr: string }>;
  exportCurl(
    files: string[],
    cwd: string,
    mode: "all" | "failed",
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }>;
  initProject(
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }>;
}
