import { createSignal, For, Show } from "solid-js";
import { Dropdown, Menu } from "./atoms/Dropdown";
import { IconButton } from "./atoms/Button";
import { projectState } from "../stores/project";
import { setActiveEnv, toolbarState } from "../stores/toolbar";

const TRAFFIC_LIGHTS = ["#ff5f57", "#febc2e", "#28c940"] as const;

type Props = {
  onOpenProject: () => void;
};

export default function Titlebar(props: Props) {
  const [envOpen, setEnvOpen] = createSignal(false);
  const hasProject = () => !!projectState.path;
  const projectName = () => {
    if (!projectState.path) return null;
    const parts = projectState.path.split("/");
    return parts[parts.length - 1] || projectState.path;
  };
  const projectPath = () =>
    projectState.path ? abbreviateHome(projectState.path) : null;

  const envItems = () => {
    const items = toolbarState.environments.map((e) => ({
      label: e.name,
      value: e.is_base ? "" : e.name,
      hint: e.file,
    }));
    if (items.length === 0) {
      items.push({ label: "default", value: "", hint: "tarn.env.yaml" });
    }
    return items;
  };

  return (
    <div
      style={{
        display: "flex",
        "align-items": "center",
        gap: "10px",
        height: "38px",
        padding: "0 12px",
        "border-bottom": "1px solid var(--line)",
        background: "var(--bg-base)",
        "flex-shrink": 0,
        "-webkit-app-region": "drag",
        "user-select": "none",
      }}
    >
      <div style={{ display: "flex", gap: "8px", "padding-right": "4px" }}>
        <For each={TRAFFIC_LIGHTS}>
          {(c) => (
            <span
              style={{
                width: "11px",
                height: "11px",
                "border-radius": "50%",
                background: c,
                opacity: 0.85,
              }}
            />
          )}
        </For>
      </div>

      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "8px",
          "-webkit-app-region": "no-drag",
        }}
      >
        <Show
          when={hasProject()}
          fallback={
            <span
              style={{
                "font-size": "13px",
                "font-weight": 600,
                color: "var(--fg-secondary)",
              }}
            >
              Tarn Studio
            </span>
          }
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 14 14"
            style={{ color: "var(--fg-tertiary)" }}
          >
            <path
              d="M2 4.5 a1 1 0 0 1 1 -1 H6 l1 1.3 H11 a1 1 0 0 1 1 1 V11 a1 1 0 0 1 -1 1 H3 a1 1 0 0 1 -1 -1 z"
              stroke="currentColor"
              stroke-width="1.2"
              fill="none"
            />
          </svg>
          <button
            onClick={props.onOpenProject}
            style={{
              background: "transparent",
              border: "none",
              padding: 0,
              cursor: "pointer",
              display: "inline-flex",
              "align-items": "center",
              gap: "8px",
            }}
          >
            <span
              style={{
                "font-size": "13px",
                "font-weight": 600,
                color: "var(--fg-primary)",
              }}
            >
              {projectName()}
            </span>
            <span
              style={{
                "font-size": "12px",
                color: "var(--fg-tertiary)",
                "font-family": "var(--font-mono)",
              }}
            >
              {projectPath()}
            </span>
            <svg
              width="9"
              height="9"
              viewBox="0 0 10 10"
              style={{ color: "var(--fg-tertiary)", "margin-left": "-2px" }}
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
        </Show>
      </div>

      <div style={{ flex: 1 }} />

      <Show when={hasProject()}>
        <div style={{ "-webkit-app-region": "no-drag" }}>
          <Dropdown
            label="env"
            value={toolbarState.activeEnv ?? "default"}
            open={envOpen()}
            onToggle={() => setEnvOpen(!envOpen())}
          >
            <Menu
              value={toolbarState.activeEnv ?? ""}
              onPick={(v) => {
                setActiveEnv(v === "" ? null : v);
                setEnvOpen(false);
              }}
              items={envItems()}
            />
          </Dropdown>
        </div>

        <div style={{ "-webkit-app-region": "no-drag" }}>
          <IconButton title="Run history">
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
              <circle
                cx="7"
                cy="7"
                r="4.6"
                stroke="currentColor"
                stroke-width="1.2"
              />
              <path
                d="M7 4.4 V7 L9 8.4"
                stroke="currentColor"
                stroke-width="1.2"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </IconButton>
        </div>
      </Show>

      <div style={{ "-webkit-app-region": "no-drag" }}>
        <IconButton title="Settings">
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
            <circle
              cx="7"
              cy="7"
              r="1.8"
              stroke="currentColor"
              stroke-width="1.2"
            />
            <path
              d="M7 1.5 V3 M7 11 V12.5 M1.5 7 H3 M11 7 H12.5 M2.7 2.7 L3.8 3.8 M10.2 10.2 L11.3 11.3 M11.3 2.7 L10.2 3.8 M3.8 10.2 L2.7 11.3"
              stroke="currentColor"
              stroke-width="1.2"
              stroke-linecap="round"
            />
          </svg>
        </IconButton>
      </div>
    </div>
  );
}

/**
 * Compresses `/Users/<name>/work/foo` to `~/work/foo`. The WebView has
 * no way to read $HOME directly, so we match the macOS pattern by
 * heuristic — the second path segment after `/Users` is taken to be
 * the home folder name. Falls back to the original path on any other
 * shape.
 */
function abbreviateHome(path: string): string {
  const m = path.match(/^\/Users\/[^/]+(\/.*)?$/);
  if (m) return `~${m[1] ?? ""}`;
  return path;
}
