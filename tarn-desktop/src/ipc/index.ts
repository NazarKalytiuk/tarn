import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DiscoverResult,
  Environment,
  FileFinishedEvent,
  FileStartedEvent,
  RunDoneEvent,
  RunReport,
  RunReportReadyEvent,
  RunSelector,
  RunStartedEvent,
  RunStartedResponse,
  SidecarLogEvent,
  StepFinishedEvent,
  TestFinishedEvent,
} from "./types";

export async function discoverTests(project: string): Promise<DiscoverResult> {
  return invoke<DiscoverResult>("discover_tests", { project });
}

export async function runTests(
  project: string,
  selector: RunSelector,
): Promise<RunStartedResponse> {
  return invoke<RunStartedResponse>("run_tests", { project, selector });
}

export async function cancelRun(runId: string): Promise<void> {
  await invoke("cancel_run", { runId });
}

export async function getRunReport(project: string): Promise<RunReport | null> {
  return invoke<RunReport | null>("get_run_report", { project });
}

export async function listEnvironments(project: string): Promise<Environment[]> {
  return invoke<Environment[]>("list_environments", { project });
}

type RunEventHandlers = {
  onRunStarted?: (e: RunStartedEvent) => void;
  onFileStarted?: (e: FileStartedEvent) => void;
  onStepFinished?: (e: StepFinishedEvent) => void;
  onTestFinished?: (e: TestFinishedEvent) => void;
  onFileFinished?: (e: FileFinishedEvent) => void;
  onRunDone?: (e: RunDoneEvent) => void;
  onRunReportReady?: (e: RunReportReadyEvent) => void;
  onSidecarLog?: (e: SidecarLogEvent) => void;
};

/**
 * Subscribe to all Tauri run events. Returns a cleanup function that
 * detaches every listener — call it in `onCleanup` of a Solid root.
 */
export async function listenToRunEvents(
  handlers: RunEventHandlers,
): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];

  const pairs: [keyof RunEventHandlers, string][] = [
    ["onRunStarted", "test:run-started"],
    ["onFileStarted", "test:file-started"],
    ["onStepFinished", "test:step-finished"],
    ["onTestFinished", "test:test-finished"],
    ["onFileFinished", "test:file-finished"],
    ["onRunDone", "test:run-done"],
    ["onRunReportReady", "test:run-report-ready"],
    ["onSidecarLog", "sidecar:log"],
  ];

  for (const [key, topic] of pairs) {
    const handler = handlers[key];
    if (!handler) continue;
    const fn = handler as (payload: unknown) => void;
    const un = await listen(topic, (e) => fn(e.payload));
    unlisteners.push(un);
  }

  return () => {
    unlisteners.forEach((u) => u());
  };
}
