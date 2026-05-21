import { createSignal, type JSX, Show } from "solid-js";

type Kind = "primary" | "secondary" | "ghost" | "danger";
type Size = "sm" | "md";

export default function Button(props: {
  kind?: Kind;
  size?: Size;
  children?: JSX.Element;
  icon?: JSX.Element;
  iconRight?: JSX.Element;
  disabled?: boolean;
  onClick?: () => void;
  kbd?: string;
  title?: string;
  style?: JSX.CSSProperties;
}) {
  const kind = () => props.kind ?? "secondary";
  const size = () => props.size ?? "md";

  const padX = () => (size() === "sm" ? 8 : 10);
  const padY = () => (size() === "sm" ? 4 : 6);
  const fs = () => (size() === "sm" ? 12 : 12.5);
  const h = () => (size() === "sm" ? 24 : 28);

  const palette = () => {
    const k = kind();
    if (k === "primary")
      return {
        bg: "var(--accent)",
        fg: "var(--fg-on-accent)",
        border: "1px solid transparent",
        hoverBg: "var(--accent-hover)",
        hoverFg: "var(--fg-on-accent)",
      };
    if (k === "ghost")
      return {
        bg: "transparent",
        fg: "var(--fg-secondary)",
        border: "1px solid transparent",
        hoverBg: "var(--bg-elevated)",
        hoverFg: "var(--fg-primary)",
      };
    if (k === "danger")
      return {
        bg: "transparent",
        fg: "var(--failed)",
        border: "1px solid var(--line)",
        hoverBg: "var(--failed-dim)",
        hoverFg: "var(--failed)",
      };
    return {
      bg: "var(--bg-surface)",
      fg: "var(--fg-primary)",
      border: "1px solid var(--line)",
      hoverBg: "var(--bg-elevated)",
      hoverFg: "var(--fg-primary)",
    };
  };

  const [hover, setHover] = createSignal(false);

  return (
    <button
      title={props.title}
      onClick={props.onClick}
      disabled={props.disabled}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "6px",
        height: `${h()}px`,
        padding: `${padY()}px ${padX()}px`,
        "border-radius": "var(--r-md)",
        background: props.disabled
          ? "var(--bg-surface)"
          : hover()
            ? palette().hoverBg
            : palette().bg,
        color: props.disabled
          ? "var(--fg-quaternary)"
          : hover()
            ? palette().hoverFg
            : palette().fg,
        border: palette().border,
        "font-family": "var(--font-sans)",
        "font-size": `${fs()}px`,
        "font-weight": 500,
        cursor: props.disabled ? "not-allowed" : "pointer",
        transition: "background 120ms, color 120ms, border-color 120ms",
        ...props.style,
      }}
    >
      {props.icon}
      <Show when={props.children}>
        <span>{props.children}</span>
      </Show>
      {props.iconRight}
      <Show when={props.kbd}>
        <span
          style={{
            "margin-left": "4px",
            "font-family": "var(--font-mono)",
            "font-size": "10.5px",
            color: kind() === "primary" ? "rgba(255,255,255,0.7)" : "var(--fg-tertiary)",
            padding: "1px 5px",
            border: "1px solid currentColor",
            opacity: 0.55,
            "border-radius": "3px",
            "line-height": 1,
          }}
        >
          {props.kbd}
        </span>
      </Show>
    </button>
  );
}

export function IconButton(props: {
  children: JSX.Element;
  onClick?: () => void;
  title?: string;
  active?: boolean;
  style?: JSX.CSSProperties;
}) {
  const [hover, setHover] = createSignal(false);
  const bg = () =>
    props.active || hover() ? "var(--bg-elevated)" : "transparent";
  const fg = () =>
    props.active || hover() ? "var(--fg-primary)" : "var(--fg-secondary)";
  return (
    <button
      title={props.title}
      onClick={props.onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        width: "26px",
        height: "26px",
        "border-radius": "5px",
        border: "1px solid transparent",
        background: bg(),
        color: fg(),
        display: "inline-flex",
        "align-items": "center",
        "justify-content": "center",
        cursor: "pointer",
        padding: "0",
        transition: "background 120ms, color 120ms",
        ...props.style,
      }}
    >
      {props.children}
    </button>
  );
}
