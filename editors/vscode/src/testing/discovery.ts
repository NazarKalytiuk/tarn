import * as vscode from "vscode";
import type { ParsedFile } from "../workspace/WorkspaceIndex";
import type { StepRange, TestRange } from "../workspace/YamlAst";

export interface TestItemIds {
  file(uri: vscode.Uri): string;
  test(uri: vscode.Uri, testName: string): string;
  step(uri: vscode.Uri, testName: string, stepIndex: number): string;
}

export const ids: TestItemIds = {
  file: (uri) => `file:${uri.toString()}`,
  test: (uri, testName) => `test:${uri.toString()}::${testName}`,
  step: (uri, testName, stepIndex) => `step:${uri.toString()}::${testName}::${stepIndex}`,
};

/** Structured metadata attached to a discovered TestItem. */
export type ItemMeta =
  | { kind: "file"; uri: vscode.Uri }
  | { kind: "test"; uri: vscode.Uri; testName: string }
  | { kind: "step"; uri: vscode.Uri; testName: string; stepIndex: number };

const metaByItem = new WeakMap<vscode.TestItem, ItemMeta>();

export function getItemMeta(item: vscode.TestItem): ItemMeta | undefined {
  return metaByItem.get(item);
}

export function buildFileItem(
  controller: vscode.TestController,
  parsed: ParsedFile,
): vscode.TestItem {
  const fileItem = controller.createTestItem(
    ids.file(parsed.uri),
    parsed.ranges.fileName || parsed.uri.path.split("/").pop() || parsed.uri.toString(),
    parsed.uri,
  );
  fileItem.canResolveChildren = false;
  fileItem.description = vscode.workspace.asRelativePath(parsed.uri);
  if (parsed.ranges.fileNameRange) {
    fileItem.range = parsed.ranges.fileNameRange;
  }
  metaByItem.set(fileItem, { kind: "file", uri: parsed.uri });
  rebuildChildren(controller, fileItem, parsed);
  return fileItem;
}

export function rebuildChildren(
  controller: vscode.TestController,
  fileItem: vscode.TestItem,
  parsed: ParsedFile,
): void {
  const children: vscode.TestItem[] = [];
  for (const test of parsed.ranges.tests) {
    children.push(buildTestItem(controller, parsed.uri, test));
  }
  fileItem.children.replace(children);
}

function buildTestItem(
  controller: vscode.TestController,
  uri: vscode.Uri,
  test: TestRange,
): vscode.TestItem {
  const item = controller.createTestItem(ids.test(uri, test.name), test.name, uri);
  item.range = test.nameRange;
  item.description = test.description ?? undefined;
  metaByItem.set(item, { kind: "test", uri, testName: test.name });
  const stepItems = test.steps.map((step) => buildStepItem(controller, uri, test.name, step));
  item.children.replace(stepItems);
  return item;
}

function buildStepItem(
  controller: vscode.TestController,
  uri: vscode.Uri,
  testName: string,
  step: StepRange,
): vscode.TestItem {
  const item = controller.createTestItem(
    ids.step(uri, testName, step.index),
    step.name,
    uri,
  );
  item.range = step.nameRange;
  metaByItem.set(item, {
    kind: "step",
    uri,
    testName,
    stepIndex: step.index,
  });
  return item;
}
