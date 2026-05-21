import { For, type JSX, Show } from "solid-js";

export type CodeToken = {
  t: "k" | "s" | "n" | "b" | "p" | "c" | "q";
  v: string;
};

/**
 * Tokenize a JSON-ish string into syntax-coloured spans. Tiny — not a real
 * parser. Mirrors the design package's `tokenizeJSON`.
 */
export function tokenizeJSON(src: string): CodeToken[] {
  const tokens: CodeToken[] = [];
  const re = /("(?:\\.|[^"\\])*"\s*:)|("(?:\\.|[^"\\])*")|(-?\d+(?:\.\d+)?)|(true|false|null)|([{}\[\],])|(\s+)|([^\s{}\[\],"]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(src)) !== null) {
    if (m[1]) tokens.push({ t: "k", v: m[1] });
    else if (m[2]) tokens.push({ t: "s", v: m[2] });
    else if (m[3]) tokens.push({ t: "n", v: m[3] });
    else if (m[4]) tokens.push({ t: "b", v: m[4] });
    else if (m[5]) tokens.push({ t: "p", v: m[5] });
    else if (m[6]) tokens.push({ t: "p", v: m[6] });
    else if (m[7]) tokens.push({ t: "p", v: m[7] });
  }
  return tokens;
}

function tokenColor(t: CodeToken["t"]): string {
  switch (t) {
    case "k":
      return "oklch(0.76 0.10 280)";
    case "s":
    case "q":
      return "oklch(0.78 0.10 145)";
    case "n":
      return "oklch(0.80 0.08 60)";
    case "b":
      return "oklch(0.74 0.10 30)";
    case "c":
      return "var(--fg-tertiary)";
    default:
      return "var(--fg-secondary)";
  }
}

export default function CodeBlock(props: {
  tokens?: CodeToken[];
  text?: string;
  title?: string;
  copy?: boolean;
  height?: number;
  style?: JSX.CSSProperties;
}) {
  return (
    <div
      style={{
        background: "var(--bg-input)",
        border: "1px solid var(--line)",
        "border-radius": "var(--r-lg)",
        overflow: "hidden",
        ...props.style,
      }}
    >
      <Show when={props.title || props.copy}>
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
            padding: "7px 10px 6px 12px",
            "border-bottom": "1px solid var(--line)",
            background: "var(--bg-surface)",
          }}
        >
          <span
            style={{
              "font-size": "11px",
              color: "var(--fg-tertiary)",
              "font-family": "var(--font-mono)",
            }}
          >
            {props.title}
          </span>
          <Show when={props.copy}>
            <button
              style={{
                background: "transparent",
                border: "none",
                color: "var(--fg-tertiary)",
                "font-size": "11px",
                "font-family": "var(--font-sans)",
                cursor: "pointer",
                display: "inline-flex",
                "align-items": "center",
                gap: "4px",
                padding: "0",
              }}
            >
              <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
                <rect
                  x="3.5"
                  y="3.5"
                  width="6"
                  height="6"
                  rx="1.2"
                  stroke="currentColor"
                  stroke-width="1.2"
                />
                <path
                  d="M2.5 7.5 V3 a1 1 0 0 1 1 -1 H7"
                  stroke="currentColor"
                  stroke-width="1.2"
                  fill="none"
                  stroke-linecap="round"
                />
              </svg>
              <span>copy</span>
            </button>
          </Show>
        </div>
      </Show>
      <div
        class="scroll"
        style={{
          padding: "10px 14px",
          "font-family": "var(--font-mono)",
          "font-size": "12px",
          "line-height": 1.55,
          color: "var(--fg-secondary)",
          "max-height": props.height ? `${props.height}px` : undefined,
          overflow: "auto",
        }}
      >
        <Show
          when={props.tokens}
          fallback={
            <pre style={{ margin: 0, "font-family": "inherit", "white-space": "pre" }}>
              {props.text}
            </pre>
          }
        >
          <pre style={{ margin: 0, "font-family": "inherit", "white-space": "pre" }}>
            <For each={props.tokens}>
              {(tk) => <span style={{ color: tokenColor(tk.t) }}>{tk.v}</span>}
            </For>
          </pre>
        </Show>
      </div>
    </div>
  );
}
