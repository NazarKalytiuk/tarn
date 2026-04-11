import * as vscode from "vscode";
import type {
  Report,
  FileResult,
  StepResult,
  TestResult,
  AssertionDetail,
  Location as TarnLocation,
} from "../util/schemaGuards";
import type { ParsedFile } from "../workspace/WorkspaceIndex";
import { ids } from "./discovery";

export interface MapContext {
  run: vscode.TestRun;
  parsedByPath: Map<string, ParsedFile>;
  testItemsById: Map<string, vscode.TestItem>;
}

export function applyReport(report: Report, ctx: MapContext): void {
  for (const file of report.files) {
    applyFile(file, ctx);
  }
}

function applyFile(file: FileResult, ctx: MapContext): void {
  const parsed = ctx.parsedByPath.get(file.file) ?? ctx.parsedByPath.get(absKey(file.file));
  if (!parsed) {
    return;
  }
  for (const test of file.tests) {
    applyTest(parsed, test, ctx);
  }
}

function absKey(filePath: string): string {
  return filePath;
}

function applyTest(parsed: ParsedFile, test: TestResult, ctx: MapContext): void {
  const testId = ids.test(parsed.uri, test.name);
  const testItem = ctx.testItemsById.get(testId);

  if (testItem) {
    ctx.run.started(testItem);
  }

  test.steps.forEach((step, stepIndex) => {
    const stepId = ids.step(parsed.uri, test.name, stepIndex);
    const stepItem = ctx.testItemsById.get(stepId);
    if (stepItem) {
      applyStep(stepItem, step, parsed, ctx);
    }
  });

  if (!testItem) {
    return;
  }

  if (test.status === "PASSED") {
    ctx.run.passed(testItem, test.duration_ms);
  } else {
    const message = new vscode.TestMessage(`Test "${test.name}" failed`);
    ctx.run.failed(testItem, message, test.duration_ms);
  }
}

function applyStep(
  stepItem: vscode.TestItem,
  step: StepResult,
  parsed: ParsedFile,
  ctx: MapContext,
): void {
  ctx.run.started(stepItem);

  if (step.status === "PASSED") {
    ctx.run.passed(stepItem, step.duration_ms);
    return;
  }

  const messages = buildFailureMessages(step, stepItem, parsed);
  ctx.run.failed(stepItem, messages, step.duration_ms);
}

export function buildFailureMessages(
  step: StepResult,
  stepItem: vscode.TestItem,
  parsed: ParsedFile,
): vscode.TestMessage[] {
  const messages: vscode.TestMessage[] = [];

  // Preference order for the step-level fallback anchor:
  //   1. step.location from the JSON report (Tarn T55, NAZ-260)
  //   2. stepItem.range derived from the current workspace YAML AST
  //
  // The JSON path is drift-free: the line/column were captured when
  // Tarn loaded the file at run time, so they still point at the
  // original step even if the user edited the file since then.
  const stepLocation = resolveStepLocation(step, stepItem, parsed);

  const failures = step.assertions?.failures ?? [];
  if (failures.length > 0) {
    for (const failure of failures) {
      // Each assertion failure can carry its own location — typically
      // the `assert.<operator>` key inside the step body. When it does,
      // use it so the squiggle lands on the exact operator node.
      const failureLocation =
        locationFromTarn(failure.location, parsed) ?? stepLocation;
      messages.push(renderAssertionFailure(step, failure, failureLocation));
    }
  } else {
    messages.push(renderGenericFailure(step, stepLocation));
  }

  return messages;
}

/**
 * Resolve the step-level source anchor used for failure decorations.
 *
 * Prefers `step.location` from the JSON report, which is authoritative
 * at run time and survives concurrent file edits (drift-free). Falls
 * back to the AST-derived `stepItem.range` so older Tarn versions and
 * `include:`-expanded steps (where Tarn emits `location: None`) still
 * produce a usable anchor.
 */
export function resolveStepLocation(
  step: StepResult,
  stepItem: vscode.TestItem,
  parsed: ParsedFile,
): vscode.Location | undefined {
  const fromJson = locationFromTarn(step.location, parsed);
  if (fromJson) {
    return fromJson;
  }
  if (stepItem.range) {
    return new vscode.Location(parsed.uri, stepItem.range);
  }
  return undefined;
}

/**
 * Convert a Tarn-reported (1-based) `location` into a `vscode.Location`.
 *
 * Tarn emits `line` and `column` as 1-based indices because that is
 * what every error message in the CLI already uses. VS Code's
 * `Position` API is 0-based, so every consumer must decrement before
 * constructing a `Position`. The resulting range is a zero-width
 * caret at the location; the editor expands it to the enclosing token
 * for rendering.
 *
 * Prefers the URI of the parsed YAML file we already hold in memory.
 * Only falls back to building a fresh URI from `location.file` when
 * that path does not match — a defensive branch for future scenarios
 * where Tarn may report a location inside an `include:`d sub-file.
 */
export function locationFromTarn(
  location: TarnLocation | undefined,
  parsed: ParsedFile,
): vscode.Location | undefined {
  if (!location) {
    return undefined;
  }
  const line = Math.max(location.line - 1, 0);
  const column = Math.max(location.column - 1, 0);
  const position = new vscode.Position(line, column);
  const range = new vscode.Range(position, position);
  const uri = resolveLocationUri(location.file, parsed);
  return new vscode.Location(uri, range);
}

function resolveLocationUri(
  reportedFile: string,
  parsed: ParsedFile,
): vscode.Uri {
  // If the location points at the same file we already indexed, reuse
  // the parsed URI to keep VS Code's URI identity stable with the rest
  // of the mapping pipeline.
  if (
    reportedFile === parsed.uri.fsPath ||
    parsed.uri.fsPath.endsWith(reportedFile) ||
    reportedFile.endsWith(parsed.uri.fsPath)
  ) {
    return parsed.uri;
  }
  return vscode.Uri.file(reportedFile);
}

function renderAssertionFailure(
  step: StepResult,
  failure: AssertionDetail,
  location: vscode.Location | undefined,
): vscode.TestMessage {
  const parts: string[] = [];
  parts.push(`**${step.name}** — ${failure.assertion}`);
  if (failure.message) {
    parts.push(failure.message);
  }
  if (failure.expected !== undefined) {
    parts.push(`Expected: \`${failure.expected}\``);
  }
  if (failure.actual !== undefined) {
    parts.push(`Actual: \`${failure.actual}\``);
  }
  if (step.failure_category) {
    parts.push(`Category: \`${step.failure_category}\``);
  }
  if (step.error_code) {
    parts.push(`Error code: \`${step.error_code}\``);
  }
  if (step.remediation_hints && step.remediation_hints.length > 0) {
    parts.push("Hints:");
    for (const hint of step.remediation_hints) {
      parts.push(`- ${hint}`);
    }
  }
  if (failure.diff) {
    parts.push("```diff");
    parts.push(failure.diff);
    parts.push("```");
  }
  if (step.request) {
    parts.push("Request:");
    parts.push("```http");
    parts.push(`${step.request.method} ${step.request.url}`);
    if (step.request.headers) {
      for (const [k, v] of Object.entries(step.request.headers)) {
        parts.push(`${k}: ${v}`);
      }
    }
    if (step.request.body !== undefined) {
      parts.push("");
      parts.push(stringifyBody(step.request.body));
    }
    parts.push("```");
  }
  if (step.response) {
    parts.push("Response:");
    parts.push("```http");
    parts.push(`HTTP ${step.response.status}`);
    if (step.response.headers) {
      for (const [k, v] of Object.entries(step.response.headers)) {
        parts.push(`${k}: ${v}`);
      }
    }
    if (step.response.body !== undefined) {
      parts.push("");
      parts.push(stringifyBody(step.response.body));
    }
    parts.push("```");
  }

  const md = new vscode.MarkdownString(parts.join("\n\n"));
  md.supportHtml = false;
  md.isTrusted = false;
  const message = new vscode.TestMessage(md);
  if (failure.expected !== undefined) {
    message.expectedOutput = failure.expected;
  }
  if (failure.actual !== undefined) {
    message.actualOutput = failure.actual;
  }
  if (location) {
    message.location = location;
  }
  return message;
}

function renderGenericFailure(
  step: StepResult,
  location: vscode.Location | undefined,
): vscode.TestMessage {
  const parts: string[] = [];
  parts.push(`**${step.name}** failed`);
  if (step.failure_category) {
    parts.push(`Category: \`${step.failure_category}\``);
  }
  if (step.error_code) {
    parts.push(`Error code: \`${step.error_code}\``);
  }
  if (step.remediation_hints) {
    for (const hint of step.remediation_hints) {
      parts.push(`- ${hint}`);
    }
  }
  const md = new vscode.MarkdownString(parts.join("\n\n"));
  const message = new vscode.TestMessage(md);
  if (location) {
    message.location = location;
  }
  return message;
}

function stringifyBody(body: unknown): string {
  if (typeof body === "string") {
    return body;
  }
  try {
    return JSON.stringify(body, null, 2);
  } catch {
    return String(body);
  }
}
