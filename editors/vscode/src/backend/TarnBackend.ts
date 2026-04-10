import * as vscode from "vscode";
import type { EnvReport, Report, ValidateReport } from "../util/schemaGuards";

export interface RunOptions {
  files: string[];
  cwd: string;
  environment?: string | null;
  tags?: string[];
  /** Array of FILE[::TEST[::STEP]] selectors forwarded as --select. */
  selectors?: string[];
  vars?: Record<string, string>;
  dryRun?: boolean;
  parallel?: boolean;
  jsonMode?: "verbose" | "compact";
  /** When true, spawn with --ndjson and stream events via onEvent. */
  streamNdjson?: boolean;
  onEvent?: (event: NdjsonEvent) => void;
  token: vscode.CancellationToken;
}

export interface RunOutcome {
  report: Report | undefined;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  cancelled: boolean;
}

export type NdjsonEvent =
  | { event: "file_started"; file: string; file_name: string }
  | {
      event: "step_finished";
      file: string;
      phase: "setup" | "test" | "teardown";
      test: string;
      step: string;
      step_index: number;
      status: "PASSED" | "FAILED";
      duration_ms: number;
      failure_category?: string;
      error_code?: string;
      assertion_failures?: Array<{
        assertion: string;
        expected?: string;
        actual?: string;
        message?: string;
        diff?: string;
      }>;
    }
  | {
      event: "test_finished";
      file: string;
      test: string;
      status: "PASSED" | "FAILED";
      duration_ms: number;
      steps: { total: number; passed: number; failed: number };
    }
  | {
      event: "file_finished";
      file: string;
      file_name: string;
      status: "PASSED" | "FAILED";
      duration_ms: number;
      summary: { total: number; passed: number; failed: number };
    }
  | {
      event: "done";
      duration_ms: number;
      summary: {
        files: number;
        tests: number;
        steps: { total: number; passed: number; failed: number };
        status: "PASSED" | "FAILED";
      };
    };

export interface TarnBackend {
  run(options: RunOptions): Promise<RunOutcome>;
  validate(
    files: string[],
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }>;
  /**
   * Structured validate: spawns `tarn validate --format json` and parses
   * the output. Returns `undefined` if the process failed or the stdout
   * could not be parsed as a valid report (e.g., older Tarn versions
   * without T52).
   */
  validateStructured(
    files: string[],
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<ValidateReport | undefined>;
  /**
   * Structured environments: spawns `tarn env --json` and parses the
   * output. Returns `undefined` on failure (missing config, older Tarn
   * without T56, parse errors) so callers can decide whether to fall
   * back to the file-glob discovery path.
   */
  envStructured(
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<EnvReport | undefined>;
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
