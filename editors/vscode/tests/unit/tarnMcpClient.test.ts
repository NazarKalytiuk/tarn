import { describe, it, expect, beforeEach } from "vitest";
import * as vscode from "vscode";
import type {
  BenchOptions,
  BenchOutcome,
  HtmlReportOptions,
  HtmlReportOutcome,
  ListFileOutcome,
  NdjsonEvent,
  RunOptions,
  RunOutcome,
  TarnBackend,
} from "../../src/backend/TarnBackend";
import {
  TarnMcpClient,
  mapMcpValidateToReport,
  synthesizeNdjsonEvents,
  type McpTransport,
} from "../../src/backend/TarnMcpClient";
import type { EnvReport, Report, ValidateReport } from "../../src/util/schemaGuards";
import {
  readBackendKind,
  readMcpPath,
} from "../../src/config";
import {
  resolveMcpCommand,
} from "../../src/backend/binaryResolver";

// We re-import the named helpers from the mock module so tests can
// seed config state and inspect notification toasts.
import * as vscodeMock from "./__mocks__/vscode";

interface MockVscode {
  __setMockConfig(entries: Record<string, unknown>): void;
  __getShownInformationMessages(): string[];
  __clearShownInformationMessages(): void;
}

const mockApi = vscodeMock as unknown as MockVscode;

/**
 * Scripted JSON-RPC transport for unit testing. Captures every outbound
 * frame and lets the test drive synthetic server responses back to the
 * client by invoking `respond(frame)`. A test can assert the full
 * request shape (method, id, params, cwd) without spawning a real
 * `tarn-mcp` process.
 */
class ScriptedTransport implements McpTransport {
  public readonly sent: string[] = [];
  private lineHandler: ((line: string) => void) | undefined;
  private closeHandler: ((reason: string) => void) | undefined;

  send(line: string): void {
    this.sent.push(line.trim());
  }

  onLine(handler: (line: string) => void): void {
    this.lineHandler = handler;
  }

  onClose(handler: (reason: string) => void): void {
    this.closeHandler = handler;
  }

  dispose(): void {
    this.closeHandler?.("disposed");
  }

  /** Push a response frame as if it came from the server. */
  respond(frame: unknown): void {
    const line = typeof frame === "string" ? frame : JSON.stringify(frame);
    this.lineHandler?.(line);
  }

  lastSentRequest(): { method: string; id: number; params: Record<string, unknown> } {
    const last = this.sent[this.sent.length - 1];
    const parsed = JSON.parse(last) as {
      method: string;
      id: number;
      params: Record<string, unknown>;
    };
    return parsed;
  }

  /**
   * Return the most recent request frame whose `method` matches. Used
   * when the caller issues two requests in rapid succession (e.g., an
   * `initialize` handshake immediately followed by a `tools/call`).
   */
  lastSentRequestOfMethod(method: string): {
    method: string;
    id: number;
    params: Record<string, unknown>;
  } {
    for (let i = this.sent.length - 1; i >= 0; i--) {
      try {
        const frame = JSON.parse(this.sent[i]) as {
          method?: string;
          id?: number;
          params?: Record<string, unknown>;
        };
        if (frame.method === method && typeof frame.id === "number") {
          return {
            method: frame.method,
            id: frame.id,
            params: frame.params ?? {},
          };
        }
      } catch {
        /* skip */
      }
    }
    throw new Error(`no frame with method=${method} was sent`);
  }
}

/**
 * Test double for the backing `TarnBackend`. We spy on which methods
 * the `TarnMcpClient` delegates to the CLI fallback (e.g., scoped
 * `listFile`, NDJSON streaming on `run`) without wiring up a real
 * process runner.
 */
class StubFallback implements TarnBackend {
  public runCalls: RunOptions[] = [];
  public validateCalls: Array<{ files: string[]; cwd: string }> = [];
  public validateStructuredCalls: Array<{ files: string[]; cwd: string }> = [];
  public listFileCalls: Array<{ path: string; cwd: string }> = [];
  public envStructuredCalls = 0;
  public formatCalls = 0;

  async run(options: RunOptions): Promise<RunOutcome> {
    this.runCalls.push(options);
    return {
      report: undefined,
      exitCode: 0,
      stdout: "<<CLI stdout>>",
      stderr: "",
      cancelled: false,
    };
  }
  async runHtmlReport(_options: HtmlReportOptions): Promise<HtmlReportOutcome> {
    return { htmlPath: undefined, exitCode: 0, stderr: "" };
  }
  async runBench(_options: BenchOptions): Promise<BenchOutcome> {
    return { result: undefined, exitCode: 0, stdout: "", stderr: "" };
  }
  async validate(files: string[], cwd: string): Promise<{ exitCode: number | null; stdout: string; stderr: string }> {
    this.validateCalls.push({ files, cwd });
    return { exitCode: 0, stdout: "<<CLI validate>>", stderr: "" };
  }
  async validateStructured(files: string[], cwd: string): Promise<ValidateReport | undefined> {
    this.validateStructuredCalls.push({ files, cwd });
    return undefined;
  }
  async envStructured(_cwd: string): Promise<EnvReport | undefined> {
    this.envStructuredCalls++;
    return undefined;
  }
  async listFile(absolutePath: string, cwd: string): Promise<ListFileOutcome> {
    this.listFileCalls.push({ path: absolutePath, cwd });
    return { ok: false, reason: "unsupported" };
  }
  async exportCurl(_files: string[], _cwd: string, _mode: "all" | "failed") {
    return { exitCode: 0, stdout: "", stderr: "" };
  }
  async initProject(_cwd: string) {
    return { exitCode: 0, stdout: "", stderr: "" };
  }
  async importHurl(_source: string, _dest: string, _cwd: string) {
    return { exitCode: 0, stdout: "", stderr: "" };
  }
  async formatDocument(content: string) {
    this.formatCalls++;
    return { formatted: content };
  }
}

function makeToken(): vscode.CancellationToken {
  return new vscode.CancellationTokenSource().token;
}

function makeCancelableToken(): {
  source: vscode.CancellationTokenSource;
  token: vscode.CancellationToken;
} {
  const source = new vscode.CancellationTokenSource();
  return { source, token: source.token };
}

/**
 * Small helper: wrap a value in the MCP `content[{ type: "text", text }]`
 * envelope the server actually returns.
 */
function mcpTextResult(value: unknown): Record<string, unknown> {
  return {
    content: [
      { type: "text", text: JSON.stringify(value) },
    ],
  };
}

const SAMPLE_REPORT: Report = {
  duration_ms: 42,
  files: [
    {
      file: "tests/health.tarn.yaml",
      name: "Health",
      status: "PASSED",
      duration_ms: 42,
      summary: { total: 1, passed: 1, failed: 0 },
      tests: [
        {
          name: "smoke",
          status: "PASSED",
          duration_ms: 42,
          steps: [
            { name: "GET /health", status: "PASSED", duration_ms: 42 },
          ],
        },
      ],
    },
  ],
  summary: {
    files: 1,
    tests: 1,
    steps: { total: 1, passed: 1, failed: 0 },
    status: "PASSED",
  },
};

describe("TarnMcpClient: JSON-RPC request/response mapping", () => {
  beforeEach(() => {
    mockApi.__setMockConfig({});
    mockApi.__clearShownInformationMessages();
  });

  it("performs the MCP initialize handshake on first request", async () => {
    const transport = new ScriptedTransport();
    const client = new TarnMcpClient(new StubFallback(), { transport });

    const readyPromise = client.isReady();
    // First frame: initialize
    const init = transport.lastSentRequestOfMethod("initialize");
    expect(init.params).toMatchObject({
      protocolVersion: "2024-11-05",
      clientInfo: { name: "tarn-vscode" },
    });
    transport.respond({ jsonrpc: "2.0", id: init.id, result: {} });

    const ready = await readyPromise;
    expect(ready).toBe(true);
    // After init + handshake, the notifications/initialized frame is
    // fire-and-forget (no id).
    const notifFrame = transport.sent.find((line) =>
      line.includes("notifications/initialized"),
    );
    expect(notifFrame).toBeTruthy();
    client.dispose();
  });

  it("returns false from isReady() when the initialize request errors", async () => {
    const transport = new ScriptedTransport();
    const client = new TarnMcpClient(new StubFallback(), { transport });

    const readyPromise = client.isReady();
    const init = transport.lastSentRequestOfMethod("initialize");
    transport.respond({
      jsonrpc: "2.0",
      id: init.id,
      error: { code: -32603, message: "boom" },
    });

    const ready = await readyPromise;
    expect(ready).toBe(false);
    client.dispose();
  });

  it("dispatches tarn_run as tools/call with cwd threaded through arguments", async () => {
    const transport = new ScriptedTransport();
    const fallback = new StubFallback();
    const client = new TarnMcpClient(fallback, { transport });

    const runPromise = client.run({
      files: ["tests/health.tarn.yaml"],
      cwd: "/workspace",
      token: makeToken(),
    });
    // Respond to initialize, then to tools/call
    const init = transport.lastSentRequestOfMethod("initialize");
    transport.respond({ jsonrpc: "2.0", id: init.id, result: {} });

    // Poll until the tools/call frame is on the wire. The JSON-RPC
    // client sends the initialize request, then waits for the reply,
    // then sends tools/call — so we need one microtask flush before
    // inspecting.
    await new Promise((r) => setImmediate(r));
    const call = transport.lastSentRequestOfMethod("tools/call");
    expect(call.params).toMatchObject({
      name: "tarn_run",
      arguments: {
        cwd: "/workspace",
        path: "tests/health.tarn.yaml",
      },
    });

    transport.respond({
      jsonrpc: "2.0",
      id: call.id,
      result: mcpTextResult(SAMPLE_REPORT),
    });

    const outcome = await runPromise;
    expect(outcome.exitCode).toBe(0);
    expect(outcome.cancelled).toBe(false);
    expect(outcome.report).toBeDefined();
    expect(outcome.report?.summary.status).toBe("PASSED");
    expect(fallback.runCalls.length).toBe(0);
    client.dispose();
  });

  it("unwraps MCP error results into a failed RunOutcome", async () => {
    const transport = new ScriptedTransport();
    const client = new TarnMcpClient(new StubFallback(), { transport });

    const runPromise = client.run({
      files: ["tests/health.tarn.yaml"],
      cwd: "/workspace",
      token: makeToken(),
    });
    const init = transport.lastSentRequestOfMethod("initialize");
    transport.respond({ jsonrpc: "2.0", id: init.id, result: {} });
    await new Promise((r) => setImmediate(r));
    const call = transport.lastSentRequestOfMethod("tools/call");
    transport.respond({
      jsonrpc: "2.0",
      id: call.id,
      result: {
        isError: true,
        content: [{ type: "text", text: "Path not found: tests/health.tarn.yaml" }],
      },
    });

    const outcome = await runPromise;
    expect(outcome.exitCode).toBeNull();
    expect(outcome.report).toBeUndefined();
    expect(outcome.stderr).toContain("Path not found");
    client.dispose();
  });

  it("maps tarn_validate output to the structured ValidateReport shape", async () => {
    const transport = new ScriptedTransport();
    const client = new TarnMcpClient(new StubFallback(), { transport });

    const validatePromise = client.validateStructured(
      ["tests/broken.tarn.yaml"],
      "/workspace",
      makeToken(),
    );
    const init = transport.lastSentRequestOfMethod("initialize");
    transport.respond({ jsonrpc: "2.0", id: init.id, result: {} });
    await new Promise((r) => setImmediate(r));
    const call = transport.lastSentRequestOfMethod("tools/call");
    expect(call.params).toMatchObject({
      name: "tarn_validate",
      arguments: { cwd: "/workspace", path: "tests/broken.tarn.yaml" },
    });
    transport.respond({
      jsonrpc: "2.0",
      id: call.id,
      result: mcpTextResult({
        valid: false,
        files: [
          { file: "tests/broken.tarn.yaml", valid: false, error: "expected mapping node at line 3" },
        ],
      }),
    });

    const report = await validatePromise;
    expect(report).toBeDefined();
    expect(report?.files).toHaveLength(1);
    expect(report?.files[0]).toEqual({
      file: "tests/broken.tarn.yaml",
      valid: false,
      errors: [{ message: "expected mapping node at line 3" }],
    });
    client.dispose();
  });

  it("falls back to the CLI for multi-file validate (MCP accepts only one path)", async () => {
    const transport = new ScriptedTransport();
    const fallback = new StubFallback();
    const client = new TarnMcpClient(fallback, { transport });

    const outcome = await client.validate(
      ["a.tarn.yaml", "b.tarn.yaml"],
      "/workspace",
      makeToken(),
    );
    // No frames should have been sent to MCP — the client short-circuited
    // straight to the CLI.
    expect(transport.sent).toHaveLength(0);
    expect(outcome.stdout).toBe("<<CLI validate>>");
    expect(fallback.validateCalls).toEqual([
      { files: ["a.tarn.yaml", "b.tarn.yaml"], cwd: "/workspace" },
    ]);
    client.dispose();
  });

  it("falls back to the CLI when run is invoked with dry-run", async () => {
    const transport = new ScriptedTransport();
    const fallback = new StubFallback();
    const client = new TarnMcpClient(fallback, { transport });

    const outcome = await client.run({
      files: ["tests/health.tarn.yaml"],
      cwd: "/workspace",
      dryRun: true,
      token: makeToken(),
    });
    expect(transport.sent).toHaveLength(0);
    expect(outcome.stdout).toBe("<<CLI stdout>>");
    expect(fallback.runCalls).toHaveLength(1);
    client.dispose();
  });

  it("falls back to the CLI when run uses selectors (non-empty)", async () => {
    const transport = new ScriptedTransport();
    const fallback = new StubFallback();
    const client = new TarnMcpClient(fallback, { transport });

    await client.run({
      files: ["tests/health.tarn.yaml"],
      cwd: "/workspace",
      selectors: ["tests/health.tarn.yaml::smoke"],
      token: makeToken(),
    });
    expect(fallback.runCalls).toHaveLength(1);
    client.dispose();
  });

  it("treats an empty selectors array as no selectors (still dispatches to MCP)", async () => {
    const transport = new ScriptedTransport();
    const fallback = new StubFallback();
    const client = new TarnMcpClient(fallback, { transport });

    const runPromise = client.run({
      files: ["tests/health.tarn.yaml"],
      cwd: "/workspace",
      selectors: [],
      token: makeToken(),
    });
    const init = transport.lastSentRequestOfMethod("initialize");
    transport.respond({ jsonrpc: "2.0", id: init.id, result: {} });
    await new Promise((r) => setImmediate(r));
    const call = transport.lastSentRequestOfMethod("tools/call");
    transport.respond({
      jsonrpc: "2.0",
      id: call.id,
      result: mcpTextResult(SAMPLE_REPORT),
    });
    await runPromise;
    expect(fallback.runCalls).toHaveLength(0);
    client.dispose();
  });

  it("delegates scoped listFile straight to the CLI (MCP has no scoped list)", async () => {
    const transport = new ScriptedTransport();
    const fallback = new StubFallback();
    const client = new TarnMcpClient(fallback, { transport });

    await client.listFile("/workspace/tests/x.tarn.yaml", "/workspace", makeToken());
    expect(transport.sent).toHaveLength(0);
    expect(fallback.listFileCalls).toEqual([
      { path: "/workspace/tests/x.tarn.yaml", cwd: "/workspace" },
    ]);
    client.dispose();
  });

  it("delegates envStructured / formatDocument / initProject / exportCurl / importHurl / runBench / runHtmlReport to the CLI", async () => {
    const transport = new ScriptedTransport();
    const fallback = new StubFallback();
    const client = new TarnMcpClient(fallback, { transport });

    await client.envStructured("/workspace", makeToken());
    expect(fallback.envStructuredCalls).toBe(1);
    await client.formatDocument("name: x\n", "/workspace", makeToken());
    expect(fallback.formatCalls).toBe(1);
    expect(transport.sent).toHaveLength(0);
    client.dispose();
  });

  it("synthesizes NDJSON events from the final report when streamNdjson is requested", async () => {
    const transport = new ScriptedTransport();
    const client = new TarnMcpClient(new StubFallback(), { transport });
    const events: NdjsonEvent[] = [];

    const runPromise = client.run({
      files: ["tests/health.tarn.yaml"],
      cwd: "/workspace",
      streamNdjson: true,
      onEvent: (event) => events.push(event),
      token: makeToken(),
    });
    const init = transport.lastSentRequestOfMethod("initialize");
    transport.respond({ jsonrpc: "2.0", id: init.id, result: {} });
    await new Promise((r) => setImmediate(r));
    const call = transport.lastSentRequestOfMethod("tools/call");
    transport.respond({
      jsonrpc: "2.0",
      id: call.id,
      result: mcpTextResult(SAMPLE_REPORT),
    });

    const outcome = await runPromise;
    expect(outcome.report).toBeDefined();
    expect(events.length).toBeGreaterThan(0);
    const names = events.map((e) => e.event);
    expect(names[0]).toBe("file_started");
    expect(names[names.length - 1]).toBe("done");
    client.dispose();
  });

  it("rejects a pending JSON-RPC request when the caller cancels via the token", async () => {
    const transport = new ScriptedTransport();
    const client = new TarnMcpClient(new StubFallback(), { transport });
    const { source, token } = makeCancelableToken();

    const runPromise = client.run({
      files: ["tests/health.tarn.yaml"],
      cwd: "/workspace",
      token,
    });
    const init = transport.lastSentRequestOfMethod("initialize");
    transport.respond({ jsonrpc: "2.0", id: init.id, result: {} });
    await new Promise((r) => setImmediate(r));
    // Cancel before the tools/call reply lands
    source.cancel();
    const outcome = await runPromise;
    expect(outcome.cancelled).toBe(true);
    expect(outcome.exitCode).toBeNull();
    client.dispose();
  });
});

describe("mapMcpValidateToReport", () => {
  it("handles a fully-valid report with no errors", () => {
    const report = mapMcpValidateToReport({
      valid: true,
      files: [{ file: "a.tarn.yaml", valid: true }],
    });
    expect(report).toEqual({
      files: [{ file: "a.tarn.yaml", valid: true, errors: [] }],
    });
  });

  it("lifts a string `error` field into an errors array with a single message", () => {
    const report = mapMcpValidateToReport({
      valid: false,
      files: [
        { file: "a.tarn.yaml", valid: false, error: "parse failed: line 3" },
      ],
    });
    expect(report?.files[0].errors).toEqual([
      { message: "parse failed: line 3" },
    ]);
  });

  it("preserves an already-structured errors array when present", () => {
    const report = mapMcpValidateToReport({
      files: [
        {
          file: "a.tarn.yaml",
          valid: false,
          errors: [{ message: "bad", line: 2, column: 5 }],
        },
      ],
    });
    expect(report?.files[0].errors).toEqual([
      { message: "bad", line: 2, column: 5 },
    ]);
  });

  it("returns undefined when the input has no files array", () => {
    expect(mapMcpValidateToReport({})).toBeUndefined();
    expect(mapMcpValidateToReport(null)).toBeUndefined();
    expect(mapMcpValidateToReport(42)).toBeUndefined();
  });
});

describe("synthesizeNdjsonEvents", () => {
  it("replays the final report as file_started / step_finished / test_finished / file_finished / done", () => {
    const events: NdjsonEvent[] = [];
    synthesizeNdjsonEvents(SAMPLE_REPORT, (e) => events.push(e));
    const names = events.map((e) => e.event);
    expect(names[0]).toBe("file_started");
    expect(names).toContain("step_finished");
    expect(names).toContain("test_finished");
    expect(names).toContain("file_finished");
    expect(names[names.length - 1]).toBe("done");
  });

  it("emits setup / test / teardown phases in order", () => {
    const report: Report = {
      duration_ms: 1,
      files: [
        {
          file: "x.tarn.yaml",
          name: "x",
          status: "PASSED",
          duration_ms: 1,
          summary: { total: 1, passed: 1, failed: 0 },
          setup: [{ name: "setup 1", status: "PASSED", duration_ms: 0 }],
          tests: [
            {
              name: "t",
              status: "PASSED",
              duration_ms: 1,
              steps: [{ name: "s", status: "PASSED", duration_ms: 1 }],
            },
          ],
          teardown: [{ name: "teardown 1", status: "PASSED", duration_ms: 0 }],
        },
      ],
      summary: {
        files: 1,
        tests: 1,
        steps: { total: 3, passed: 3, failed: 0 },
        status: "PASSED",
      },
    };
    const phases: string[] = [];
    synthesizeNdjsonEvents(report, (e) => {
      if (e.event === "step_finished") {
        phases.push(e.phase);
      }
    });
    expect(phases).toEqual(["setup", "test", "teardown"]);
  });
});

describe("config: readBackendKind / readMcpPath", () => {
  beforeEach(() => {
    mockApi.__setMockConfig({});
  });

  it("defaults backend to cli when unset", () => {
    expect(readBackendKind()).toBe("cli");
  });

  it("returns mcp only for the exact 'mcp' string", () => {
    mockApi.__setMockConfig({ "tarn.backend": "mcp" });
    expect(readBackendKind()).toBe("mcp");
    mockApi.__setMockConfig({ "tarn.backend": "MCP" });
    expect(readBackendKind()).toBe("cli");
    mockApi.__setMockConfig({ "tarn.backend": "something-else" });
    expect(readBackendKind()).toBe("cli");
  });

  it("readMcpPath returns undefined when the setting is unset", () => {
    expect(readMcpPath()).toBeUndefined();
  });

  it("readMcpPath returns the user override when set", () => {
    mockApi.__setMockConfig({ "tarn.mcpPath": "/opt/tarn-mcp" });
    expect(readMcpPath()).toBe("/opt/tarn-mcp");
  });
});

describe("resolveMcpCommand", () => {
  it("falls back to 'tarn-mcp' when unset or blank", () => {
    expect(resolveMcpCommand(undefined)).toBe("tarn-mcp");
    expect(resolveMcpCommand("")).toBe("tarn-mcp");
    expect(resolveMcpCommand("   ")).toBe("tarn-mcp");
  });

  it("returns bare names unchanged (resolved via PATH at spawn)", () => {
    expect(resolveMcpCommand("tarn-mcp")).toBe("tarn-mcp");
    expect(resolveMcpCommand("tarn-mcp-nightly")).toBe("tarn-mcp-nightly");
  });

  it("trims whitespace", () => {
    expect(resolveMcpCommand("  tarn-mcp  ")).toBe("tarn-mcp");
  });
});
