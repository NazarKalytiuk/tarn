import { spawn, type ChildProcessWithoutNullStreams } from "child_process";
import * as readline from "readline";
import * as vscode from "vscode";
import { getOutputChannel } from "../outputChannel";
import { readConfig } from "../config";
import {
  parseReport,
  type EnvReport,
  type ValidateReport,
} from "../util/schemaGuards";
import type {
  BenchOptions,
  BenchOutcome,
  HtmlReportOptions,
  HtmlReportOutcome,
  ListFileOutcome,
  RunOptions,
  RunOutcome,
  TarnBackend,
} from "./TarnBackend";

/**
 * JSON-RPC 2.0 request wire shape sent to `tarn-mcp`.
 */
interface JsonRpcRequest {
  readonly jsonrpc: "2.0";
  readonly id: number;
  readonly method: string;
  readonly params?: unknown;
}

/**
 * JSON-RPC 2.0 response wire shape received from `tarn-mcp`.
 */
interface JsonRpcResponse {
  readonly jsonrpc?: string;
  readonly id: number | string | null;
  readonly result?: unknown;
  readonly error?: { code: number; message: string; data?: unknown };
}

/**
 * MCP tool call result envelope. `tarn-mcp` wraps every tool result in
 * `{ content: [{ type: "text", text: <stringified-json> }], isError? }`
 * per the MCP spec. We unwrap the JSON text in {@link callTool}.
 */
interface McpToolResult {
  readonly content?: ReadonlyArray<{ type: string; text?: string }>;
  readonly isError?: boolean;
}

/**
 * Abstract stdio transport. Split out from {@link TarnMcpClient} so
 * unit tests can inject a scripted transport and pin the request /
 * response mapping without spawning a real child process.
 */
export interface McpTransport {
  send(line: string): void;
  onLine(handler: (line: string) => void): void;
  onClose(handler: (reason: string) => void): void;
  dispose(): void;
}

/**
 * Minimal NDJSON JSON-RPC 2.0 client for `tarn-mcp`.
 *
 * `vscode-jsonrpc` is listed as a transitive dependency (via
 * `vscode-languageclient`) but its default `MessageConnection` frames
 * messages with LSP-style `Content-Length:` headers. `tarn-mcp` — see
 * `tarn-mcp/src/main.rs` — reads one JSON object per newline and
 * writes one JSON object per newline, i.e. plain NDJSON. Wrapping the
 * upstream `MessageConnection` would either require writing a custom
 * `MessageReader`/`MessageWriter` pair (which is this class in all
 * but name) or teaching the server LSP framing (which would break
 * every other MCP client). A focused client here is the right fix.
 */
class JsonRpcClient {
  private nextId = 1;
  private readonly pending = new Map<
    number,
    { resolve(value: unknown): void; reject(err: Error): void }
  >();
  private closed = false;
  private closeReason: string | undefined;
  private buffer = "";

  constructor(private readonly transport: McpTransport) {
    transport.onLine((line) => this.handleLine(line));
    transport.onClose((reason) => this.handleClose(reason));
  }

  /**
   * Issue a JSON-RPC request and resolve with the parsed `result`.
   * Rejects if the server returns an error, if the transport closes
   * before a reply lands, or if the caller aborts via {@link signal}.
   */
  request<T>(
    method: string,
    params: unknown,
    signal?: AbortSignal,
  ): Promise<T> {
    if (this.closed) {
      return Promise.reject(
        new Error(
          `JSON-RPC transport closed: ${this.closeReason ?? "unknown reason"}`,
        ),
      );
    }
    const id = this.nextId++;
    const payload: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };
    return new Promise<T>((resolve, reject) => {
      const typedResolve = (value: unknown): void => {
        resolve(value as T);
      };
      this.pending.set(id, { resolve: typedResolve, reject });
      if (signal) {
        const onAbort = (): void => {
          if (this.pending.delete(id)) {
            reject(
              signal.reason instanceof Error
                ? signal.reason
                : new Error(String(signal.reason ?? "aborted")),
            );
          }
        };
        if (signal.aborted) {
          onAbort();
          return;
        }
        signal.addEventListener("abort", onAbort, { once: true });
      }
      try {
        this.transport.send(JSON.stringify(payload) + "\n");
      } catch (err) {
        this.pending.delete(id);
        reject(err instanceof Error ? err : new Error(String(err)));
      }
    });
  }

  /**
   * Issue a JSON-RPC notification (no `id`, no response expected). Used
   * for the MCP `notifications/initialized` handshake.
   */
  notify(method: string, params?: unknown): void {
    if (this.closed) {
      return;
    }
    const payload = {
      jsonrpc: "2.0",
      method,
      params,
    };
    this.transport.send(JSON.stringify(payload) + "\n");
  }

  dispose(): void {
    if (this.closed) {
      return;
    }
    this.handleClose("disposed");
    this.transport.dispose();
  }

  private handleLine(line: string): void {
    // `readline` gives us whole lines, but a defensive split on the
    // accumulated buffer keeps us robust against a future transport
    // that chunks differently (e.g., raw `data` handlers).
    this.buffer += line;
    const trimmed = this.buffer.trim();
    if (trimmed.length === 0) {
      this.buffer = "";
      return;
    }
    this.buffer = "";
    let parsed: JsonRpcResponse;
    try {
      parsed = JSON.parse(trimmed) as JsonRpcResponse;
    } catch (err) {
      // l10n-ignore: debug log for engineers.
      getOutputChannel().appendLine(
        `[tarn-mcp] failed to parse JSON-RPC frame: ${String(err)}`,
      );
      return;
    }
    if (parsed.id === null || parsed.id === undefined) {
      // Notifications from the server side — `tarn-mcp` currently
      // never sends any, but the spec allows them. Log and drop.
      return;
    }
    const id =
      typeof parsed.id === "number"
        ? parsed.id
        : typeof parsed.id === "string"
          ? Number(parsed.id)
          : Number.NaN;
    if (!Number.isFinite(id)) {
      return;
    }
    const entry = this.pending.get(id);
    if (!entry) {
      return;
    }
    this.pending.delete(id);
    if (parsed.error) {
      entry.reject(
        new Error(
          `JSON-RPC error ${parsed.error.code}: ${parsed.error.message}`,
        ),
      );
      return;
    }
    entry.resolve(parsed.result);
  }

  private handleClose(reason: string): void {
    this.closed = true;
    this.closeReason = reason;
    const pending = Array.from(this.pending.values());
    this.pending.clear();
    for (const entry of pending) {
      entry.reject(new Error(`JSON-RPC transport closed: ${reason}`));
    }
  }
}

/**
 * `child_process` transport backing the production MCP client. Tests
 * use a scripted {@link McpTransport} implementation instead.
 */
class ChildProcessTransport implements McpTransport {
  private lineHandler: ((line: string) => void) | undefined;
  private closeHandler: ((reason: string) => void) | undefined;
  private closed = false;
  private readonly reader: readline.Interface;

  constructor(private readonly child: ChildProcessWithoutNullStreams) {
    this.reader = readline.createInterface({
      input: child.stdout,
      crlfDelay: Infinity,
    });
    this.reader.on("line", (line) => {
      this.lineHandler?.(line);
    });
    child.on("close", (code) => {
      this.closed = true;
      this.closeHandler?.(`process exited with code ${code}`);
    });
    child.on("error", (err) => {
      this.closed = true;
      this.closeHandler?.(err.message);
    });
    child.stderr.on("data", (chunk: Buffer) => {
      // l10n-ignore: debug log for engineers.
      getOutputChannel().appendLine(`[tarn-mcp] stderr: ${chunk.toString("utf8").trim()}`);
    });
  }

  send(line: string): void {
    if (this.closed) {
      throw new Error("tarn-mcp transport already closed");
    }
    if (!this.child.stdin.writable) {
      throw new Error("tarn-mcp stdin is not writable");
    }
    this.child.stdin.write(line);
  }

  onLine(handler: (line: string) => void): void {
    this.lineHandler = handler;
  }

  onClose(handler: (reason: string) => void): void {
    this.closeHandler = handler;
  }

  dispose(): void {
    if (this.closed) {
      return;
    }
    this.closed = true;
    try {
      this.child.stdin.end();
    } catch {
      /* ignore: stdin may already be closed */
    }
    this.reader.close();
    try {
      this.child.kill("SIGTERM");
    } catch {
      /* ignore: child may already be dead */
    }
    const killTimer = setTimeout(() => {
      if (!this.child.killed) {
        try {
          this.child.kill("SIGKILL");
        } catch {
          /* ignore */
        }
      }
    }, 2000);
    killTimer.unref();
  }
}

/**
 * Options for constructing a {@link TarnMcpClient}. A caller can pass a
 * {@link McpTransport} directly (tests), or a child process that the
 * client wraps in a {@link ChildProcessTransport} (production).
 */
export type TarnMcpClientSource =
  | { readonly transport: McpTransport }
  | { readonly child: ChildProcessWithoutNullStreams };

/**
 * MCP backend for the Tarn VS Code extension.
 *
 * Spawns a long-lived `tarn-mcp` process per workspace and dispatches
 * `tarn_run`, `tarn_list`, `tarn_validate`, and `tarn_fix_plan` as
 * JSON-RPC 2.0 `tools/call` requests. Every request includes the
 * workspace `cwd` so the server can resolve relative paths and
 * project roots correctly (NAZ-248 on the server side).
 *
 * Operations the MCP surface does not expose (`env`, `fmt`, `bench`,
 * `init`, `exportCurl`, `importHurl`, `html-report`, scoped `list
 * --file`) are delegated to the backing {@link TarnBackend} — in
 * production, the existing `TarnProcessRunner`. This lets the user
 * opt into MCP for the hot path (`run` / `list` / `validate`) without
 * losing any feature that only the CLI ships.
 *
 * **NDJSON limitation.** `tarn-mcp` answers `tools/call` with a single
 * JSON reply; it does not stream intermediate events. When the caller
 * requests NDJSON (`streamNdjson: true`), the client degrades
 * gracefully: it runs the request as a normal one-shot `tarn_run` and
 * synthesizes `file_started` / `step_finished` / `test_finished` /
 * `file_finished` / `done` events from the final report after it
 * arrives. UI code therefore stays unchanged, but there is no live
 * progress — all events fire at the end of the run.
 */
export class TarnMcpClient implements TarnBackend {
  private readonly rpc: JsonRpcClient;
  private initialized: Promise<void> | undefined;
  private disposed = false;

  constructor(
    private readonly fallback: TarnBackend,
    source: TarnMcpClientSource,
  ) {
    const transport =
      "transport" in source
        ? source.transport
        : new ChildProcessTransport(source.child);
    this.rpc = new JsonRpcClient(transport);
  }

  /**
   * Perform the MCP `initialize` / `notifications/initialized` handshake
   * required by the protocol. Cached so subsequent commands re-use the
   * same warm session. Returns `true` on success and `false` if the
   * handshake fails — the caller can then fall back to the CLI.
   */
  async isReady(signal?: AbortSignal): Promise<boolean> {
    try {
      await this.ensureInitialized(signal);
      return true;
    } catch (err) {
      // l10n-ignore: debug log for engineers.
      getOutputChannel().appendLine(
        `[tarn-mcp] initialize failed: ${err instanceof Error ? err.message : String(err)}`,
      );
      return false;
    }
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.rpc.dispose();
  }

  async run(options: RunOptions): Promise<RunOutcome> {
    // MCP's `tarn_run` tool schema only exposes `path`, `env`, `vars`,
    // `tag` — it cannot receive `--dry-run`, `--select`, or
    // `--cookie-jar-per-test`. Whenever the caller asks for a feature
    // MCP cannot deliver we fall back to the CLI so the user never
    // silently loses a capability.
    //
    // `parallel` is a no-op for single-file runs (it only matters when
    // `tarn run` is iterating multiple files), so we do NOT fall back
    // on `parallel: true` here — every multi-file run already routes to
    // the CLI via the `files.length !== 1` guard below, and that is the
    // branch where `--parallel` would actually affect behavior.
    if (
      options.files.length !== 1 ||
      options.dryRun ||
      (options.selectors !== undefined && options.selectors.length > 0)
    ) {
      return this.fallback.run(options);
    }
    const params = this.toolParams(options.cwd, {
      path: options.files[0],
      env: options.environment ?? undefined,
      vars: options.vars ?? {},
      tag:
        options.tags && options.tags.length > 0
          ? options.tags.join(",")
          : undefined,
    });
    try {
      const payload = await this.callToolWithToken<unknown>("tarn_run", params, options.token);
      const jsonText = JSON.stringify(payload);
      const report = parseReport(jsonText);
      if (options.streamNdjson && options.onEvent) {
        synthesizeNdjsonEvents(report, options.onEvent);
      }
      const exitCode =
        report.summary.status === "PASSED" ? 0 : 1;
      return {
        report,
        exitCode,
        stdout: jsonText,
        stderr: "",
        cancelled: options.token.isCancellationRequested,
      };
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      return {
        report: undefined,
        exitCode: null,
        stdout: "",
        stderr: message,
        cancelled: options.token.isCancellationRequested,
      };
    }
  }

  async validate(
    files: string[],
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }> {
    // `tarn-mcp` exposes `tarn_validate` with a single `path` argument.
    // For a multi-file validate, fall back to the CLI which accepts a
    // variadic file list in one invocation.
    if (files.length !== 1) {
      return this.fallback.validate(files, cwd, token);
    }
    try {
      const payload = await this.callToolWithToken<{ valid?: boolean }>(
        "tarn_validate",
        this.toolParams(cwd, { path: files[0] }),
        token,
      );
      const text = JSON.stringify(payload);
      const exitCode = payload.valid === false ? 2 : 0;
      return { exitCode, stdout: text, stderr: "" };
    } catch (err) {
      return {
        exitCode: null,
        stdout: "",
        stderr: err instanceof Error ? err.message : String(err),
      };
    }
  }

  async validateStructured(
    files: string[],
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<ValidateReport | undefined> {
    if (files.length !== 1) {
      return this.fallback.validateStructured(files, cwd, token);
    }
    try {
      const payload = await this.callToolWithToken<unknown>(
        "tarn_validate",
        this.toolParams(cwd, { path: files[0] }),
        token,
      );
      return mapMcpValidateToReport(payload);
    } catch (err) {
      // l10n-ignore: debug log for engineers.
      getOutputChannel().appendLine(
        `[tarn-mcp] validateStructured failed: ${err instanceof Error ? err.message : String(err)}`,
      );
      return undefined;
    }
  }

  /**
   * Scoped list is a CLI-only feature — `tarn-mcp`'s `tarn_list` tool
   * returns a different envelope (no `setup` / `steps` / `teardown`
   * top-level fields). Delegate to the CLI so scoped discovery stays
   * functional.
   */
  async listFile(
    absolutePath: string,
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<ListFileOutcome> {
    return this.fallback.listFile(absolutePath, cwd, token);
  }

  // The following methods have no MCP equivalent — delegate to the
  // backing CLI runner so the user loses no capability when they
  // choose `tarn.backend: mcp`.

  async envStructured(
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<EnvReport | undefined> {
    return this.fallback.envStructured(cwd, token);
  }

  async runBench(options: BenchOptions): Promise<BenchOutcome> {
    return this.fallback.runBench(options);
  }

  async runHtmlReport(options: HtmlReportOptions): Promise<HtmlReportOutcome> {
    return this.fallback.runHtmlReport(options);
  }

  async exportCurl(
    files: string[],
    cwd: string,
    mode: "all" | "failed",
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }> {
    return this.fallback.exportCurl(files, cwd, mode, token);
  }

  async initProject(
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }> {
    return this.fallback.initProject(cwd, token);
  }

  async importHurl(
    source: string,
    dest: string,
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<{ exitCode: number | null; stdout: string; stderr: string }> {
    return this.fallback.importHurl(source, dest, cwd, token);
  }

  async formatDocument(
    content: string,
    cwd: string,
    token: vscode.CancellationToken,
  ): Promise<{ formatted: string; error?: string }> {
    return this.fallback.formatDocument(content, cwd, token);
  }

  /**
   * Low-level `tools/call` driver used by unit tests. Returns the raw
   * parsed JSON payload so tests can assert the unwrapping of MCP's
   * `content[{ type, text }]` envelope.
   */
  async callTool<T>(
    name: string,
    params: unknown,
    signal?: AbortSignal,
  ): Promise<T> {
    await this.ensureInitialized(signal);
    const watchdog = spawnWatchdog(readConfig().requestTimeoutMs, signal);
    try {
      const raw = await this.rpc.request<McpToolResult>(
        "tools/call",
        { name, arguments: params },
        watchdog.signal,
      );
      if (raw && raw.isError) {
        const text = raw.content?.[0]?.text ?? "<no error message>";
        throw new Error(text);
      }
      const textBlock = raw?.content?.find((c) => c && c.type === "text");
      if (!textBlock || textBlock.text === undefined) {
        throw new Error("MCP tool returned no text content");
      }
      try {
        return JSON.parse(textBlock.text) as T;
      } catch (err) {
        throw new Error(
          `MCP tool ${name} returned non-JSON text: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    } finally {
      watchdog.dispose();
    }
  }

  /**
   * Build the `arguments` object for a `tools/call` request. Always
   * threads `cwd` through so the server can resolve relative paths
   * against the user's workspace root (NAZ-248). Undefined fields are
   * stripped so the server sees a clean payload.
   */
  private toolParams(
    cwd: string,
    extra: Record<string, unknown>,
  ): Record<string, unknown> {
    const out: Record<string, unknown> = { cwd };
    for (const [key, value] of Object.entries(extra)) {
      if (value !== undefined) {
        out[key] = value;
      }
    }
    return out;
  }

  private async callToolWithToken<T>(
    name: string,
    params: unknown,
    token: vscode.CancellationToken,
  ): Promise<T> {
    const controller = new AbortController();
    const sub = token.onCancellationRequested(() => controller.abort());
    try {
      return await this.callTool<T>(name, params, controller.signal);
    } finally {
      sub.dispose();
    }
  }

  private ensureInitialized(signal?: AbortSignal): Promise<void> {
    if (!this.initialized) {
      this.initialized = (async (): Promise<void> => {
        await this.rpc.request<unknown>(
          "initialize",
          {
            protocolVersion: "2024-11-05",
            capabilities: {},
            clientInfo: { name: "tarn-vscode", version: "mcp-client" },
          },
          signal,
        );
        this.rpc.notify("notifications/initialized");
      })();
    }
    return this.initialized;
  }
}

/**
 * Construct a production {@link TarnMcpClient} by spawning the
 * configured `tarn-mcp` binary. The returned client is not yet
 * initialized; the caller should `await client.isReady()` before
 * issuing requests so initialization failures surface as a clean
 * fallback rather than a stack trace.
 */
export function spawnMcpClient(
  fallback: TarnBackend,
  binaryPath: string,
  cwd: string,
): TarnMcpClient {
  const output = getOutputChannel();
  // l10n-ignore: debug log for engineers.
  output.appendLine(`[tarn-mcp] spawning ${binaryPath} (cwd=${cwd})`);
  const child = spawn(binaryPath, [], {
    cwd,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  return new TarnMcpClient(fallback, { child });
}

/**
 * Map the MCP `tarn_validate` payload shape to the CLI's
 * {@link ValidateReport} shape consumed by the rest of the extension.
 *
 * MCP tool output: `{ valid: bool, files: [{ file, valid, error? }] }`
 *   where `error` is a single string message (or absent on success).
 * CLI JSON output: `{ files: [{ file, valid, errors: [{ message, line?, column? }] }] }`
 *
 * We translate the single `error` string into a `errors: [{ message }]`
 * array and drop location metadata (MCP does not emit line/column).
 * Exported for unit testing.
 */
export function mapMcpValidateToReport(payload: unknown): ValidateReport | undefined {
  if (payload === null || typeof payload !== "object") {
    return undefined;
  }
  const obj = payload as {
    files?: ReadonlyArray<{ file?: string; valid?: boolean; error?: string; errors?: unknown }>;
    error?: string;
  };
  if (!Array.isArray(obj.files)) {
    return undefined;
  }
  const files = obj.files.map((f) => {
    const file = typeof f.file === "string" ? f.file : "";
    const valid = f.valid === true;
    if (Array.isArray(f.errors)) {
      const errors = f.errors
        .filter((e): e is { message?: string; line?: number; column?: number } =>
          e !== null && typeof e === "object",
        )
        .map((e) => ({
          message: typeof e.message === "string" ? e.message : "",
          ...(typeof e.line === "number" && e.line >= 0 ? { line: e.line } : {}),
          ...(typeof e.column === "number" && e.column >= 0 ? { column: e.column } : {}),
        }));
      return { file, valid, errors };
    }
    const errors =
      !valid && typeof f.error === "string" ? [{ message: f.error }] : [];
    return { file, valid, errors };
  });
  const out: ValidateReport = { files };
  if (typeof obj.error === "string") {
    out.error = obj.error;
  }
  return out;
}

/**
 * Replay a final run report as the NDJSON event stream the CLI backend
 * produces. Used when a caller asks for `streamNdjson: true` against
 * the MCP backend, which does not support intermediate streaming.
 *
 * Exported for unit testing.
 */
export function synthesizeNdjsonEvents(
  report: ReturnType<typeof parseReport>,
  onEvent: NonNullable<RunOptions["onEvent"]>,
): void {
  for (const file of report.files) {
    onEvent({
      event: "file_started",
      file: file.file,
      file_name: file.name,
    });
    const emitStep = (
      phase: "setup" | "test" | "teardown",
      test: string,
      step: {
        name: string;
        status: "PASSED" | "FAILED";
        duration_ms: number;
        failure_category?: string;
        error_code?: string;
        assertions?: {
          failures?: Array<{
            assertion: string;
            expected?: string;
            actual?: string;
            message?: string;
            diff?: string | null | undefined;
          }>;
        };
      },
      step_index: number,
    ): void => {
      onEvent({
        event: "step_finished",
        file: file.file,
        phase,
        test,
        step: step.name,
        step_index,
        status: step.status,
        duration_ms: step.duration_ms,
        failure_category: step.failure_category,
        error_code: step.error_code,
        assertion_failures: step.assertions?.failures?.map((f) => ({
          assertion: f.assertion,
          expected: f.expected,
          actual: f.actual,
          message: f.message,
          diff: typeof f.diff === "string" ? f.diff : undefined,
        })),
      });
    };
    (file.setup ?? []).forEach((step, idx) => emitStep("setup", "<setup>", step, idx));
    for (const test of file.tests) {
      test.steps.forEach((step, idx) => emitStep("test", test.name, step, idx));
      onEvent({
        event: "test_finished",
        file: file.file,
        test: test.name,
        status: test.status,
        duration_ms: test.duration_ms,
        steps: {
          total: test.steps.length,
          passed: test.steps.filter((s) => s.status === "PASSED").length,
          failed: test.steps.filter((s) => s.status === "FAILED").length,
        },
      });
    }
    (file.teardown ?? []).forEach((step, idx) => emitStep("teardown", "<teardown>", step, idx));
    onEvent({
      event: "file_finished",
      file: file.file,
      file_name: file.name,
      status: file.status,
      duration_ms: file.duration_ms,
      summary: file.summary,
    });
  }
  onEvent({
    event: "done",
    duration_ms: report.duration_ms,
    summary: {
      files: report.summary.files,
      tests: report.summary.tests,
      steps: report.summary.steps,
      status: report.summary.status,
    },
  });
}

/**
 * Merge an optional caller-supplied abort signal with a timer-based
 * watchdog so every MCP request has a hard upper bound. Returns the
 * merged signal plus a `dispose` hook the caller invokes on completion
 * so the watchdog timer is cleared even when the request succeeds.
 */
function spawnWatchdog(
  timeoutMs: number,
  parent?: AbortSignal,
): { signal: AbortSignal; dispose(): void } {
  const controller = new AbortController();
  const timer = setTimeout(() => {
    controller.abort(new Error(`tarn-mcp request timed out after ${timeoutMs}ms`));
  }, timeoutMs);
  timer.unref();
  let onParentAbort: (() => void) | undefined;
  if (parent) {
    if (parent.aborted) {
      clearTimeout(timer);
      controller.abort(parent.reason);
    } else {
      onParentAbort = (): void => {
        controller.abort(parent.reason);
      };
      parent.addEventListener("abort", onParentAbort, { once: true });
    }
  }
  return {
    signal: controller.signal,
    dispose: (): void => {
      clearTimeout(timer);
      if (parent && onParentAbort) {
        parent.removeEventListener("abort", onParentAbort);
      }
    },
  };
}
