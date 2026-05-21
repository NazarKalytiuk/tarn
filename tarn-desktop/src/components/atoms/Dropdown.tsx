import { createSignal, For, type JSX, Show } from "solid-js";

export function Dropdown(props: {
  label?: string;
  value: string;
  hint?: string;
  open?: boolean;
  onToggle?: () => void;
  style?: JSX.CSSProperties;
  children?: JSX.Element;
  width?: number;
}) {
  const [hover, setHover] = createSignal(false);
  return (
    <div style={{ position: "relative", display: "inline-block", ...props.style }}>
      <button
        onClick={() => props.onToggle?.()}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
        style={{
          height: "28px",
          padding: "0 8px 0 10px",
          "min-width": props.width ? `${props.width}px` : undefined,
          background:
            hover() || props.open ? "var(--bg-elevated)" : "var(--bg-surface)",
          color: "var(--fg-primary)",
          border: "1px solid var(--line)",
          "border-radius": "var(--r-md)",
          display: "inline-flex",
          "align-items": "center",
          gap: "8px",
          "font-family": "var(--font-sans)",
          "font-size": "12.5px",
          "font-weight": 500,
          cursor: "pointer",
          transition: "background 120ms",
        }}
      >
        <Show when={props.label}>
          <span style={{ color: "var(--fg-tertiary)", "font-weight": 500 }}>
            {props.label}
          </span>
        </Show>
        <span>{props.value}</span>
        <Show when={props.hint}>
          <span style={{ color: "var(--fg-tertiary)", "font-size": "11.5px" }}>
            {props.hint}
          </span>
        </Show>
        <svg
          width="9"
          height="9"
          viewBox="0 0 10 10"
          style={{ "margin-left": "2px", opacity: 0.6 }}
        >
          <path
            d="M2 4 L5 7 L8 4"
            stroke="currentColor"
            stroke-width="1.3"
            fill="none"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      </button>
      <Show when={props.open}>{props.children}</Show>
    </div>
  );
}

export type MenuItem = {
  label: string;
  value: string;
  hint?: string;
  icon?: JSX.Element;
};

export function Menu(props: {
  items: MenuItem[];
  value?: string;
  onPick?: (value: string) => void;
  footer?: JSX.Element;
  style?: JSX.CSSProperties;
}) {
  return (
    <div
      style={{
        position: "absolute",
        top: "calc(100% + 4px)",
        right: "0",
        "z-index": 30,
        "min-width": "200px",
        background: "var(--bg-surface)",
        border: "1px solid var(--line-strong)",
        "border-radius": "var(--r-lg)",
        "box-shadow":
          "0 8px 28px rgba(0,0,0,0.45), 0 0 0 1px rgba(0,0,0,0.2)",
        padding: "4px",
        ...props.style,
      }}
    >
      <For each={props.items}>
        {(it) => (
          <div
            onClick={() => props.onPick?.(it.value)}
            style={{
              display: "flex",
              "align-items": "center",
              gap: "8px",
              padding: "6px 10px",
              "border-radius": "5px",
              cursor: "pointer",
              background:
                it.value === props.value ? "var(--bg-elevated)" : "transparent",
              color: "var(--fg-primary)",
              "font-size": "12.5px",
            }}
            onMouseEnter={(e) =>
              (e.currentTarget.style.background = "var(--bg-elevated)")
            }
            onMouseLeave={(e) =>
              (e.currentTarget.style.background =
                it.value === props.value ? "var(--bg-elevated)" : "transparent")
            }
          >
            {it.icon}
            <span style={{ flex: 1 }}>{it.label}</span>
            <Show when={it.hint}>
              <span style={{ color: "var(--fg-tertiary)", "font-size": "11.5px" }}>
                {it.hint}
              </span>
            </Show>
            <Show when={it.value === props.value}>
              <svg width="11" height="11" viewBox="0 0 12 12">
                <path
                  d="M2.5 6.5 L5 9 L9.5 3.5"
                  stroke="var(--accent)"
                  stroke-width="1.6"
                  fill="none"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
            </Show>
          </div>
        )}
      </For>
      <Show when={props.footer}>
        <div
          style={{
            "border-top": "1px solid var(--line)",
            "margin-top": "4px",
            "padding-top": "6px",
            padding: "6px 10px",
            color: "var(--fg-tertiary)",
            "font-size": "11.5px",
          }}
        >
          {props.footer}
        </div>
      </Show>
    </div>
  );
}
