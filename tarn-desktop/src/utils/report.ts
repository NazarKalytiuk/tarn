import type { ReportStep, RunReport } from "../ipc/types";

/**
 * Resolve a step key (file/test/index) against a parsed run report.
 * Mirrors the path the tree uses: a non-empty `test` selects from the
 * test's steps, an empty/null `test` selects from the file's flat
 * steps. Returns `null` if anything along the path is missing — the
 * caller renders a "no data" affordance in that case.
 */
export function findReportStep(
  report: RunReport | null,
  file: string,
  test: string | null | undefined,
  stepIndex: number,
): ReportStep | null {
  if (!report) return null;
  const reportFile = report.files.find((f) => f.file === file);
  if (!reportFile) return null;
  if (test && test !== "") {
    const reportTest = reportFile.tests.find((t) => t.name === test);
    return reportTest?.steps?.[stepIndex] ?? null;
  }
  return reportFile.steps?.[stepIndex] ?? null;
}
