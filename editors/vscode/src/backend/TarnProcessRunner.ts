import { spawn } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as readline from "readline";
import * as vscode from "vscode";
import { getOutputChannel } from "../outputChannel";
import { formatCommandForLog } from "../util/shellEscape";
import {
  parseEnvReport,
  parseReport,
  parseValidateReport,
  type EnvReport,
  type ValidateReport,
} from "../util/schemaGuards";
import type { NdjsonEvent, RunOptions, RunOutcome, TarnBackend } from "./TarnBackend";
import { readConfig } from "../config";

interface CollectedOutput {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
}

export class TarnProcessRunner implements TarnBackend {
  constructor(private readonly binaryPath: string) {}

  async run(options: RunOptions): Promise<RunOutcome> {
    if (options.streamNdjson) {
      return this.runNdjson(options);
    }
    const args = this.buildRunArgs(options, undefined);
    const collected = await this.spawnAndCollect(args, options.cwd, options.token);
    return this.toRunOutcome(collected, options.token, true);
  }

  async validate(
    files: string[],
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }> {
    const args = ["validate", ...files];
    const collected = await this.spawnAndCollect(args, cwd, token);
    return {
      exitCode: collected.exitCode,
      stdout: collected.stdout,
      stderr: collected.stderr,
    };
  }

  async validateStructured(
    files: string[],
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<ValidateReport | undefined> {
    const args = ["validate", "--format", "json", ...files];
    const collected = await this.spawnAndCollect(args, cwd, token);
    if (token.isCancellationRequested || collected.timedOut) {
      return undefined;
    }
    if (collected.stdout.length === 0) {
      return undefined;
    }
    try {
      return parseValidateReport(collected.stdout);
    } catch (err) {
      getOutputChannel().appendLine(
        `[tarn] failed to parse validate JSON (exit ${collected.exitCode}): ${String(err)}`,
      );
      return undefined;
    }
  }

  async envStructured(
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<EnvReport | undefined> {
    const args = ["env", "--json"];
    const collected = await this.spawnAndCollect(args, cwd, token);
    if (token.isCancellationRequested || collected.timedOut) {
      return undefined;
    }
    if (collected.stdout.length === 0) {
      return undefined;
    }
    try {
      return parseEnvReport(collected.stdout);
    } catch (err) {
      getOutputChannel().appendLine(
        `[tarn] failed to parse env JSON (exit ${collected.exitCode}): ${String(err)}`,
      );
      return undefined;
    }
  }

  async exportCurl(
    files: string[],
    cwd: string,
    mode: "all" | "failed",
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }> {
    const format = mode === "all" ? "curl-all" : "curl";
    const args = ["run", "--format", format, "--no-progress", ...files];
    const collected = await this.spawnAndCollect(args, cwd, token);
    return {
      exitCode: collected.exitCode,
      stdout: collected.stdout,
      stderr: collected.stderr,
    };
  }

  async initProject(
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }> {
    const collected = await this.spawnAndCollect(["init"], cwd, token);
    return {
      exitCode: collected.exitCode,
      stdout: collected.stdout,
      stderr: collected.stderr,
    };
  }

  private buildRunArgs(options: RunOptions, ndjsonReportPath: string | undefined): string[] {
    const args: string[] = ["run"];
    if (ndjsonReportPath) {
      args.push("--ndjson");
      args.push("--format", `json=${ndjsonReportPath}`);
      args.push("--json-mode", options.jsonMode ?? "verbose");
    } else {
      args.push("--format", "json");
      args.push("--json-mode", options.jsonMode ?? "verbose");
      args.push("--no-progress");
    }
    if (options.dryRun) {
      args.push("--dry-run");
    }
    if (options.parallel) {
      args.push("--parallel");
    }
    if (options.environment) {
      args.push("--env", options.environment);
    }
    if (options.tags && options.tags.length > 0) {
      args.push("--tag", options.tags.join(","));
    }
    if (options.selectors) {
      for (const selector of options.selectors) {
        args.push("--select", selector);
      }
    }
    if (options.vars) {
      for (const [key, value] of Object.entries(options.vars)) {
        args.push("--var", `${key}=${value}`);
      }
    }
    for (const file of options.files) {
      args.push(file);
    }
    return args;
  }

  private async runNdjson(options: RunOptions): Promise<RunOutcome> {
    const reportPath = path.join(
      os.tmpdir(),
      `tarn-vscode-${Date.now()}-${Math.random().toString(36).slice(2, 8)}.json`,
    );
    const args = this.buildRunArgs(options, reportPath);

    const config = readConfig();
    const output = getOutputChannel();
    output.appendLine(`[tarn] $ ${formatCommandForLog(this.binaryPath, args)}`);

    const child = spawn(this.binaryPath, args, {
      cwd: options.cwd,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });

    let timedOut = false;
    const watchdog = setTimeout(() => {
      timedOut = true;
      output.appendLine(
        `[tarn] watchdog fired after ${config.requestTimeoutMs}ms, killing process`,
      );
      child.kill("SIGKILL");
    }, config.requestTimeoutMs);
    watchdog.unref();

    const cancelSubscription = options.token.onCancellationRequested(() => {
      output.appendLine("[tarn] cancellation requested, sending SIGINT");
      child.kill("SIGINT");
      const killTimer = setTimeout(() => {
        if (!child.killed) {
          child.kill("SIGKILL");
        }
      }, 2000);
      killTimer.unref();
    });

    const stderrChunks: Buffer[] = [];
    child.stderr?.on("data", (chunk: Buffer) => {
      stderrChunks.push(chunk);
    });

    const rl = readline.createInterface({
      input: child.stdout!,
      crlfDelay: Infinity,
    });

    const stdoutLines: string[] = [];
    rl.on("line", (line: string) => {
      if (line.length === 0) {
        return;
      }
      stdoutLines.push(line);
      if (options.onEvent) {
        try {
          const event = JSON.parse(line) as NdjsonEvent;
          options.onEvent(event);
        } catch (err) {
          output.appendLine(`[tarn] failed to parse NDJSON line: ${String(err)}`);
        }
      }
    });

    const exitCode = await new Promise<number | null>((resolve) => {
      let settled = false;
      const finalize = (code: number | null) => {
        if (settled) return;
        settled = true;
        clearTimeout(watchdog);
        cancelSubscription.dispose();
        rl.close();
        resolve(code);
      };
      child.on("error", (err) => {
        stderrChunks.push(Buffer.from(String(err)));
        finalize(null);
      });
      child.on("close", (code) => finalize(code));
    });

    const stderr = Buffer.concat(stderrChunks).toString("utf8");
    if (stderr.length > 0) {
      output.appendLine(`[tarn] stderr: ${stderr.trim()}`);
    }

    let report: RunOutcome["report"] = undefined;
    try {
      const raw = await fs.promises.readFile(reportPath, "utf8");
      report = parseReport(raw);
    } catch (err) {
      output.appendLine(`[tarn] failed to read/parse NDJSON final report: ${String(err)}`);
    } finally {
      fs.promises.unlink(reportPath).catch(() => {});
    }

    return {
      report,
      exitCode,
      stdout: stdoutLines.join("\n"),
      stderr,
      cancelled: options.token.isCancellationRequested || timedOut,
    };
  }

  private spawnAndCollect(
    args: string[],
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<CollectedOutput> {
    const config = readConfig();
    const output = getOutputChannel();
    output.appendLine(`[tarn] $ ${formatCommandForLog(this.binaryPath, args)}`);

    return new Promise<CollectedOutput>((resolve) => {
      const child = spawn(this.binaryPath, args, {
        cwd,
        stdio: ["ignore", "pipe", "pipe"],
        windowsHide: true,
      });

      const stdoutChunks: Buffer[] = [];
      const stderrChunks: Buffer[] = [];
      let settled = false;
      let timedOut = false;

      const watchdog = setTimeout(() => {
        timedOut = true;
        output.appendLine(
          `[tarn] watchdog fired after ${config.requestTimeoutMs}ms, killing process`,
        );
        child.kill("SIGKILL");
      }, config.requestTimeoutMs);
      watchdog.unref();

      const cancelSubscription = token.onCancellationRequested(() => {
        output.appendLine("[tarn] cancellation requested, sending SIGINT");
        child.kill("SIGINT");
        const killTimer = setTimeout(() => {
          if (!child.killed) {
            child.kill("SIGKILL");
          }
        }, 2000);
        killTimer.unref();
      });

      child.stdout?.on("data", (chunk: Buffer) => {
        stdoutChunks.push(chunk);
      });
      child.stderr?.on("data", (chunk: Buffer) => {
        stderrChunks.push(chunk);
      });

      const finalize = (exitCode: number | null) => {
        if (settled) {
          return;
        }
        settled = true;
        clearTimeout(watchdog);
        cancelSubscription.dispose();
        const stdout = Buffer.concat(stdoutChunks).toString("utf8");
        const stderr = Buffer.concat(stderrChunks).toString("utf8");
        if (stderr.length > 0) {
          output.appendLine(`[tarn] stderr: ${stderr.trim()}`);
        }
        resolve({ exitCode, stdout, stderr, timedOut });
      };

      child.on("error", (err) => {
        stderrChunks.push(Buffer.from(String(err)));
        finalize(null);
      });

      child.on("close", (code) => {
        finalize(code);
      });
    });
  }

  private toRunOutcome(
    collected: CollectedOutput,
    token: vscode.CancellationToken,
    parseJson: boolean,
  ): RunOutcome {
    const cancelled = token.isCancellationRequested || collected.timedOut;
    let report: RunOutcome["report"] = undefined;
    if (parseJson && collected.stdout.length > 0) {
      try {
        report = parseReport(collected.stdout);
      } catch (err) {
        getOutputChannel().appendLine(`[tarn] failed to parse JSON report: ${String(err)}`);
      }
    }
    return {
      report,
      exitCode: collected.exitCode,
      stdout: collected.stdout,
      stderr: collected.stderr,
      cancelled,
    };
  }
}
