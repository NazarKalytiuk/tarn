import { createMemo, Show } from "solid-js";
import { runState, type StepStatus } from "../stores/run";
import { toolbarState } from "../stores/toolbar";
import StatusDot from "./atoms/StatusDot";
import { projectState } from "../stores/project";
import { flatSteps } from "../utils/flatSteps";

export default function Statusbar(props: { errorText?: string }) {
  const counts = createMemo(() => {
    const all = flatSteps(projectState.files);
    const c = { passed: 0, failed: 0, running: 0, skipped: 0, pending: 0 };
    for (const fs of all) {
      const s = (runState.steps[fs.key]?.status ?? "pending") as StepStatus;
      c[s] += 1;
    }
    return c;
  });

  const running = () => runState.status === "running";
  const duration = () => {
    if (runState.startedAt == null) return null;
    const end = runState.finishedAt ?? Date.now();
    return end - runState.startedAt;
  };

  return (
    <div
      style={{
        display: "flex",
        "align-items": "center",
        gap: "14px",
        height: "28px",
        padding: "0 12px",
        "border-top": "1px solid var(--line)",
        background: "var(--bg-base)",
        "flex-shrink": 0,
        "font-size": "11.5px",
        color: "var(--fg-tertiary)",
      }}
    >
      <CountEntry status="passed" count={counts().passed} color="var(--fg-secondary)" />
      <CountEntry
        status="failed"
        count={counts().failed}
        color={counts().failed > 0 ? "var(--failed)" : "var(--fg-tertiary)"}
      />
      <CountEntry
        status="running"
        count={counts().running}
        color={running() ? "var(--running)" : "var(--fg-tertiary)"}
      />
      <Show when={counts().skipped > 0}>
        <CountEntry status="skipped" count={counts().skipped} color="var(--fg-tertiary)" />
      </Show>
      <CountEntry status="pending" count={counts().pending} color="var(--fg-tertiary)" />

      <div style={{ flex: 1 }} />

      <Show when={props.errorText}>
        <span style={{ color: "var(--failed)" }}>{props.errorText}</span>
      </Show>

      <Show when={duration() != null}>
        <span
          style={{
            "font-family": "var(--font-mono)",
            "font-variant-numeric": "tabular-nums",
          }}
        >
          {running() ? "elapsed" : "duration"}{" "}
          {duration()! >= 1000 ? `${(duration()! / 1000).toFixed(2)}s` : `${duration()!}ms`}
        </span>
      </Show>

      <span
        style={{
          display: "inline-flex",
          "align-items": "center",
          gap: "4px",
          "font-family": "var(--font-mono)",
        }}
      >
        <span
          style={{
            width: "5px",
            height: "5px",
            "border-radius": "50%",
            background: "var(--accent)",
          }}
        />
        {toolbarState.activeEnv ?? "default"}
      </span>
    </div>
  );
}

function CountEntry(props: { status: StepStatus; count: number; color: string }) {
  return (
    <span
      style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "6px",
      }}
    >
      <StatusDot status={props.status} size={7} />
      <span
        style={{
          color: props.color,
          "font-variant-numeric": "tabular-nums",
        }}
      >
        {props.count} {props.status}
      </span>
    </span>
  );
}
