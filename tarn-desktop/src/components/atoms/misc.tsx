import type { JSX } from "solid-js";

export function Chevron(props: { open?: boolean; size?: number; style?: JSX.CSSProperties }) {
  const size = () => props.size ?? 9;
  return (
    <svg
      width={size()}
      height={size()}
      viewBox="0 0 10 10"
      style={{
        transform: `rotate(${props.open ? 90 : 0}deg)`,
        transition: "transform 100ms ease",
        "flex-shrink": 0,
        color: "var(--fg-tertiary)",
        ...props.style,
      }}
    >
      <path
        d="M3.5 2 L7 5 L3.5 8"
        stroke="currentColor"
        stroke-width="1.3"
        fill="none"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

const METHOD_COLORS: Record<string, string> = {
  GET: "oklch(0.78 0.08 220)",
  POST: "oklch(0.78 0.10 145)",
  PATCH: "oklch(0.80 0.10 75)",
  PUT: "oklch(0.80 0.10 75)",
  DELETE: "oklch(0.74 0.12 30)",
};

const METHOD_TINTS: Record<string, string> = {
  GET: "oklch(0.78 0.08 220 / 0.18)",
  POST: "oklch(0.78 0.10 145 / 0.18)",
  PATCH: "oklch(0.80 0.10 75 / 0.18)",
  PUT: "oklch(0.80 0.10 75 / 0.18)",
  DELETE: "oklch(0.74 0.12 30 / 0.18)",
};

const METHOD_FG_BRIGHT: Record<string, string> = {
  GET: "oklch(0.86 0.08 220)",
  POST: "oklch(0.86 0.10 145)",
  PATCH: "oklch(0.86 0.10 75)",
  PUT: "oklch(0.86 0.10 75)",
  DELETE: "oklch(0.82 0.12 30)",
};

export function methodColor(method: string): string {
  return METHOD_COLORS[method.toUpperCase()] ?? "var(--fg-secondary)";
}

export function methodTint(method: string): string {
  return METHOD_TINTS[method.toUpperCase()] ?? "var(--bg-elevated)";
}

export function methodFgBright(method: string): string {
  return METHOD_FG_BRIGHT[method.toUpperCase()] ?? "var(--fg-primary)";
}

export function MethodTag(props: {
  method: string;
  dim?: boolean;
  style?: JSX.CSSProperties;
}) {
  return (
    <span
      style={{
        "font-family": "var(--font-mono)",
        "font-size": "10px",
        "font-weight": 600,
        "letter-spacing": "0.04em",
        color: props.dim ? "var(--fg-tertiary)" : methodColor(props.method),
        width: "44px",
        "text-align": "left",
        display: "inline-block",
        "flex-shrink": 0,
        ...props.style,
      }}
    >
      {props.method.toUpperCase()}
    </span>
  );
}

export function Duration(props: { ms?: number | null; dim?: boolean; style?: JSX.CSSProperties }) {
  const ms = () => props.ms;
  const text = () => {
    const v = ms();
    if (v == null) return "";
    return v >= 1000 ? `${(v / 1000).toFixed(1)}s` : `${v}ms`;
  };
  if (ms() == null) return null;
  return (
    <span
      style={{
        "font-family": "var(--font-mono)",
        "font-size": "11px",
        color: props.dim ? "var(--fg-quaternary)" : "var(--fg-tertiary)",
        "font-variant-numeric": "tabular-nums",
        ...props.style,
      }}
    >
      {text()}
    </span>
  );
}

export function KBD(props: { children: JSX.Element }) {
  return (
    <kbd
      style={{
        "font-family": "var(--font-mono)",
        "font-size": "11px",
        background: "var(--bg-surface)",
        border: "1px solid var(--line-strong)",
        "border-bottom-width": "1.5px",
        "border-radius": "3px",
        padding: "0 5px",
        color: "var(--fg-secondary)",
        margin: "0 3px",
      }}
    >
      {props.children}
    </kbd>
  );
}

export function SubsectionLabel(props: { children: JSX.Element; style?: JSX.CSSProperties }) {
  return (
    <span
      style={{
        "font-size": "11px",
        "font-weight": 600,
        "letter-spacing": "0.08em",
        "text-transform": "uppercase",
        color: "var(--fg-tertiary)",
        ...props.style,
      }}
    >
      {props.children}
    </span>
  );
}
