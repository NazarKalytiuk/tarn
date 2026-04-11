import { describe, it, expect } from "vitest";
import {
  buildFailureMessages,
  locationFromTarn,
  resolveStepLocation,
} from "../../src/testing/ResultMapper";
import type { StepResult } from "../../src/util/schemaGuards";
import { Location, Range, Position, Uri, MarkdownString } from "./__mocks__/vscode";

function fakeStepItem() {
  return {
    range: new Range(new Position(10, 2), new Position(10, 20)),
  };
}

function fakeParsed() {
  return {
    uri: Uri.file("/fake/tests/users.tarn.yaml"),
    ranges: { fileName: "users", tests: [], setup: [], teardown: [], fileNameRange: undefined },
  };
}

describe("buildFailureMessages", () => {
  it("renders assertion_mismatch with diff, expected, actual, and request/response", () => {
    const step: StepResult = {
      name: "Create user",
      status: "FAILED",
      duration_ms: 42,
      failure_category: "assertion_failed",
      error_code: "assertion_mismatch",
      remediation_hints: ["check server status"],
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
        url: "http://localhost/users",
        headers: { "content-type": "application/json" },
        body: { name: "alice" },
      },
      response: { status: 500, headers: {}, body: { error: "boom" } },
    };

    const msgs = buildFailureMessages(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(1);
    const m = msgs[0];
    expect(m.expectedOutput).toBe("201");
    expect(m.actualOutput).toBe("500");
    expect(m.location).toBeDefined();
    expect(m.message).toBeInstanceOf(MarkdownString);
    const text = (m.message as MarkdownString).value;
    expect(text).toContain("Create user");
    expect(text).toContain("assertion_failed");
    expect(text).toContain("assertion_mismatch");
    expect(text).toContain("check server status");
    expect(text).toContain("+500");
    expect(text).toContain("POST http://localhost/users");
    expect(text).toContain("HTTP 500");
    expect(text).toContain("alice");
    expect(text).toContain("boom");
  });

  it("emits one message per assertion failure", () => {
    const step: StepResult = {
      name: "Multi-assert",
      status: "FAILED",
      duration_ms: 1,
      assertions: {
        total: 2,
        passed: 0,
        failed: 2,
        failures: [
          { assertion: "status", passed: false, expected: "200", actual: "500" },
          { assertion: "body $.ok", passed: false, expected: "true", actual: "false" },
        ],
      },
    };
    const msgs = buildFailureMessages(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(2);
    expect(msgs[0].expectedOutput).toBe("200");
    expect(msgs[1].expectedOutput).toBe("true");
  });

  it("falls back to a generic message when no assertion failures are attached", () => {
    const step: StepResult = {
      name: "Connect",
      status: "FAILED",
      duration_ms: 1500,
      failure_category: "connection_error",
      error_code: "connection_refused",
      remediation_hints: ["is the server running?"],
    };
    const msgs = buildFailureMessages(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(1);
    const text = (msgs[0].message as MarkdownString).value;
    expect(text).toContain("Connect");
    expect(text).toContain("connection_error");
    expect(text).toContain("connection_refused");
    expect(text).toContain("is the server running?");
  });

  it("covers every documented failure category with a generic message", () => {
    const categories = [
      "assertion_failed",
      "connection_error",
      "timeout",
      "parse_error",
      "capture_error",
      "unresolved_template",
    ] as const;
    for (const category of categories) {
      const step: StepResult = {
        name: `${category} step`,
        status: "FAILED",
        duration_ms: 1,
        failure_category: category,
      };
      const msgs = buildFailureMessages(
        step,
        fakeStepItem() as never,
        fakeParsed() as never,
      );
      expect(msgs).toHaveLength(1);
      const text = (msgs[0].message as MarkdownString).value;
      expect(text).toContain(category);
    }
  });
});

describe("locationFromTarn", () => {
  it("converts 1-based line/column to a zero-width 0-based vscode.Location", () => {
    const parsed = fakeParsed();
    const loc = locationFromTarn(
      { file: "/fake/tests/users.tarn.yaml", line: 14, column: 11 },
      parsed as never,
    );
    expect(loc).toBeDefined();
    expect(loc!.range.start.line).toBe(13);
    expect(loc!.range.start.character).toBe(10);
    // Zero-width caret: Tarn reports a single point; VS Code expands it.
    expect(loc!.range.end.line).toBe(13);
    expect(loc!.range.end.character).toBe(10);
  });

  it("clamps line/column 1 (the smallest valid value) down to 0-based 0", () => {
    const parsed = fakeParsed();
    const loc = locationFromTarn(
      { file: "/fake/tests/users.tarn.yaml", line: 1, column: 1 },
      parsed as never,
    );
    expect(loc).toBeDefined();
    expect(loc!.range.start.line).toBe(0);
    expect(loc!.range.start.character).toBe(0);
  });

  it("returns undefined when no location is provided (older Tarn versions)", () => {
    const parsed = fakeParsed();
    expect(locationFromTarn(undefined, parsed as never)).toBeUndefined();
  });

  it("reuses the ParsedFile URI when the reported file matches", () => {
    const parsed = fakeParsed();
    const loc = locationFromTarn(
      { file: "/fake/tests/users.tarn.yaml", line: 5, column: 3 },
      parsed as never,
    );
    expect(loc).toBeDefined();
    expect(loc!.uri).toBe(parsed.uri);
  });
});

describe("resolveStepLocation", () => {
  it("prefers step.location from the JSON report over the AST range", () => {
    // AST range says line 10; JSON report says line 25 (1-based).
    // The JSON value wins because it was captured at run time and is
    // drift-free with respect to concurrent edits in the editor.
    const step: StepResult = {
      name: "GET /status/500",
      status: "FAILED",
      duration_ms: 5,
      location: { file: "/fake/tests/users.tarn.yaml", line: 25, column: 7 },
    };
    const loc = resolveStepLocation(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(loc).toBeInstanceOf(Location);
    expect(loc!.range.start.line).toBe(24);
    expect(loc!.range.start.character).toBe(6);
  });

  it("falls back to the AST stepItem.range when JSON location is missing", () => {
    // Simulates a report produced by a Tarn version that does not yet
    // emit `location` (or an `include:`-expanded step where Tarn emits
    // `location: None`). We must still anchor the failure.
    const step: StepResult = {
      name: "Legacy step",
      status: "FAILED",
      duration_ms: 5,
    };
    const loc = resolveStepLocation(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(loc).toBeDefined();
    // fakeStepItem() places the AST range on line 10.
    expect(loc!.range.start.line).toBe(10);
    expect(loc!.range.start.character).toBe(2);
  });

  it("returns undefined when neither JSON nor AST location is available", () => {
    const step: StepResult = {
      name: "Orphan",
      status: "FAILED",
      duration_ms: 5,
    };
    const stepItemWithoutRange = { range: undefined };
    const loc = resolveStepLocation(
      step,
      stepItemWithoutRange as never,
      fakeParsed() as never,
    );
    expect(loc).toBeUndefined();
  });
});

describe("buildFailureMessages with location metadata (NAZ-281)", () => {
  it("anchors an assertion failure on the failure's own JSON location", () => {
    // The smoking gun: AST says line 10, step.location says line 25,
    // and the failing assertion says line 27. The assertion location
    // must win, overriding both the AST and the step-level location.
    // This is what makes the squiggle land on the exact operator key
    // (`status: 200`) instead of the step's `name:` key.
    const step: StepResult = {
      name: "GET /status/500",
      status: "FAILED",
      duration_ms: 42,
      failure_category: "assertion_failed",
      error_code: "assertion_mismatch",
      location: { file: "/fake/tests/users.tarn.yaml", line: 25, column: 7 },
      assertions: {
        total: 1,
        passed: 0,
        failed: 1,
        failures: [
          {
            assertion: "status",
            expected: "200",
            actual: "500",
            message: "Expected HTTP status 200, got 500",
            diff: null,
            location: {
              file: "/fake/tests/users.tarn.yaml",
              line: 27,
              column: 11,
            },
          },
        ],
      },
    };
    const msgs = buildFailureMessages(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(1);
    const msg = msgs[0];
    expect(msg.location).toBeDefined();
    expect(msg.location!.range.start.line).toBe(26);
    expect(msg.location!.range.start.character).toBe(10);
  });

  it("falls back to step.location when the assertion lacks its own", () => {
    // Scenario: Tarn emits the step location but cannot map the
    // assertion back to a specific operator key (e.g., the assertion
    // was synthesized by a capture failure). The step-level JSON
    // location is still preferred over the stale AST range.
    const step: StepResult = {
      name: "GET /users",
      status: "FAILED",
      duration_ms: 42,
      location: { file: "/fake/tests/users.tarn.yaml", line: 25, column: 7 },
      assertions: {
        total: 1,
        passed: 0,
        failed: 1,
        failures: [
          {
            assertion: "capture $.id",
            expected: "string",
            actual: "null",
            // no location
          },
        ],
      },
    };
    const msgs = buildFailureMessages(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(1);
    expect(msgs[0].location!.range.start.line).toBe(24);
    expect(msgs[0].location!.range.start.character).toBe(6);
  });

  it("survives AST drift: JSON line wins even when AST range was updated to a new line", () => {
    // Simulated drift: the user inserted a blank line above the step
    // between run start and report parse, so the current AST range
    // moved from line 10 to line 11. The step was originally at line
    // 10 in the YAML Tarn actually executed, and Tarn reported that.
    // The TestMessage location must reflect the pre-edit line (the
    // file Tarn ran), not the post-edit AST.
    const driftedStepItem = {
      range: new Range(new Position(11, 2), new Position(11, 20)),
    };
    const step: StepResult = {
      name: "GET /flaky",
      status: "FAILED",
      duration_ms: 10,
      // 1-based line 10 = 0-based 9, i.e. the pre-edit position.
      location: { file: "/fake/tests/users.tarn.yaml", line: 10, column: 7 },
      assertions: {
        total: 1,
        passed: 0,
        failed: 1,
        failures: [
          {
            assertion: "status",
            expected: "200",
            actual: "503",
            location: {
              file: "/fake/tests/users.tarn.yaml",
              line: 15,
              column: 11,
            },
          },
        ],
      },
    };
    const msgs = buildFailureMessages(
      step,
      driftedStepItem as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(1);
    // Must be 14 (= 15 - 1), NOT 11 from the drifted AST range.
    expect(msgs[0].location!.range.start.line).toBe(14);
  });

  it("uses AST fallback when the report is from an older Tarn (no location)", () => {
    // Forward compatibility check: a step from Tarn 0.x without any
    // location metadata still gets anchored via the AST-derived range
    // that was captured when discovery built the TestItem tree.
    const step: StepResult = {
      name: "Legacy",
      status: "FAILED",
      duration_ms: 5,
      assertions: {
        total: 1,
        passed: 0,
        failed: 1,
        failures: [
          {
            assertion: "status",
            expected: "200",
            actual: "500",
          },
        ],
      },
    };
    const msgs = buildFailureMessages(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(1);
    // fakeStepItem() has line 10 in the AST mock.
    expect(msgs[0].location!.range.start.line).toBe(10);
  });

  it("emits distinct locations for every assertion failure when each has its own", () => {
    // Multiple failed assertions on the same step, each mapped to a
    // different line in the YAML (e.g., `status` on one line, a body
    // JSONPath assertion several lines below).
    const step: StepResult = {
      name: "Multi-assert",
      status: "FAILED",
      duration_ms: 1,
      location: { file: "/fake/tests/users.tarn.yaml", line: 20, column: 7 },
      assertions: {
        total: 2,
        passed: 0,
        failed: 2,
        failures: [
          {
            assertion: "status",
            expected: "200",
            actual: "500",
            location: {
              file: "/fake/tests/users.tarn.yaml",
              line: 22,
              column: 11,
            },
          },
          {
            assertion: "body $.ok",
            expected: "true",
            actual: "false",
            location: {
              file: "/fake/tests/users.tarn.yaml",
              line: 28,
              column: 13,
            },
          },
        ],
      },
    };
    const msgs = buildFailureMessages(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(2);
    expect(msgs[0].location!.range.start.line).toBe(21);
    expect(msgs[0].location!.range.start.character).toBe(10);
    expect(msgs[1].location!.range.start.line).toBe(27);
    expect(msgs[1].location!.range.start.character).toBe(12);
  });

  it("anchors a generic (non-assertion) failure on the JSON step.location", () => {
    // Connection errors have no assertion failures attached. The
    // generic failure message must still pick up the JSON step
    // location instead of the stale AST range.
    const step: StepResult = {
      name: "Connect to unreachable host",
      status: "FAILED",
      duration_ms: 1500,
      failure_category: "connection_error",
      error_code: "connection_refused",
      location: { file: "/fake/tests/users.tarn.yaml", line: 33, column: 9 },
    };
    const msgs = buildFailureMessages(
      step,
      fakeStepItem() as never,
      fakeParsed() as never,
    );
    expect(msgs).toHaveLength(1);
    expect(msgs[0].location!.range.start.line).toBe(32);
    expect(msgs[0].location!.range.start.character).toBe(8);
  });
});
