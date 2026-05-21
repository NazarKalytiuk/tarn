import type { ListedFile, ListedStep, ListedTest } from "../ipc/types";
import { stepKey } from "../stores/run";

export type FlatStep = {
  key: string;
  file: ListedFile;
  test: ListedTest | null;
  step: ListedStep;
  stepIndex: number;
};

/**
 * Flatten the discovery tree into one list of steps with refs back to
 * the file and test they live under. Used by:
 *   - the run dashboard's per-step completion strip,
 *   - the "currently running" card,
 *   - the recent-failures list,
 *   - the "re-run failures" selector.
 */
export function flatSteps(files: ListedFile[]): FlatStep[] {
  const out: FlatStep[] = [];
  for (const file of files) {
    for (const test of file.tests) {
      test.steps.forEach((step, idx) => {
        out.push({
          key: stepKey(file.file, test.name, idx),
          file,
          test,
          step,
          stepIndex: idx,
        });
      });
    }
    file.steps.forEach((step, idx) => {
      out.push({
        key: stepKey(file.file, null, idx),
        file,
        test: null,
        step,
        stepIndex: idx,
      });
    });
  }
  return out;
}
