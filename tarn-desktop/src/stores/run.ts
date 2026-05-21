import { createStore } from "solid-js/store";
import type {
  AssertionFailure,
  RunReport,
  StepFinishedEvent,
} from "../ipc/types";

export type StepStatus =
  | "pending"
  | "running"
  | "passed"
  | "failed"
  | "skipped";

/** Per-step state mirrored from the NDJSON stream. Keyed by
 * `${file}::${test ?? ""}::${stepIndex}` so we can address both
 * test-scoped and flat-steps forms. */
export type StepState = {
  status: StepStatus;
  durationMs?: number;
  phase?: string;
  errorCode?: string;
  failureCategory?: string;
  assertionFailures: AssertionFailure[];
};

export type FileRunState = {
  status: StepStatus;
  durationMs?: number;
};

export type RunState = {
  runId: string | null;
  startedAt: number | null;
  finishedAt: number | null;
  status: "idle" | "running" | "passed" | "failed";
  /** key: `${file}::${test ?? ""}::${stepIndex}` */
  steps: Record<string, StepState>;
  files: Record<string, FileRunState>;
  /** Stream of stderr lines from the sidecar. Capped to last 200. */
  log: string[];
  /** The most recent `.tarn/last-run.json` payload, populated after
   *  the `test:run-report-ready` event fires. `null` while running or
   *  before the first run completes. */
  report: RunReport | null;
};

const initialRun = (): RunState => ({
  runId: null,
  startedAt: null,
  finishedAt: null,
  status: "idle",
  steps: {},
  files: {},
  log: [],
  report: null,
});

export const [runState, setRunState] = createStore<RunState>(initialRun());

export function stepKey(file: string, test: string | undefined | null, stepIndex: number): string {
  return `${file}::${test ?? ""}::${stepIndex}`;
}

/**
 * Reset state in preparation for a run. When `scope` is `"all"`, the
 * entire steps + files maps and the report are cleared. When `scope`
 * is an array of step keys, only those keys are reset to `pending`
 * (along with the files they belong to), so a partial run — like
 * "Re-run failures" — preserves the green statuses of every other
 * step from the previous run.
 */
export function resetStepsForRun(scope: "all" | string[]) {
  if (scope === "all") {
    setRunState({
      steps: {},
      files: {},
      report: null,
      startedAt: null,
      finishedAt: null,
      status: "idle",
    });
    return;
  }
  const affectedFiles = new Set<string>();
  for (const key of scope) {
    affectedFiles.add(key.split("::", 1)[0]);
    setRunState("steps", key, {
      status: "pending",
      durationMs: undefined,
      phase: undefined,
      errorCode: undefined,
      failureCategory: undefined,
      assertionFailures: [],
    });
  }
  for (const file of affectedFiles) {
    setRunState("files", file, { status: "pending", durationMs: undefined });
  }
}

export function markRunStarted(runId: string) {
  setRunState({
    runId,
    startedAt: Date.now(),
    finishedAt: null,
    status: "running",
    log: [],
  });
}

export function markFileStarted(file: string) {
  setRunState("files", file, { status: "running" });
}

export function markStepFinished(e: StepFinishedEvent) {
  const key = stepKey(e.file, e.test, e.step_index);
  const status: StepStatus = normalizeStatus(e.status);
  setRunState("steps", key, {
    status,
    durationMs: e.duration_ms,
    phase: e.phase,
    errorCode: e.error_code,
    failureCategory: e.failure_category,
    assertionFailures: e.assertion_failures,
  });
}

export function markFileFinished(file: string, status: string, durationMs: number) {
  setRunState("files", file, { status: normalizeStatus(status), durationMs });
}

export function finishRun(status: "passed" | "failed") {
  setRunState({
    status,
    finishedAt: Date.now(),
  });
}

export function setReport(report: RunReport) {
  setRunState("report", report);
}

export function appendLog(line: string) {
  setRunState("log", (lines) => {
    const next = [...lines, line];
    return next.length > 200 ? next.slice(next.length - 200) : next;
  });
}

function normalizeStatus(raw: string): StepStatus {
  const lower = raw.toLowerCase();
  if (lower.includes("pass")) return "passed";
  if (lower.includes("fail")) return "failed";
  if (lower.includes("skip")) return "skipped";
  if (lower.includes("run")) return "running";
  return "pending";
}
