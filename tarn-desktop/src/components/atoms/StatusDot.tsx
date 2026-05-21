import type { JSX } from "solid-js";
import { Show } from "solid-js";
import type { StepStatus } from "../../stores/run";

/**
 * Five shape-distinct status indicators (color is redundant for colour-blind users):
 *   passed  → solid filled circle
 *   failed  → filled circle with a negative-space inner ring (donut)
 *   running → filled circle + outer glow, 1 Hz opacity pulse
 *   skipped → hollow ring
 *   pending → tiny dim dot centred inside a transparent slot
 *
 * Ported from design package (atoms.jsx).
 */
export default function StatusDot(props: {
  status?: StepStatus;
  size?: number;
  style?: JSX.CSSProperties;
}) {
  const status = () => props.status ?? "pending";
  const size = () => props.size ?? 9;
  const colorVar = () => {
    switch (status()) {
      case "passed":
        return "var(--passed)";
      case "failed":
        return "var(--failed)";
      case "running":
        return "var(--running)";
      case "skipped":
        return "var(--skipped)";
      default:
        return "var(--pending)";
    }
  };

  const common = (): JSX.CSSProperties => ({
    width: `${size()}px`,
    height: `${size()}px`,
    "border-radius": "50%",
    "flex-shrink": 0,
    display: "inline-block",
    position: "relative",
    ...props.style,
  });

  return (
    <>
      <Show when={status() === "passed"}>
        <span style={{ ...common(), background: colorVar() }} aria-label="passed" />
      </Show>
      <Show when={status() === "failed"}>
        <span
          style={{
            ...common(),
            background: colorVar(),
            "box-shadow": `inset 0 0 0 ${Math.max(1, size() * 0.18)}px var(--bg-base)`,
          }}
          aria-label="failed"
        >
          <span
            style={{
              position: "absolute",
              inset: "0",
              margin: "auto",
              width: `${size() * 0.28}px`,
              height: `${size() * 0.28}px`,
              "border-radius": "50%",
              background: colorVar(),
              top: "0",
              bottom: "0",
              left: "0",
              right: "0",
            }}
          />
        </span>
      </Show>
      <Show when={status() === "running"}>
        <span
          class="tarn-pulse"
          style={{
            ...common(),
            background: colorVar(),
            "box-shadow": `0 0 0 ${Math.max(2, size() * 0.45)}px var(--running-dim)`,
          }}
          aria-label="running"
        />
      </Show>
      <Show when={status() === "skipped"}>
        <span
          style={{
            ...common(),
            background: "transparent",
            "box-shadow": `inset 0 0 0 ${Math.max(1.2, size() * 0.16)}px ${colorVar()}`,
          }}
          aria-label="skipped"
        />
      </Show>
      <Show when={status() === "pending"}>
        <span
          style={{
            ...common(),
            background: "transparent",
            display: "inline-flex",
            "align-items": "center",
            "justify-content": "center",
          }}
          aria-label="pending"
        >
          <span
            style={{
              width: `${Math.max(2, Math.round(size() * 0.36))}px`,
              height: `${Math.max(2, Math.round(size() * 0.36))}px`,
              "border-radius": "50%",
              background: colorVar(),
            }}
          />
        </span>
      </Show>
    </>
  );
}

export const STATUS_LABEL: Record<StepStatus, string> = {
  passed: "PASS",
  failed: "FAIL",
  running: "RUN",
  skipped: "SKIP",
  pending: "—",
};

export function StatusTag(props: { status: StepStatus; style?: JSX.CSSProperties }) {
  const color = () => {
    switch (props.status) {
      case "passed":
        return "var(--passed)";
      case "failed":
        return "var(--failed)";
      case "running":
        return "var(--running)";
      case "skipped":
        return "var(--skipped)";
      default:
        return "var(--fg-tertiary)";
    }
  };
  return (
    <span
      style={{
        "font-size": "10px",
        "font-weight": 600,
        "letter-spacing": "0.08em",
        color: color(),
        "font-variant-numeric": "tabular-nums",
        ...props.style,
      }}
    >
      {STATUS_LABEL[props.status]}
    </span>
  );
}
