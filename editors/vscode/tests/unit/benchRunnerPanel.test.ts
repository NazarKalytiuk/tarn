import { describe, it, expect } from "vitest";
import {
  formatDuration,
  formatNumber,
  formatPercent,
  percentWidth,
  renderBar,
} from "../../src/views/BenchRunnerPanel";
import {
  benchResultSchema,
  parseBenchResult,
} from "../../src/util/schemaGuards";
import { buildSettingsKey } from "../../src/commands/bench";

const SAMPLE_BENCH_JSON = JSON.stringify({
  step_name: "hit httpbin",
  method: "GET",
  url: "https://httpbin.org/status/200",
  concurrency: 2,
  ramp_up_ms: null,
  total_requests: 5,
  successful: 5,
  failed: 0,
  error_rate: 0.0,
  total_duration_ms: 833,
  throughput_rps: 6.0,
  latency: {
    min_ms: 126,
    max_ms: 535,
    mean_ms: 298.8,
    median_ms: 177,
    p95_ms: 535,
    p99_ms: 535,
    stdev_ms: 190.89,
  },
  status_codes: { "200": 5 },
  errors: [],
  gates: [],
  passed_gates: true,
});

describe("benchResultSchema", () => {
  it("parses a real tarn bench JSON output", () => {
    const result = parseBenchResult(SAMPLE_BENCH_JSON);
    expect(result.step_name).toBe("hit httpbin");
    expect(result.successful).toBe(5);
    expect(result.latency.p95_ms).toBe(535);
    expect(result.status_codes?.["200"]).toBe(5);
  });

  it("accepts reports missing optional fields", () => {
    const minimal = {
      step_name: "s",
      method: "GET",
      url: "http://example.com",
      concurrency: 1,
      total_requests: 1,
      successful: 1,
      failed: 0,
      error_rate: 0,
      total_duration_ms: 10,
      throughput_rps: 100,
      latency: {
        min_ms: 10,
        max_ms: 10,
        mean_ms: 10,
        median_ms: 10,
        p95_ms: 10,
        p99_ms: 10,
        stdev_ms: 0,
      },
    };
    const parsed = benchResultSchema.parse(minimal);
    expect(parsed.step_name).toBe("s");
    expect(parsed.status_codes).toBeUndefined();
  });

  it("rejects reports missing required latency fields", () => {
    expect(() =>
      parseBenchResult(
        JSON.stringify({
          ...JSON.parse(SAMPLE_BENCH_JSON),
          latency: { min_ms: 10, max_ms: 20 },
        }),
      ),
    ).toThrow();
  });
});

describe("percentWidth", () => {
  it("scales values proportionally to max", () => {
    expect(percentWidth(50, 100)).toBe(50);
    expect(percentWidth(25, 100)).toBe(25);
    expect(percentWidth(100, 100)).toBe(100);
  });

  it("returns 0 for non-positive max", () => {
    expect(percentWidth(10, 0)).toBe(0);
    expect(percentWidth(10, -5)).toBe(0);
  });

  it("clamps to 0..100", () => {
    expect(percentWidth(-1, 100)).toBe(0);
    expect(percentWidth(200, 100)).toBe(100);
  });

  it("floors tiny non-zero values to 2 so the bar stays visible", () => {
    expect(percentWidth(0.5, 100)).toBe(2);
    expect(percentWidth(0, 100)).toBe(0);
  });
});

describe("formatters", () => {
  it("formatNumber picks precision by magnitude", () => {
    expect(formatNumber(1234)).toBe("1234");
    expect(formatNumber(12.3)).toBe("12.3");
    expect(formatNumber(1.23)).toBe("1.23");
    expect(formatNumber(NaN)).toBe("–");
  });

  it("formatPercent multiplies by 100", () => {
    expect(formatPercent(0)).toBe("0.00%");
    expect(formatPercent(0.01)).toBe("1.00%");
    expect(formatPercent(1)).toBe("100%");
  });

  it("formatDuration switches units at 1s", () => {
    expect(formatDuration(500)).toBe("500 ms");
    expect(formatDuration(2500)).toBe("2.50 s");
    expect(formatDuration(NaN)).toBe("–");
  });
});

describe("renderBar", () => {
  it("includes the label, the ms value, and a fill width", () => {
    const html = renderBar("p95", 200, 400);
    expect(html).toContain("p95");
    expect(html).toContain("200 ms");
    expect(html).toContain('style="width: 50%"');
  });

  it("escapes HTML in labels", () => {
    const html = renderBar("<script>", 10, 100);
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
  });
});

describe("buildSettingsKey", () => {
  it("namespaces the key by file", () => {
    expect(buildSettingsKey("tests/a.tarn.yaml")).toBe(
      "tarn.benchSettings:tests/a.tarn.yaml",
    );
    expect(buildSettingsKey("b.tarn.yaml")).not.toEqual(
      buildSettingsKey("a.tarn.yaml"),
    );
  });
});
