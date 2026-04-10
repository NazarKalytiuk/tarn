import * as assert from "assert";
import * as vscode from "vscode";

const EXTENSION_ID = "nazarkalytiuk.tarn-vscode";

interface BenchRunContextShape {
  result: {
    step_name: string;
    method: string;
    url: string;
    concurrency: number;
    total_requests: number;
    successful: number;
    failed: number;
    error_rate: number;
    total_duration_ms: number;
    throughput_rps: number;
    latency: {
      min_ms: number;
      max_ms: number;
      mean_ms: number;
      median_ms: number;
      p95_ms: number;
      p99_ms: number;
      stdev_ms: number;
    };
    status_codes?: Record<string, number>;
    errors?: string[];
    gates?: Array<{ name: string; passed: boolean }>;
    passed_gates?: boolean;
  };
  file: string;
  testName?: string;
}

interface TarnExtensionApiShape {
  readonly commands: readonly string[];
  readonly testing: {
    readonly showBenchResult: (context: BenchRunContextShape) => void;
    readonly lastBenchContext: () => BenchRunContextShape | undefined;
  };
}

async function getApi(): Promise<TarnExtensionApiShape> {
  const ext = vscode.extensions.getExtension<TarnExtensionApiShape>(EXTENSION_ID);
  assert.ok(ext, `extension ${EXTENSION_ID} not found`);
  const api = await ext!.activate();
  assert.ok(api, "extension activated but returned no API");
  return api;
}

const sampleContext: BenchRunContextShape = {
  result: {
    step_name: "hit httpbin",
    method: "GET",
    url: "https://httpbin.org/status/200",
    concurrency: 4,
    total_requests: 100,
    successful: 98,
    failed: 2,
    error_rate: 0.02,
    total_duration_ms: 5123,
    throughput_rps: 19.5,
    latency: {
      min_ms: 80,
      max_ms: 512,
      mean_ms: 210.4,
      median_ms: 188,
      p95_ms: 412,
      p99_ms: 498,
      stdev_ms: 65.2,
    },
    status_codes: { "200": 98, "500": 2 },
    errors: ["timeout on iteration 42", "connection reset on 73"],
    gates: [],
    passed_gates: false,
  },
  file: "tests/health.tarn.yaml",
  testName: "service_is_up",
};

describe("BenchRunnerPanel (tarn.benchStep)", () => {
  let api: TarnExtensionApiShape;

  before(async function () {
    this.timeout(60000);
    api = await getApi();
  });

  it("registers the tarn.benchStep command", async () => {
    const commands = await vscode.commands.getCommands(true);
    assert.ok(
      commands.includes("tarn.benchStep"),
      "tarn.benchStep should be registered",
    );
  });

  it("showBenchResult opens the panel and stores the context", () => {
    api.testing.showBenchResult(sampleContext);
    const stored = api.testing.lastBenchContext();
    assert.ok(stored, "expected a stored bench context after show");
    assert.strictEqual(stored!.result.step_name, "hit httpbin");
    assert.strictEqual(stored!.file, "tests/health.tarn.yaml");
    assert.strictEqual(stored!.testName, "service_is_up");
  });

  it("repeat show replaces the previous context", () => {
    const second: BenchRunContextShape = {
      ...sampleContext,
      result: { ...sampleContext.result, step_name: "second run" },
      testName: undefined,
    };
    api.testing.showBenchResult(second);
    const stored = api.testing.lastBenchContext();
    assert.strictEqual(stored!.result.step_name, "second run");
    assert.strictEqual(stored!.testName, undefined);
  });
});
