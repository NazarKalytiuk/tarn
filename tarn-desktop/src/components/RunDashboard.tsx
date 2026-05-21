import { createMemo, For, Show } from "solid-js";
import { projectState } from "../stores/project";
import { runState, type StepStatus } from "../stores/run";
import { toolbarState } from "../stores/toolbar";
import StatusDot from "./atoms/StatusDot";
import { KBD, MethodTag, SubsectionLabel } from "./atoms/misc";
import { flatSteps } from "../utils/flatSteps";
import { parseStepName } from "../utils/stepName";
import { findReportStep } from "../utils/report";
import { relativePath } from "../utils/relativePath";

type StepSelector = {
  key: string;
  file: string;
  test: string | null;
  stepIndex: number;
  label: string;
};

type Props = {
  onSelectStep: (sel: StepSelector) => void;
  onRunAll: () => void;
};

export default function RunDashboard(props: Props) {
  const all = createMemo(() => flatSteps(projectState.files));

  const counts = createMemo(() => {
    const c = { passed: 0, failed: 0, running: 0, skipped: 0, pending: 0, total: all().length };
    for (const fs of all()) {
      const s = (runState.steps[fs.key]?.status ?? "pending") as StepStatus;
      c[s] += 1;
    }
    return c;
  });

  const running = () => runState.status === "running";
  const hasRun = () => runState.status !== "idle" || runState.report !== null;
  const cancelled = () => runState.status === "passed" || runState.status === "failed"
    ? false
    : false; // cancellation flag isn't tracked separately; left for future
  const duration = () => {
    if (runState.startedAt == null) return null;
    const end = runState.finishedAt ?? Date.now();
    return end - runState.startedAt;
  };

  const currentRunning = createMemo(() => {
    for (const fs of all()) {
      if (runState.steps[fs.key]?.status === "running") return fs;
    }
    return null;
  });

  const recentFailures = createMemo(() => {
    const out: { fs: ReturnType<typeof flatSteps>[number]; message: string }[] = [];
    for (const fs of all()) {
      const s = runState.steps[fs.key];
      if (s?.status === "failed") {
        const msg =
          s.assertionFailures[0]?.message ??
          findReportStep(runState.report, fs.file.file, fs.test?.name ?? null, fs.stepIndex)
            ?.assertions?.failures?.[0]?.message ??
          "Assertion failed";
        out.push({ fs, message: msg });
      }
    }
    return out.slice(0, 5);
  });

  const done = () => counts().passed + counts().failed + counts().skipped;
  const pct = () =>
    counts().total > 0 ? Math.round((done() / counts().total) * 100) : 0;

  const heroTitle = () => {
    if (running()) return "Running";
    if (cancelled()) return "Cancelled";
    if (hasRun()) return "Last run";
    return "Catalogue";
  };

  return (
    <div
      class="scroll"
      style={{
        flex: 1,
        padding: "40px 36px 24px",
        "min-height": 0,
        overflow: "auto",
      }}
    >
      <div
        style={{
          display: "flex",
          "align-items": "baseline",
          gap: "12px",
          "margin-bottom": "4px",
        }}
      >
        <SubsectionLabel>{heroTitle()}</SubsectionLabel>
        <Show when={running() && currentRunning()}>
          <span style={{ "font-size": "12px", color: "var(--fg-secondary)" }}>
            <span style={{ "font-family": "var(--font-mono)" }}>
              {relativePath(projectState.path, currentRunning()!.file.file)}
            </span>
          </span>
        </Show>
      </div>

      <div
        style={{
          display: "flex",
          "align-items": "baseline",
          gap: "22px",
          "margin-top": "4px",
          "margin-bottom": "28px",
          "flex-wrap": "wrap",
        }}
      >
        <Counter label="passed" value={counts().passed} color="var(--passed)" hero />
        <Counter
          label="failed"
          value={counts().failed}
          color={counts().failed > 0 ? "var(--failed)" : "var(--fg-quaternary)"}
          hero
        />
        <Show when={running()}>
          <Counter
            label="running"
            value={counts().running}
            color="var(--running)"
            hero
            pulse
          />
        </Show>
        <Counter label="pending" value={counts().pending} color="var(--fg-tertiary)" hero subdued />
        <Show when={counts().skipped > 0}>
          <Counter
            label="skipped"
            value={counts().skipped}
            color="var(--skipped)"
            hero
            subdued
          />
        </Show>
        <div style={{ flex: 1 }} />
        <Show when={duration() != null}>
          <div style={{ "text-align": "right" }}>
            <div
              style={{
                "font-family": "var(--font-mono)",
                "font-size": "28px",
                color: "var(--fg-primary)",
                "font-variant-numeric": "tabular-nums",
                "letter-spacing": "-0.02em",
              }}
            >
              {fmtDuration(duration()!)}
            </div>
            <div
              style={{
                "font-size": "10.5px",
                "font-weight": 600,
                "letter-spacing": "0.08em",
                "text-transform": "uppercase",
                color: "var(--fg-tertiary)",
              }}
            >
              {running() ? "elapsed" : "duration"}
            </div>
          </div>
        </Show>
      </div>

      <Show when={running() || hasRun()}>
        <div style={{ "margin-bottom": "28px" }}>
          <div
            style={{
              display: "flex",
              "align-items": "center",
              gap: "2px",
              height: "4px",
              "border-radius": "2px",
              overflow: "hidden",
              background: "var(--bg-input)",
            }}
          >
            <For each={all()}>
              {(fs) => {
                const st = (): StepStatus =>
                  (runState.steps[fs.key]?.status ?? "pending") as StepStatus;
                return (
                  <span
                    class={st() === "running" ? "tarn-pulse" : ""}
                    style={{
                      flex: 1,
                      background: stripColor(st()),
                      "min-width": "4px",
                    }}
                  />
                );
              }}
            </For>
          </div>
          <div
            style={{
              display: "flex",
              "justify-content": "space-between",
              "font-size": "11px",
              color: "var(--fg-tertiary)",
              "font-family": "var(--font-mono)",
              "margin-top": "6px",
              "font-variant-numeric": "tabular-nums",
            }}
          >
            <span>{done()} of {counts().total} steps</span>
            <span>{pct()}%</span>
          </div>
        </div>
      </Show>

      <Show when={running() && currentRunning()}>
        <div style={{ "margin-bottom": "28px" }}>
          <SubsectionLabel>Currently running</SubsectionLabel>
          <div
            style={{
              display: "flex",
              "align-items": "center",
              gap: "12px",
              padding: "12px 14px",
              background: "var(--bg-surface)",
              border: "1px solid var(--line)",
              "border-radius": "var(--r-lg)",
              "margin-top": "8px",
            }}
          >
            <StatusDot status="running" size={10} />
            <For
              each={[currentRunning()!]}
              children={(fs) => {
                const parsed = parseStepName(fs.step.name);
                return (
                  <>
                    <Show when={parsed.method}>
                      <MethodTag method={parsed.method!} />
                    </Show>
                    <span
                      style={{
                        "font-family": "var(--font-mono)",
                        "font-size": "13px",
                        color: "var(--fg-primary)",
                      }}
                    >
                      {parsed.path}
                    </span>
                    <span style={{ color: "var(--fg-quaternary)" }}>·</span>
                    <span style={{ "font-size": "12px", color: "var(--fg-tertiary)" }}>
                      in <span style={{ "font-family": "var(--font-mono)" }}>
                        {fs.test?.name ?? fs.file.name}
                      </span>
                    </span>
                  </>
                );
              }}
            />
            <div style={{ flex: 1 }} />
            <span
              class="tarn-pulse"
              style={{
                "font-family": "var(--font-mono)",
                "font-size": "11.5px",
                color: "var(--running)",
                "letter-spacing": "0.04em",
              }}
            >
              waiting on response…
            </span>
          </div>
        </div>
      </Show>

      <Show when={recentFailures().length > 0}>
        <div style={{ "margin-bottom": "28px" }}>
          <SubsectionLabel>{running() ? "Failures so far" : "Failures"}</SubsectionLabel>
          <div style={{ display: "flex", "flex-direction": "column", gap: 0, "margin-top": "8px" }}>
            <For each={recentFailures()}>
              {(item, i) => {
                const parsed = parseStepName(item.fs.step.name);
                const last = i() === recentFailures().length - 1;
                return (
                  <div
                    onClick={() =>
                      props.onSelectStep({
                        key: item.fs.key,
                        file: item.fs.file.file,
                        test: item.fs.test?.name ?? null,
                        stepIndex: item.fs.stepIndex,
                        label: item.fs.step.name,
                      })
                    }
                    style={{
                      display: "flex",
                      "align-items": "center",
                      gap: "10px",
                      padding: "10px 14px",
                      background: i() % 2 === 0 ? "var(--bg-surface)" : "var(--bg-base)",
                      border: "1px solid var(--line)",
                      "border-top-left-radius": i() === 0 ? "var(--r-lg)" : 0,
                      "border-top-right-radius": i() === 0 ? "var(--r-lg)" : 0,
                      "border-bottom-left-radius": last ? "var(--r-lg)" : 0,
                      "border-bottom-right-radius": last ? "var(--r-lg)" : 0,
                      "border-top": i() ? "none" : "1px solid var(--line)",
                      cursor: "pointer",
                    }}
                  >
                    <StatusDot status="failed" size={9} />
                    <Show when={parsed.method}>
                      <MethodTag method={parsed.method!} />
                    </Show>
                    <span
                      style={{
                        "font-family": "var(--font-mono)",
                        "font-size": "12.5px",
                        color: "var(--fg-primary)",
                      }}
                    >
                      {parsed.path}
                    </span>
                    <span style={{ color: "var(--fg-quaternary)" }}>·</span>
                    <span style={{ "font-size": "12px", color: "var(--fg-tertiary)" }}>
                      {item.fs.test?.name ?? item.fs.file.name}
                      <span style={{ color: "var(--fg-quaternary)" }}> in </span>
                      <span style={{ "font-family": "var(--font-mono)" }}>
                        {relativePath(projectState.path, item.fs.file.file)}
                      </span>
                    </span>
                    <div style={{ flex: 1 }} />
                    <span
                      style={{
                        "font-family": "var(--font-mono)",
                        "font-size": "11.5px",
                        color: "var(--fg-tertiary)",
                      }}
                    >
                      {item.message}
                    </span>
                    <svg
                      width="11"
                      height="11"
                      viewBox="0 0 12 12"
                      style={{ color: "var(--fg-tertiary)" }}
                    >
                      <path
                        d="M4 2 L8 6 L4 10"
                        stroke="currentColor"
                        stroke-width="1.4"
                        fill="none"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                      />
                    </svg>
                  </div>
                );
              }}
            </For>
          </div>
        </div>
      </Show>

      <Show
        when={
          !running() &&
          hasRun() &&
          counts().failed === 0 &&
          counts().passed > 0
        }
      >
        <div
          style={{
            padding: "36px 24px",
            "text-align": "center",
            background:
              "linear-gradient(180deg, var(--passed-dim) 0%, transparent 100%)",
            border: "1px solid var(--passed-dim)",
            "border-radius": "var(--r-xl)",
            "margin-bottom": "22px",
          }}
        >
          <div
            style={{
              display: "inline-flex",
              "align-items": "center",
              "justify-content": "center",
              width: "44px",
              height: "44px",
              "border-radius": "50%",
              background: "var(--passed-dim)",
              "margin-bottom": "12px",
            }}
          >
            <StatusDot status="passed" size={18} />
          </div>
          <div
            style={{
              "font-size": "18px",
              "font-weight": 600,
              color: "var(--fg-primary)",
              "letter-spacing": "-0.01em",
              "margin-bottom": "4px",
            }}
          >
            All {counts().passed} steps passed.
          </div>
          <div
            style={{
              "font-size": "13px",
              color: "var(--fg-secondary)",
              "font-family": "var(--font-mono)",
            }}
          >
            {duration() != null ? fmtDuration(duration()!) : ""} · {toolbarState.activeEnv ?? "default"}
          </div>
        </div>
      </Show>

      <Show when={!running() && !hasRun()}>
        <div style={{ "padding-top": "12px" }}>
          <SubsectionLabel>This project</SubsectionLabel>
          <div
            style={{
              display: "grid",
              "grid-template-columns": "repeat(3, 1fr)",
              gap: "10px",
              "margin-top": "10px",
            }}
          >
            <FactTile k="Files" v={projectState.files.length} />
            <FactTile
              k="Tests"
              v={projectState.files.reduce((acc, f) => acc + f.tests.length, 0)}
            />
            <FactTile k="Steps" v={counts().total} />
            <FactTile
              k="Tags"
              v={
                Array.from(
                  new Set(projectState.files.flatMap((f) => f.tags ?? [])),
                ).length
              }
            />
            <FactTile k="Target env" v={toolbarState.activeEnv ?? "default"} mono />
            <FactTile k="Last run" v="never" muted />
          </div>
          <div
            style={{
              "margin-top": "22px",
              "font-size": "12.5px",
              color: "var(--fg-tertiary)",
              "line-height": 1.6,
            }}
          >
            Press <KBD>⌘R</KBD> to run all, <KBD>↑</KBD>/<KBD>↓</KBD> to walk the tree,
            <KBD>⏎</KBD> to open a step. Hold <KBD>⌥</KBD> on a step to run only that step.
          </div>
        </div>
      </Show>
    </div>
  );
}

function fmtDuration(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${ms}ms`;
}

function stripColor(s: StepStatus): string {
  switch (s) {
    case "passed":
      return "var(--passed)";
    case "failed":
      return "var(--failed)";
    case "running":
      return "var(--running)";
    case "skipped":
      return "var(--skipped)";
    default:
      return "transparent";
  }
}

function Counter(props: {
  label: string;
  value: number;
  color: string;
  hero?: boolean;
  subdued?: boolean;
  pulse?: boolean;
}) {
  return (
    <div style={{ display: "flex", "flex-direction": "column", "min-width": "56px" }}>
      <span
        class={props.pulse ? "tarn-pulse" : ""}
        style={{
          "font-family": "var(--font-mono)",
          "font-size": props.hero ? "36px" : "18px",
          color: props.subdued ? "var(--fg-secondary)" : props.color,
          "font-variant-numeric": "tabular-nums",
          "letter-spacing": "-0.02em",
          "line-height": 1,
          "font-weight": 400,
        }}
      >
        {props.value}
      </span>
      <span
        style={{
          "margin-top": "6px",
          "font-size": "10.5px",
          "font-weight": 600,
          "letter-spacing": "0.08em",
          "text-transform": "uppercase",
          color: props.subdued ? "var(--fg-tertiary)" : props.color,
        }}
      >
        {props.label}
      </span>
    </div>
  );
}

function FactTile(props: { k: string; v: string | number; mono?: boolean; muted?: boolean }) {
  return (
    <div
      style={{
        padding: "12px 14px",
        background: "var(--bg-surface)",
        border: "1px solid var(--line)",
        "border-radius": "var(--r-md)",
      }}
    >
      <div
        style={{
          "font-size": "10.5px",
          "font-weight": 600,
          "letter-spacing": "0.06em",
          "text-transform": "uppercase",
          color: "var(--fg-tertiary)",
        }}
      >
        {props.k}
      </div>
      <div
        style={{
          "font-family": props.mono ? "var(--font-mono)" : "var(--font-sans)",
          "font-size": props.mono ? "14px" : "18px",
          "font-weight": 500,
          color: props.muted ? "var(--fg-tertiary)" : "var(--fg-primary)",
          "margin-top": "2px",
          "font-variant-numeric": "tabular-nums",
        }}
      >
        {props.v}
      </div>
    </div>
  );
}
