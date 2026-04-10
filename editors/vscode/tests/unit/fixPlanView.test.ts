import { describe, it, expect } from "vitest";
import {
  categoryOrder,
  deserializeRange,
  flattenReportToPlan,
  humanizeCategory,
} from "../../src/views/FixPlanView";
import type { Report } from "../../src/util/schemaGuards";
import type { ParsedFile } from "../../src/workspace/WorkspaceIndex";

/** Cheap ParsedFile stub — flattenReportToPlan only reads `uri` and `ranges.tests`. */
function stubParsedFile(filePath: string): ParsedFile {
  return {
    uri: {
      // The unit-test vscode mock returns a plain object from Uri.file
      fsPath: filePath,
      toString: () => `file://${filePath}`,
      path: filePath,
    } as unknown as ParsedFile["uri"],
    ranges: {
      fileName: "stub",
      fileNameRange: undefined,
      setup: [],
      teardown: [],
      tests: [
        {
          name: "login",
          description: null,
          nameRange: {} as ParsedFile["ranges"]["tests"][number]["nameRange"],
          steps: [
            {
              index: 0,
              name: "POST /login",
              nameRange: {} as ParsedFile["ranges"]["tests"][number]["steps"][number]["nameRange"],
            },
          ],
        },
      ],
    },
  };
}

function report(overrides: Partial<Report>): Report {
  return {
    schema_version: 1,
    version: "1",
    timestamp: "2026-04-10T12:00:00Z",
    duration_ms: 10,
    files: [],
    summary: {
      files: 0,
      tests: 0,
      steps: { total: 0, passed: 0, failed: 0 },
      status: "PASSED",
    },
    ...overrides,
  };
}

describe("flattenReportToPlan", () => {
  it("returns an empty list when no files failed", () => {
    const groups = flattenReportToPlan(
      report({
        files: [
          {
            file: "tests/ok.tarn.yaml",
            name: "ok",
            status: "PASSED",
            duration_ms: 10,
            summary: { total: 1, passed: 1, failed: 0 },
            setup: [],
            tests: [
              {
                name: "t",
                description: null,
                status: "PASSED",
                duration_ms: 10,
                steps: [{ name: "s", status: "PASSED", duration_ms: 10 }],
              },
            ],
            teardown: [],
          },
        ],
      }),
      () => undefined,
    );
    expect(groups).toEqual([]);
  });

  it("creates one entry per remediation hint, grouped by category", () => {
    const groups = flattenReportToPlan(
      report({
        files: [
          {
            file: "tests/login.tarn.yaml",
            name: "login",
            status: "FAILED",
            duration_ms: 50,
            summary: { total: 2, passed: 0, failed: 2 },
            setup: [],
            tests: [
              {
                name: "login",
                description: null,
                status: "FAILED",
                duration_ms: 50,
                steps: [
                  {
                    name: "POST /login",
                    status: "FAILED",
                    duration_ms: 30,
                    failure_category: "assertion_failed",
                    error_code: "assertion_mismatch",
                    remediation_hints: [
                      "Check that the response matches schema.json",
                      "Verify the `status` field in the expected body",
                    ],
                  },
                  {
                    name: "GET /me",
                    status: "FAILED",
                    duration_ms: 20,
                    failure_category: "connection_error",
                    error_code: "connection_refused",
                    remediation_hints: ["Start the upstream service before running."],
                  },
                ],
              },
            ],
            teardown: [],
          },
        ],
      }),
      () => undefined,
    );
    expect(groups).toHaveLength(2);
    const assertion = groups.find((g) => g.category === "assertion_failed")!;
    expect(assertion.entries).toHaveLength(2);
    expect(assertion.entries[0].hint).toContain("schema.json");
    const conn = groups.find((g) => g.category === "connection_error")!;
    expect(conn.entries).toHaveLength(1);
  });

  it("surfaces failed steps with no hints via a synthetic placeholder", () => {
    const groups = flattenReportToPlan(
      report({
        files: [
          {
            file: "tests/x.tarn.yaml",
            name: "x",
            status: "FAILED",
            duration_ms: 10,
            summary: { total: 1, passed: 0, failed: 1 },
            setup: [],
            tests: [
              {
                name: "t",
                description: null,
                status: "FAILED",
                duration_ms: 10,
                steps: [
                  {
                    name: "s",
                    status: "FAILED",
                    duration_ms: 10,
                    failure_category: "timeout",
                    error_code: "request_timed_out",
                  },
                ],
              },
            ],
            teardown: [],
          },
        ],
      }),
      () => undefined,
    );
    expect(groups).toHaveLength(1);
    expect(groups[0].category).toBe("timeout");
    expect(groups[0].entries).toHaveLength(1);
    expect(groups[0].entries[0].hint).toMatch(/No remediation hints/);
    expect(groups[0].entries[0].hint).toContain("request_timed_out");
  });

  it("defaults to the 'unknown' category when failure_category is missing", () => {
    const groups = flattenReportToPlan(
      report({
        files: [
          {
            file: "tests/y.tarn.yaml",
            name: "y",
            status: "FAILED",
            duration_ms: 10,
            summary: { total: 1, passed: 0, failed: 1 },
            setup: [],
            tests: [
              {
                name: "t",
                description: null,
                status: "FAILED",
                duration_ms: 10,
                steps: [
                  {
                    name: "s",
                    status: "FAILED",
                    duration_ms: 10,
                    remediation_hints: ["tweak something"],
                  },
                ],
              },
            ],
            teardown: [],
          },
        ],
      }),
      () => undefined,
    );
    expect(groups[0].category).toBe("unknown");
  });

  it("attaches location when the file is parsed by the index", () => {
    const parsed = stubParsedFile("tests/login.tarn.yaml");
    const groups = flattenReportToPlan(
      report({
        files: [
          {
            file: "tests/login.tarn.yaml",
            name: "login",
            status: "FAILED",
            duration_ms: 10,
            summary: { total: 1, passed: 0, failed: 1 },
            setup: [],
            tests: [
              {
                name: "login",
                description: null,
                status: "FAILED",
                duration_ms: 10,
                steps: [
                  {
                    name: "POST /login",
                    status: "FAILED",
                    duration_ms: 10,
                    failure_category: "assertion_failed",
                    remediation_hints: ["fix it"],
                  },
                ],
              },
            ],
            teardown: [],
          },
        ],
      }),
      () => parsed,
    );
    expect(groups[0].entries[0].location?.uri).toBe(parsed.uri);
    expect(groups[0].entries[0].location?.range).toBe(
      parsed.ranges.tests[0].steps[0].nameRange,
    );
  });

  it("orders categories with assertion_failed first and unknown last", () => {
    const groups = flattenReportToPlan(
      report({
        files: [
          {
            file: "tests/multi.tarn.yaml",
            name: "multi",
            status: "FAILED",
            duration_ms: 30,
            summary: { total: 3, passed: 0, failed: 3 },
            setup: [],
            tests: [
              {
                name: "t",
                description: null,
                status: "FAILED",
                duration_ms: 30,
                steps: [
                  {
                    name: "s1",
                    status: "FAILED",
                    duration_ms: 10,
                    failure_category: "timeout",
                    remediation_hints: ["x"],
                  },
                  {
                    name: "s2",
                    status: "FAILED",
                    duration_ms: 10,
                    remediation_hints: ["y"],
                  },
                  {
                    name: "s3",
                    status: "FAILED",
                    duration_ms: 10,
                    failure_category: "assertion_failed",
                    remediation_hints: ["z"],
                  },
                ],
              },
            ],
            teardown: [],
          },
        ],
      }),
      () => undefined,
    );
    const categories = groups.map((g) => g.category);
    expect(categories[0]).toBe("assertion_failed");
    expect(categories[categories.length - 1]).toBe("unknown");
  });
});

describe("categoryOrder + humanizeCategory", () => {
  it("gives known categories a deterministic order", () => {
    expect(categoryOrder("assertion_failed")).toBeLessThan(
      categoryOrder("connection_error"),
    );
    expect(categoryOrder("unknown")).toBeGreaterThan(
      categoryOrder("connection_error"),
    );
    expect(categoryOrder("brand_new_category")).toBeGreaterThanOrEqual(99);
  });

  it("humanizes known categories and leaves unknown ones as-is", () => {
    expect(humanizeCategory("assertion_failed")).toBe("Assertion failed");
    expect(humanizeCategory("unknown")).toBe("Other");
    expect(humanizeCategory("future_cat")).toBe("future_cat");
  });
});

describe("deserializeRange", () => {
  it("rebuilds a vscode.Range from a flat tuple", () => {
    const range = deserializeRange([1, 2, 3, 4]);
    expect(range.start.line).toBe(1);
    expect(range.start.character).toBe(2);
    expect(range.end.line).toBe(3);
    expect(range.end.character).toBe(4);
  });
});
