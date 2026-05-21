import { createSignal, onCleanup, onMount, Show } from "solid-js";
import BinaryError from "./components/BinaryError";
import CatalogueTree, { type StepSelection } from "./components/CatalogueTree";
import EmptyState from "./components/EmptyState";
import RunDashboard from "./components/RunDashboard";
import StepDetail from "./components/StepDetail";
import Statusbar from "./components/Statusbar";
import Titlebar from "./components/Titlebar";
import Toolbar from "./components/Toolbar";
import { openProjectAtPath, pickProject } from "./components/ProjectPicker";
import { cancelRun, listenToRunEvents, runTests } from "./ipc";
import { projectState } from "./stores/project";
import { loadRecentProjects } from "./stores/recent";
import {
  appendLog,
  finishRun,
  markFileFinished,
  markFileStarted,
  markRunStarted,
  markStepFinished,
  resetStepsForRun,
  runState,
  setReport,
} from "./stores/run";
import { toolbarState } from "./stores/toolbar";
import { flatSteps } from "./utils/flatSteps";

export default function App() {
  const [selection, setSelection] = createSignal<StepSelection | null>(null);
  const [focusFailures, setFocusFailures] = createSignal(false);

  let unlisten: (() => void) | undefined;

  onMount(async () => {
    await loadRecentProjects();
    unlisten = await listenToRunEvents({
      onRunStarted: (e) => markRunStarted(e.run_id),
      onFileStarted: (e) => markFileStarted(e.file),
      onStepFinished: (e) => markStepFinished(e),
      onFileFinished: (e) => markFileFinished(e.file, e.status, e.duration_ms),
      onRunDone: (e) => {
        const summary = (e.summary ?? {}) as { status?: string };
        finishRun(summary.status?.toLowerCase() === "passed" ? "passed" : "failed");
      },
      onRunReportReady: (e) => setReport(e.report),
      onSidecarLog: (e) => appendLog(`[${e.level}] ${e.message}`),
    });
    window.addEventListener("keydown", onKeyDown);
  });

  onCleanup(() => {
    unlisten?.();
    window.removeEventListener("keydown", onKeyDown);
  });

  function onKeyDown(e: KeyboardEvent) {
    const cmd = e.metaKey || e.ctrlKey;
    if (!cmd) return;
    if (e.key.toLowerCase() === "o") {
      e.preventDefault();
      void pickProject();
    } else if (e.key.toLowerCase() === "r" && e.shiftKey) {
      e.preventDefault();
      reRunSelectedStep();
    } else if (e.key.toLowerCase() === "r") {
      e.preventDefault();
      void spawnRun([]);
    } else if (e.key === ".") {
      e.preventDefault();
      void onCancel();
    }
  }

  const busy = () => runState.status === "running";

  /**
   * Expand selector strings into the concrete set of step keys they
   * cover. An empty selector list means "every step in the project".
   * Used so a subset run only resets the affected steps' statuses,
   * preserving the green dots of everything else from the previous run.
   */
  function expandSelectors(selectors: string[]): string[] {
    const all = flatSteps(projectState.files);
    if (selectors.length === 0) return all.map((fs) => fs.key);
    const out = new Set<string>();
    for (const sel of selectors) {
      const parts = sel.split("::");
      const file = parts[0];
      const test = parts.length > 1 ? parts[1] : null;
      const idx = parts.length > 2 ? Number(parts[2]) : null;
      for (const fs of all) {
        if (fs.file.file !== file) continue;
        if (test != null && (fs.test?.name ?? "") !== test) continue;
        if (idx != null && fs.stepIndex !== idx) continue;
        out.add(fs.key);
      }
    }
    return Array.from(out);
  }

  async function spawnRun(selectors: string[]) {
    if (!projectState.path) return;
    const scopeKeys = expandSelectors(selectors);
    resetStepsForRun(selectors.length === 0 ? "all" : scopeKeys);
    try {
      const tag =
        toolbarState.activeTags.length > 0
          ? toolbarState.activeTags.join(",")
          : undefined;
      await runTests(projectState.path, {
        selectors,
        env: toolbarState.activeEnv ?? undefined,
        tag,
        vars: [],
      });
    } catch (err) {
      appendLog(`[error] ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  function buildSelector(file: string, test?: string | null, stepIndex?: number) {
    let s = file;
    if (test != null && test !== "") s += `::${test}`;
    if (stepIndex != null) s += `::${stepIndex}`;
    return s;
  }

  function reRunSelectedStep() {
    const sel = selection();
    if (!sel) return;
    void spawnRun([buildSelector(sel.file, sel.test, sel.stepIndex)]);
  }

  function onRunFailures() {
    const selectors: string[] = [];
    for (const [key, state] of Object.entries(runState.steps)) {
      if (state.status !== "failed") continue;
      const parts = key.split("::");
      if (parts.length < 3) continue;
      const file = parts[0];
      const test = parts[1] || null;
      const idx = Number(parts[2]);
      if (Number.isFinite(idx)) {
        selectors.push(buildSelector(file, test, idx));
      }
    }
    if (selectors.length > 0) void spawnRun(selectors);
  }

  async function onCancel() {
    if (runState.runId) await cancelRun(runState.runId);
  }

  const hasError = () => projectState.error != null;

  return (
    <div
      class="tarn-window"
      style={{
        width: "100%",
        height: "100%",
        display: "flex",
        "flex-direction": "column",
        position: "relative",
      }}
    >
      <Titlebar onOpenProject={() => void pickProject()} />

      <Show when={projectState.path && !hasError()}>
        <Toolbar
          busy={busy()}
          focusFailures={focusFailures()}
          onRunAll={() => void spawnRun([])}
          onRunFailures={onRunFailures}
          onCancel={onCancel}
          onToggleFocus={() => setFocusFailures(!focusFailures())}
        />
      </Show>

      <div
        style={{
          flex: 1,
          display: "flex",
          "min-height": 0,
          background: "var(--bg-base)",
        }}
      >
        <Show
          when={projectState.path && !hasError()}
          fallback={
            <Show
              when={hasError()}
              fallback={
                <EmptyState
                  onOpen={() => void pickProject()}
                  onOpenPath={(p) => void openProjectAtPath(p)}
                />
              }
            >
              <BinaryError message={projectState.error ?? undefined} />
            </Show>
          }
        >
          <aside
            style={{
              width: "304px",
              "flex-shrink": 0,
              "border-right": "1px solid var(--line)",
              display: "flex",
              "flex-direction": "column",
              background: "var(--bg-base)",
            }}
          >
            <CatalogueTree
              selectedKey={selection()?.key ?? null}
              focusFailures={focusFailures()}
              busy={busy()}
              onSelect={setSelection}
              onRunFile={(file) => void spawnRun([buildSelector(file)])}
              onRunTest={(file, test) =>
                void spawnRun([buildSelector(file, test)])
              }
              onRunStep={(file, test, idx) =>
                void spawnRun([buildSelector(file, test, idx)])
              }
            />
          </aside>
          <section
            style={{
              flex: 1,
              display: "flex",
              "flex-direction": "column",
              "min-width": 0,
            }}
          >
            <Show
              when={selection()}
              fallback={
                <RunDashboard
                  onSelectStep={setSelection}
                  onRunAll={() => void spawnRun([])}
                />
              }
            >
              <StepDetail
                selectedKey={selection()!.key}
                selectedFile={selection()!.file}
                selectedTest={selection()!.test}
                selectedStepIndex={selection()!.stepIndex}
                selectedLabel={selection()!.label}
                onReRun={reRunSelectedStep}
              />
            </Show>
          </section>
        </Show>
      </div>

      <Show when={projectState.path && !hasError()}>
        <Statusbar />
      </Show>
    </div>
  );
}

