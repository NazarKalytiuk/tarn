import { spawn } from "child_process";
import * as vscode from "vscode";
import { getOutputChannel } from "../outputChannel";
import { formatCommandForLog } from "../util/shellEscape";
import { parseReport } from "../util/schemaGuards";
import type { RunOptions, RunOutcome, TarnBackend } from "./TarnBackend";
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
    const args = this.buildRunArgs(options);
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

  private buildRunArgs(options: RunOptions): string[] {
    const args: string[] = ["run"];
    args.push("--format", "json");
    args.push("--json-mode", options.jsonMode ?? "verbose");
    args.push("--no-progress");
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
