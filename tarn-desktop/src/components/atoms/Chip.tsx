import { createSignal, type JSX, Show } from "solid-js";

export default function Chip(props: {
  children: JSX.Element;
  active?: boolean;
  onClick?: () => void;
  count?: number;
  style?: JSX.CSSProperties;
}) {
  const [hover, setHover] = createSignal(false);
  const bg = () =>
    props.active ? "var(--accent-dim)" : hover() ? "var(--bg-elevated)" : "transparent";
  const fg = () =>
    props.active ? "var(--fg-primary)" : hover() ? "var(--fg-primary)" : "var(--fg-secondary)";
  const border = () =>
    props.active ? "var(--accent-line)" : "var(--line)";
  return (
    <span
      onClick={props.onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "5px",
        height: "22px",
        padding: "0 8px",
        "border-radius": "11px",
        background: bg(),
        color: fg(),
        border: `1px solid ${border()}`,
        "font-size": "11.5px",
        "font-weight": 500,
        cursor: "pointer",
        transition: "background 120ms, color 120ms, border-color 120ms",
        ...props.style,
      }}
    >
      <span
        style={{
          width: "5px",
          height: "5px",
          "border-radius": "50%",
          background: props.active ? "var(--accent)" : "var(--fg-tertiary)",
        }}
      />
      <span>{props.children}</span>
      <Show when={props.count != null}>
        <span
          style={{
            color: "var(--fg-tertiary)",
            "font-variant-numeric": "tabular-nums",
            "font-size": "11px",
          }}
        >
          {props.count}
        </span>
      </Show>
    </span>
  );
}
