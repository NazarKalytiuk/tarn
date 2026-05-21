// Shape mirrors the Rust types in `src-tauri/src/ipc.rs` and
// `src-tauri/src/sidecar/event.rs`. Keep in sync — schema changes start
// on the Rust side.

export interface ListedStep {
  kind: string;
  name: string;
}

export interface ListedTest {
  name: string;
  steps: ListedStep[];
  tags?: string[];
}

export interface ListedFile {
  file: string;
  name: string;
  setup: ListedStep[];
  steps: ListedStep[];
  tests: ListedTest[];
  teardown: ListedStep[];
  tags: string[];
}

export interface DiscoverResult {
  files: ListedFile[];
}

export interface RunSelector {
  selectors: string[];
  env?: string;
  tag?: string;
  vars: { key: string; value: string }[];
}

export interface RunStartedResponse {
  run_id: string;
}

export interface AssertionFailure {
  actual?: string;
  assertion?: string;
  expected?: string;
  message?: string;
}

export interface Progress {
  index: number;
  total: number;
}

export interface FileStartedEvent {
  run_id: string;
  event: "file_started";
  file: string;
  file_name?: string;
}

export interface StepFinishedEvent {
  run_id: string;
  event: "step_finished";
  file: string;
  test?: string;
  step: string;
  step_index: number;
  status: string;
  duration_ms: number;
  phase?: string;
  progress?: Progress;
  assertion_failures: AssertionFailure[];
  error_code?: string;
  failure_category?: string;
}

export interface TestFinishedEvent {
  run_id: string;
  event: "test_finished";
  file: string;
  test: string;
  status: string;
  duration_ms: number;
  steps: { total: number; passed: number; failed: number };
}

export interface FileFinishedEvent {
  run_id: string;
  event: "file_finished";
  file: string;
  file_name?: string;
  status: string;
  duration_ms: number;
  summary: unknown;
}

export interface RunDoneEvent {
  run_id: string;
  event: "done";
  duration_ms: number;
  summary: unknown;
}

export interface RunReportReadyEvent {
  run_id: string;
  report: RunReport;
}

export interface SidecarLogEvent {
  run_id: string;
  level: "info" | "warn" | "error";
  message: string;
}

export interface RunStartedEvent {
  run_id: string;
  project: string;
}

export interface Environment {
  name: string;
  file: string;
  is_base: boolean;
}

// ---------------------------------------------------------------------
// .tarn/last-run.json report shape (the JSON artifact, NOT the NDJSON
// stream). Only the subset Studio consumes is declared. Extra keys are
// preserved by `Record<string, unknown>` typings on container nodes.
// ---------------------------------------------------------------------

export interface RunReport {
  args?: string[];
  duration_ms?: number;
  env_name?: string | null;
  files: ReportFile[];
  summary?: ReportSummary;
}

export interface ReportFile {
  file: string;
  name: string;
  status: string;
  duration_ms: number;
  setup: ReportStep[];
  steps: ReportStep[];
  teardown: ReportStep[];
  tests: ReportTest[];
  summary?: ReportSummary;
}

export interface ReportTest {
  name: string;
  description?: string;
  duration_ms: number;
  status: string;
  steps: ReportStep[];
}

export interface ReportStep {
  name: string;
  status?: string;
  duration_ms?: number;
  error_code?: string;
  failure_category?: string;
  location?: ReportLocation;
  remediation_hints?: string[];
  assertions?: ReportAssertions;
  request?: ReportRequest;
  response?: ReportResponse;
  captures?: Record<string, unknown>;
  progress?: Progress;
}

export interface ReportLocation {
  file: string;
  line: number;
  column: number;
}

export interface ReportAssertions {
  total: number;
  passed: number;
  failed: number;
  details?: ReportAssertionDetail[];
  failures?: ReportAssertionDetail[];
}

export interface ReportAssertionDetail {
  assertion: string;
  expected?: string;
  actual?: string;
  message?: string;
  diff?: unknown;
  passed?: boolean;
  location?: ReportLocation;
}

export interface ReportRequest {
  method?: string;
  url?: string;
  headers?: Record<string, string>;
  body?: unknown;
}

export interface ReportResponse {
  status?: number;
  headers?: Record<string, string>;
  body?: unknown;
}

export interface ReportSummary {
  total: number;
  passed: number;
  failed: number;
}
