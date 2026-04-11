import { describe, it, expect } from "vitest";
import { parseReport } from "../../src/util/schemaGuards";

const passingReport = {
  schema_version: 1,
  version: "1",
  timestamp: "2026-04-10T00:00:00Z",
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
          name: "default",
          description: null,
          status: "PASSED",
          duration_ms: 42,
          steps: [
            {
              name: "GET /health",
              status: "PASSED",
              duration_ms: 42,
              assertions: { total: 1, passed: 1, failed: 0, details: [], failures: [] },
            },
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

const failingReport = {
  duration_ms: 100,
  files: [
    {
      file: "tests/users.tarn.yaml",
      name: "Users",
      status: "FAILED",
      duration_ms: 100,
      summary: { total: 2, passed: 1, failed: 1 },
      tests: [
        {
          name: "create_user",
          description: "Creates a user",
          status: "FAILED",
          duration_ms: 100,
          steps: [
            {
              name: "Create",
              status: "FAILED",
              duration_ms: 100,
              failure_category: "assertion_failed",
              error_code: "assertion_mismatch",
              remediation_hints: ["Check the expected status"],
              assertions: {
                total: 1,
                passed: 0,
                failed: 1,
                failures: [
                  {
                    assertion: "status",
                    passed: false,
                    expected: "201",
                    actual: "500",
                    message: "unexpected status",
                    diff: "--- expected\n+++ actual\n-201\n+500",
                  },
                ],
              },
              request: {
                method: "POST",
                url: "http://localhost:3000/users",
                headers: { "content-type": "application/json" },
                body: { name: "alice" },
              },
              response: {
                status: 500,
                headers: {},
                body: { error: "kaboom" },
              },
            },
          ],
        },
      ],
    },
  ],
  summary: {
    files: 1,
    tests: 1,
    steps: { total: 1, passed: 0, failed: 1 },
    status: "FAILED",
  },
};

describe("parseReport", () => {
  it("accepts a passing report", () => {
    const report = parseReport(JSON.stringify(passingReport));
    expect(report.summary.status).toBe("PASSED");
    expect(report.files[0].tests[0].steps[0].status).toBe("PASSED");
  });

  it("accepts a failing report with rich failure detail", () => {
    const report = parseReport(JSON.stringify(failingReport));
    const step = report.files[0].tests[0].steps[0];
    expect(step.status).toBe("FAILED");
    expect(step.failure_category).toBe("assertion_failed");
    expect(step.error_code).toBe("assertion_mismatch");
    expect(step.assertions?.failures?.[0].diff).toContain("+500");
    expect(step.request?.method).toBe("POST");
    expect(step.response?.status).toBe(500);
  });

  it("accepts the real tarn JSON shape: diff=null, no passed on failures[]", () => {
    // Regression: the schema used to require `diff: string | undefined`
    // and `passed: bool` on every assertion entry, but the real tarn
    // binary emits `diff: null` and omits `passed` inside `failures[]`
    // because those entries are by definition failed. parseReport must
    // accept that shape so a run with even one failing step does not
    // collapse to `report: undefined` in the runner.
    const realShape = {
      duration_ms: 5,
      files: [
        {
          file: "tests/cookie.tarn.yaml",
          name: "Cookie",
          status: "FAILED",
          duration_ms: 5,
          summary: { total: 1, passed: 0, failed: 1 },
          tests: [
            {
              name: "needs_clean_jar",
              status: "FAILED",
              duration_ms: 5,
              steps: [
                {
                  name: "confirm no session",
                  status: "FAILED",
                  duration_ms: 5,
                  assertions: {
                    total: 2,
                    passed: 1,
                    failed: 1,
                    details: [
                      {
                        assertion: "body $.session",
                        passed: false,
                        expected: "null",
                        actual: "\"abc123\"",
                        message: "JSONPath $.session: expected null",
                        diff: null,
                      },
                    ],
                    failures: [
                      {
                        // note: no `passed` field — the real tarn
                        // binary omits it inside failures[]
                        assertion: "body $.session",
                        expected: "null",
                        actual: "\"abc123\"",
                        message: "JSONPath $.session: expected null",
                        diff: null,
                      },
                    ],
                  },
                },
              ],
            },
          ],
        },
      ],
      summary: {
        files: 1,
        tests: 1,
        steps: { total: 1, passed: 0, failed: 1 },
        status: "FAILED" as const,
      },
    };
    const report = parseReport(JSON.stringify(realShape));
    expect(report.summary.status).toBe("FAILED");
    expect(report.files[0].tests[0].steps[0].assertions?.failures?.[0].diff).toBeNull();
  });

  it("rejects reports with wrong enum values", () => {
    const bad = { ...passingReport, summary: { ...passingReport.summary, status: "SKIPPED" } };
    expect(() => parseReport(JSON.stringify(bad))).toThrow();
  });

  it("rejects reports missing required fields", () => {
    const bad = { duration_ms: 1 };
    expect(() => parseReport(JSON.stringify(bad))).toThrow();
  });

  it("accepts optional `location` on steps and on assertion details/failures (NAZ-281)", () => {
    // Tarn T55 (NAZ-260) attaches a 1-based `location: { file, line, column }`
    // to every step and to every assertion detail/failure that maps back to
    // a YAML operator key. The extension consumes this in ResultMapper, so
    // the zod schema must preserve it through parseReport without dropping
    // the field or rejecting the payload.
    const withLocations = {
      duration_ms: 7,
      files: [
        {
          file: "tests/health.tarn.yaml",
          name: "Health",
          status: "FAILED",
          duration_ms: 7,
          summary: { total: 1, passed: 0, failed: 1 },
          tests: [
            {
              name: "smoke",
              description: null,
              status: "FAILED",
              duration_ms: 7,
              steps: [
                {
                  name: "GET /status/500",
                  status: "FAILED",
                  duration_ms: 7,
                  location: {
                    file: "/ws/tests/health.tarn.yaml",
                    line: 9,
                    column: 9,
                  },
                  assertions: {
                    total: 1,
                    passed: 0,
                    failed: 1,
                    details: [
                      {
                        assertion: "status",
                        passed: false,
                        expected: "200",
                        actual: "500",
                        message: "Expected HTTP status 200, got 500",
                        diff: null,
                        location: {
                          file: "/ws/tests/health.tarn.yaml",
                          line: 14,
                          column: 11,
                        },
                      },
                    ],
                    failures: [
                      {
                        assertion: "status",
                        expected: "200",
                        actual: "500",
                        message: "Expected HTTP status 200, got 500",
                        diff: null,
                        location: {
                          file: "/ws/tests/health.tarn.yaml",
                          line: 14,
                          column: 11,
                        },
                      },
                    ],
                  },
                },
              ],
            },
          ],
        },
      ],
      summary: {
        files: 1,
        tests: 1,
        steps: { total: 1, passed: 0, failed: 1 },
        status: "FAILED" as const,
      },
    };
    const report = parseReport(JSON.stringify(withLocations));
    const step = report.files[0].tests[0].steps[0];
    expect(step.location).toEqual({
      file: "/ws/tests/health.tarn.yaml",
      line: 9,
      column: 9,
    });
    expect(step.assertions?.details?.[0].location).toEqual({
      file: "/ws/tests/health.tarn.yaml",
      line: 14,
      column: 11,
    });
    expect(step.assertions?.failures?.[0].location).toEqual({
      file: "/ws/tests/health.tarn.yaml",
      line: 14,
      column: 11,
    });
  });

  it("rejects a location with non-positive line (must be 1-based)", () => {
    // Tarn spec: line and column are 1-based >= 1. A line of 0 means
    // a producer bug and must not silently coerce to a valid Position.
    const bad = {
      ...passingReport,
      files: [
        {
          ...passingReport.files[0],
          tests: [
            {
              ...passingReport.files[0].tests[0],
              steps: [
                {
                  ...passingReport.files[0].tests[0].steps[0],
                  location: { file: "x.yaml", line: 0, column: 1 },
                },
              ],
            },
          ],
        },
      ],
    };
    expect(() => parseReport(JSON.stringify(bad))).toThrow();
  });
});
