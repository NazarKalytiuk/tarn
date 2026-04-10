import { LineCounter, parseDocument, isMap, isScalar, isSeq } from "yaml";
import type {
  Document as YAMLDocument,
  YAMLMap as YAMLMapType,
  YAMLSeq as YAMLSeqType,
} from "yaml";

/**
 * One capture variable declared by a prior step. `stepIndex` is 0-based
 * within the phase (setup / test). `phase` lets callers prefer setup
 * captures over same-test captures when naming collides, and adds
 * useful context to the completion UI.
 */
export interface VisibleCapture {
  name: string;
  stepIndex: number;
  phase: "setup" | "test";
  testName?: string;
  stepName: string;
}

/**
 * Compute which captures are visible at a given byte offset in a Tarn
 * test file. Rules (matching the Rust runner):
 *
 *   * Setup captures are visible from every step in every test.
 *   * Within a test, captures from strictly earlier steps are visible.
 *   * Captures from the same step, later steps, or other tests are not.
 *   * Teardown is not considered because templates in teardown can see
 *     both setup and test captures, so the full union is returned.
 *
 * Parses the YAML via the shared `yaml` library and walks the CST so
 * we do not need to keep a separate AST in sync with the workspace
 * index.
 */
export function collectVisibleCaptures(
  source: string,
  offset: number,
): VisibleCapture[] {
  const lineCounter = new LineCounter();
  let doc: YAMLDocument.Parsed;
  try {
    doc = parseDocument(source, { lineCounter, keepSourceTokens: false });
  } catch {
    return [];
  }
  if (doc.errors.length > 0) {
    return [];
  }
  const root = doc.contents;
  if (!isMap(root)) {
    return [];
  }

  const setupCaptures = collectStepSequence(root, "setup", "setup", undefined);
  const { testName, stepIndex, location } = locateCursor(root, offset);

  if (location === "outside") {
    return setupCaptures;
  }

  if (location === "setup") {
    return setupCaptures.filter((c) => c.stepIndex < stepIndex);
  }

  if (location === "flat-steps") {
    // Simple-format file: `steps:` at the root. The whole document is
    // one pseudo-test; setup captures plus prior steps are visible.
    const flatCaptures = collectStepSequence(
      root,
      "steps",
      "test",
      undefined,
    );
    return [
      ...setupCaptures,
      ...flatCaptures.filter((c) => c.stepIndex < stepIndex),
    ];
  }

  if (location === "test" && testName) {
    const testsNode = getMapValue(root, "tests");
    if (isMap(testsNode)) {
      const testValue = getMapValue(testsNode, testName);
      if (isMap(testValue)) {
        const testCaptures = collectStepSequence(
          testValue,
          "steps",
          "test",
          testName,
        );
        return [
          ...setupCaptures,
          ...testCaptures.filter((c) => c.stepIndex < stepIndex),
        ];
      }
    }
    return setupCaptures;
  }

  if (location === "teardown") {
    // Teardown runs after tests, so every previously declared capture
    // is in scope.
    const testsNode = getMapValue(root, "tests");
    const allTestCaptures: VisibleCapture[] = [];
    if (isMap(testsNode)) {
      for (const pair of testsNode.items) {
        if (!isScalar(pair.key)) continue;
        const name = String(pair.key.value);
        const value = pair.value;
        if (!isMap(value)) continue;
        allTestCaptures.push(
          ...collectStepSequence(value, "steps", "test", name),
        );
      }
    }
    return [...setupCaptures, ...allTestCaptures];
  }

  return setupCaptures;
}

type CursorLocation =
  | "outside"
  | "setup"
  | "flat-steps"
  | "test"
  | "teardown";

interface CursorInfo {
  location: CursorLocation;
  testName?: string;
  stepIndex: number;
}

function locateCursor(root: YAMLMapType, offset: number): CursorInfo {
  const setupSeq = getMapValue(root, "setup");
  if (isSeq(setupSeq)) {
    const idx = indexOfStepContainingOffset(setupSeq, offset);
    if (idx !== undefined) {
      return { location: "setup", stepIndex: idx };
    }
  }

  const flatSteps = getMapValue(root, "steps");
  if (isSeq(flatSteps)) {
    const idx = indexOfStepContainingOffset(flatSteps, offset);
    if (idx !== undefined) {
      return { location: "flat-steps", stepIndex: idx };
    }
  }

  const testsNode = getMapValue(root, "tests");
  if (isMap(testsNode)) {
    for (const pair of testsNode.items) {
      if (!isScalar(pair.key)) continue;
      const name = String(pair.key.value);
      const value = pair.value;
      if (!isMap(value)) continue;
      const stepsSeq = getMapValue(value, "steps");
      if (!isSeq(stepsSeq)) continue;
      const idx = indexOfStepContainingOffset(stepsSeq, offset);
      if (idx !== undefined) {
        return { location: "test", testName: name, stepIndex: idx };
      }
    }
  }

  const teardownSeq = getMapValue(root, "teardown");
  if (isSeq(teardownSeq)) {
    const idx = indexOfStepContainingOffset(teardownSeq, offset);
    if (idx !== undefined) {
      return { location: "teardown", stepIndex: idx };
    }
  }

  return { location: "outside", stepIndex: 0 };
}

function indexOfStepContainingOffset(
  seq: YAMLSeqType,
  offset: number,
): number | undefined {
  const seqRange = getNodeRange(seq);
  if (!seqRange) {
    return undefined;
  }
  const [seqStart, , seqNodeEnd] = seqRange;
  const seqEnd = seqNodeEnd ?? seqRange[1];
  if (offset < seqStart || offset >= seqEnd) {
    // Offset is outside the whole sequence — not our responsibility.
    return undefined;
  }

  const items = seq.items;
  if (items.length === 0) {
    return undefined;
  }

  // For each item, claim the half-open range [item.start, nextItem.start)
  // so lines that belong to item i (like its request/assert/capture
  // sub-blocks) resolve to i even though nodeEnd may report an earlier
  // offset. The final item claims everything up to the sequence end.
  for (let i = 0; i < items.length; i++) {
    const range = getNodeRange(items[i]);
    if (!range) continue;
    const [start] = range;
    const next = items[i + 1];
    const nextRange = next ? getNodeRange(next) : undefined;
    const effectiveEnd = nextRange ? nextRange[0] : seqEnd;
    if (offset >= start && offset < effectiveEnd) {
      return i;
    }
    if (i === 0 && offset < start) {
      return undefined;
    }
  }
  return undefined;
}

function collectStepSequence(
  parent: YAMLMapType,
  key: string,
  phase: "setup" | "test",
  testName: string | undefined,
): VisibleCapture[] {
  const value = getMapValue(parent, key);
  if (!isSeq(value)) {
    return [];
  }
  const captures: VisibleCapture[] = [];
  value.items.forEach((item, index) => {
    if (!isMap(item)) return;
    const stepName = getScalarString(item, "name") ?? `step ${index + 1}`;
    const captureBlock = getMapValue(item, "capture");
    if (!isMap(captureBlock)) return;
    for (const pair of captureBlock.items) {
      if (!isScalar(pair.key)) continue;
      const name = String(pair.key.value);
      captures.push({
        name,
        stepIndex: index,
        phase,
        testName,
        stepName,
      });
    }
  });
  return captures;
}

function getMapValue(map: YAMLMapType, key: string): unknown {
  for (const pair of map.items) {
    if (isScalar(pair.key) && pair.key.value === key) {
      return pair.value;
    }
  }
  return undefined;
}

function getScalarString(map: YAMLMapType, key: string): string | undefined {
  const value = getMapValue(map, key);
  if (isScalar(value)) {
    return typeof value.value === "string" ? value.value : String(value.value);
  }
  return undefined;
}

function getNodeRange(node: unknown): [number, number, number | undefined] | undefined {
  if (!node || typeof node !== "object") return undefined;
  const r = (node as { range?: [number, number, number] }).range;
  if (!r) return undefined;
  return [r[0], r[1], r[2]];
}

// ---------------------------------------------------------------------------
// Capture symbol index (definition / references / rename)
// ---------------------------------------------------------------------------

/**
 * A single `capture: { name: ... }` declaration in a test file. Byte
 * offsets point at the scalar key node so providers can turn them into
 * VS Code ranges via `document.positionAt`.
 */
export interface CaptureDeclaration {
  name: string;
  phase: "setup" | "test" | "teardown";
  testName?: string;
  stepIndex: number;
  stepName: string;
  /** Byte offset range of the capture key node (the `name:` scalar). */
  keyStart: number;
  keyEnd: number;
}

/**
 * Searchable index of every capture declaration in a file. Returned by
 * `buildCaptureIndex` and consumed by definition / references / rename
 * providers.
 */
export interface CaptureIndex {
  readonly declarations: readonly CaptureDeclaration[];
  findDeclarationAt(offset: number): CaptureDeclaration | undefined;
  findByName(name: string): CaptureDeclaration[];
}

/**
 * Walk a Tarn YAML file's CST and collect every capture declaration
 * (from setup, flat steps, named tests, and teardown). Returns
 * `undefined` when the document has parse errors so callers can bail
 * out before offering a broken rename.
 */
export function buildCaptureIndex(source: string): CaptureIndex | undefined {
  let doc: YAMLDocument.Parsed;
  try {
    doc = parseDocument(source, { keepSourceTokens: false });
  } catch {
    return undefined;
  }
  if (doc.errors.length > 0) {
    return undefined;
  }
  const root = doc.contents;
  if (!isMap(root)) {
    return undefined;
  }

  const declarations: CaptureDeclaration[] = [];

  const collectStepCaptures = (
    stepsNode: unknown,
    phase: "setup" | "test" | "teardown",
    testName: string | undefined,
  ): void => {
    if (!isSeq(stepsNode)) return;
    stepsNode.items.forEach((item, index) => {
      if (!isMap(item)) return;
      const stepName = getScalarString(item, "name") ?? `step ${index + 1}`;
      const captureBlock = getMapValue(item, "capture");
      if (!isMap(captureBlock)) return;
      for (const pair of captureBlock.items) {
        if (!isScalar(pair.key)) continue;
        const name = String(pair.key.value);
        const range = getNodeRange(pair.key);
        if (!range) continue;
        declarations.push({
          name,
          phase,
          testName,
          stepIndex: index,
          stepName,
          keyStart: range[0],
          keyEnd: range[1],
        });
      }
    });
  };

  collectStepCaptures(getMapValue(root, "setup"), "setup", undefined);
  collectStepCaptures(getMapValue(root, "steps"), "test", undefined);

  const testsNode = getMapValue(root, "tests");
  if (isMap(testsNode)) {
    for (const pair of testsNode.items) {
      if (!isScalar(pair.key)) continue;
      const testName = String(pair.key.value);
      const value = pair.value;
      if (!isMap(value)) continue;
      collectStepCaptures(getMapValue(value, "steps"), "test", testName);
    }
  }

  collectStepCaptures(getMapValue(root, "teardown"), "teardown", undefined);

  return {
    declarations,
    findDeclarationAt(offset: number): CaptureDeclaration | undefined {
      return declarations.find(
        (d) => offset >= d.keyStart && offset < d.keyEnd,
      );
    },
    findByName(name: string): CaptureDeclaration[] {
      return declarations.filter((d) => d.name === name);
    },
  };
}

/**
 * Regex-scan a document for every `{{ capture.NAME }}` reference.
 * Returns byte-offset ranges for the identifier portion only (not the
 * `{{` / `}}` punctuation) so providers can highlight the renameable
 * word precisely. `nameFilter` narrows results to a single name.
 */
export interface CaptureReference {
  name: string;
  nameStart: number;
  nameEnd: number;
}

export function findCaptureReferences(
  source: string,
  nameFilter?: string,
): CaptureReference[] {
  const pattern = /\{\{\s*capture\.([A-Za-z_][A-Za-z0-9_]*)\s*\}\}/g;
  const results: CaptureReference[] = [];
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(source)) !== null) {
    const fullMatch = match[0];
    const name = match[1];
    if (nameFilter && nameFilter !== name) {
      continue;
    }
    // Offset of the identifier inside the match: skip the `{{`, any
    // whitespace, and the `capture.` prefix.
    const identifierOffsetInMatch = fullMatch.indexOf(name);
    const nameStart = match.index + identifierOffsetInMatch;
    const nameEnd = nameStart + name.length;
    results.push({ name, nameStart, nameEnd });
  }
  return results;
}
