import { createSignal, For, Show } from "solid-js";
import { recentProjects } from "../stores/recent";
import Button from "./atoms/Button";

type Props = {
  onOpen: () => void;
  onOpenPath: (path: string) => void;
};

export default function EmptyState(props: Props) {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        padding: "40px",
      }}
    >
      <div style={{ width: "440px", "text-align": "left" }}>
        <div
          style={{
            width: "38px",
            height: "38px",
            "border-radius": "9px",
            background: "var(--bg-surface)",
            border: "1px solid var(--line-strong)",
            display: "inline-flex",
            "align-items": "center",
            "justify-content": "center",
            "margin-bottom": "20px",
          }}
        >
          <div
            style={{
              width: "14px",
              height: "14px",
              "border-radius": "50%",
              background: "var(--accent)",
            }}
          />
        </div>
        <div
          style={{
            "font-size": "22px",
            "font-weight": 600,
            color: "var(--fg-primary)",
            "letter-spacing": "-0.01em",
            "margin-bottom": "4px",
          }}
        >
          Open a project
        </div>
        <div
          style={{
            "font-size": "13.5px",
            color: "var(--fg-secondary)",
            "line-height": 1.55,
            "margin-bottom": "24px",
          }}
        >
          A folder containing{" "}
          <span class="mono" style={{ color: "var(--fg-primary)" }}>
            .tarn.yaml
          </span>{" "}
          files.{" "}
          <Show
            when={recentProjects().length > 0}
            fallback="Or drop one onto this window."
          >
            Pick a recent one or browse to a new path.
          </Show>
        </div>

        <div style={{ display: "flex", gap: "8px", "margin-bottom": "24px" }}>
          <Button
            kind="primary"
            size="md"
            kbd="⌘O"
            onClick={props.onOpen}
            icon={
              <svg width="12" height="12" viewBox="0 0 14 14" fill="none">
                <path
                  d="M2 4 a1 1 0 0 1 1 -1 H6 l1.3 1.3 H11 a1 1 0 0 1 1 1 V11 a1 1 0 0 1 -1 1 H3 a1 1 0 0 1 -1 -1 z"
                  stroke="currentColor"
                  stroke-width="1.3"
                  fill="none"
                />
              </svg>
            }
          >
            Choose folder
          </Button>
          <Button
            kind="secondary"
            size="md"
            icon={
              <svg width="12" height="12" viewBox="0 0 14 14" fill="none">
                <rect
                  x="2.5"
                  y="3"
                  width="9"
                  height="8"
                  rx="1.2"
                  stroke="currentColor"
                  stroke-width="1.2"
                />
                <path
                  d="M5 6 H9 M5 8 H7.5"
                  stroke="currentColor"
                  stroke-width="1.2"
                  stroke-linecap="round"
                />
              </svg>
            }
          >
            New from template
          </Button>
        </div>

        <Show
          when={recentProjects().length > 0}
          fallback={
            <div
              style={{
                border: "1px dashed var(--line-strong)",
                "border-radius": "var(--r-lg)",
                padding: "22px 18px",
                color: "var(--fg-tertiary)",
                "font-size": "12.5px",
                "text-align": "center",
              }}
            >
              Drop a folder anywhere on this window.
            </div>
          }
        >
          <div>
            <div
              style={{
                "font-size": "11px",
                "font-weight": 600,
                "letter-spacing": "0.08em",
                "text-transform": "uppercase",
                color: "var(--fg-tertiary)",
                "margin-bottom": "8px",
              }}
            >
              Recent
            </div>
            <div
              style={{
                border: "1px solid var(--line)",
                "border-radius": "var(--r-lg)",
                overflow: "hidden",
                background: "var(--bg-surface)",
              }}
            >
              <For each={recentProjects()}>
                {(path, i) => (
                  <RecentRow
                    path={path}
                    isFirst={i() === 0}
                    onClick={() => props.onOpenPath(path)}
                  />
                )}
              </For>
            </div>
          </div>
        </Show>
      </div>
    </div>
  );
}

function RecentRow(props: { path: string; isFirst: boolean; onClick: () => void }) {
  const [hover, setHover] = createSignal(false);
  const name = () => props.path.split("/").filter(Boolean).pop() ?? props.path;
  return (
    <div
      onClick={props.onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "flex",
        "align-items": "center",
        gap: "10px",
        padding: "9px 14px",
        cursor: "pointer",
        background: hover() ? "var(--bg-elevated)" : "transparent",
        "border-top": props.isFirst ? "none" : "1px solid var(--line)",
      }}
    >
      <svg
        width="13"
        height="13"
        viewBox="0 0 14 14"
        style={{ color: "var(--fg-tertiary)" }}
      >
        <path
          d="M2 4 a1 1 0 0 1 1 -1 H6 l1.3 1.3 H11 a1 1 0 0 1 1 1 V11 a1 1 0 0 1 -1 1 H3 a1 1 0 0 1 -1 -1 z"
          stroke="currentColor"
          stroke-width="1.2"
          fill="none"
        />
      </svg>
      <span style={{ "font-size": "13px", color: "var(--fg-primary)", "font-weight": 500 }}>
        {name()}
      </span>
      <span style={{ flex: 1 }} />
      <span
        style={{
          "font-family": "var(--font-mono)",
          "font-size": "11.5px",
          color: "var(--fg-tertiary)",
        }}
      >
        {props.path}
      </span>
      <svg
        width="11"
        height="11"
        viewBox="0 0 12 12"
        style={{ color: hover() ? "var(--fg-secondary)" : "var(--fg-quaternary)" }}
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
}
