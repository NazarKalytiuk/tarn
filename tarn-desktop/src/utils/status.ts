import type { ListedFile, ListedTest } from "../ipc/types";
import { runState, stepKey, type StepStatus } from "../stores/run";

const RANK: Record<StepStatus, number> = {
  failed: 5,
  running: 4,
  pending: 3,
  skipped: 2,
  passed: 1,
};

export function stepStatus(file: string, test: string | null, idx: number): StepStatus {
  return (runState.steps[stepKey(file, test, idx)]?.status ?? "pending") as StepStatus;
}

export function testStatus(file: string, test: ListedTest): StepStatus {
  let best: StepStatus = "pending";
  let bestRank = -1;
  for (let i = 0; i < test.steps.length; i++) {
    const s = stepStatus(file, test.name, i);
    if (RANK[s] > bestRank) {
      best = s;
      bestRank = RANK[s];
    }
  }
  return best;
}

export function fileStatus(file: ListedFile): StepStatus {
  let best: StepStatus = "pending";
  let bestRank = -1;
  const consider = (s: StepStatus) => {
    if (RANK[s] > bestRank) {
      best = s;
      bestRank = RANK[s];
    }
  };
  for (const t of file.tests) consider(testStatus(file.file, t));
  for (let i = 0; i < file.steps.length; i++) consider(stepStatus(file.file, null, i));
  return best;
}

export function filePassedCount(file: ListedFile): { passed: number; total: number } {
  let passed = 0;
  let total = 0;
  for (const t of file.tests) {
    for (let i = 0; i < t.steps.length; i++) {
      total += 1;
      if (stepStatus(file.file, t.name, i) === "passed") passed += 1;
    }
  }
  for (let i = 0; i < file.steps.length; i++) {
    total += 1;
    if (stepStatus(file.file, null, i) === "passed") passed += 1;
  }
  return { passed, total };
}

export function testPassedCount(file: string, test: ListedTest): { passed: number; total: number } {
  let passed = 0;
  for (let i = 0; i < test.steps.length; i++) {
    if (stepStatus(file, test.name, i) === "passed") passed += 1;
  }
  return { passed, total: test.steps.length };
}

export function isStepRunning(file: string, test: string | null, idx: number): boolean {
  return stepStatus(file, test, idx) === "running";
}
