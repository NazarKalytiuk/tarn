import { createMemo, For, Show } from "solid-js";
import { projectState } from "../stores/project";
import { runState } from "../stores/run";
import { toggleTag, toolbarState } from "../stores/toolbar";
import Button from "./atoms/Button";
import Chip from "./atoms/Chip";

type Props = {
  busy: boolean;
  focusFailures: boolean;
  onRunAll: () => void;
  onRunFailures: () => void;
  onCancel: () => void;
  onToggleFocus: () => void;
};

export default function Toolbar(props: Props) {
  const availableTags = createMemo(() => {
    const counts: Record<string, number> = {};
    for (const f of projectState.files) {
      for (const t of f.tags ?? []) counts[t] = (counts[t] ?? 0) + 1;
    }
    return Object.entries(counts)
      .map(([name, count]) => ({ name, count }))
      .sort((a, b) => a.name.localeCompare(b.name));
  });

  const failed = () =>
    Object.values(runState.steps).filter((s) => s.status === "failed").length;

  return (
    <div
      style={{
        display: "flex",
        "align-items": "center",
        gap: "8px",
        height: "44px",
        padding: "0 12px",
        "border-bottom": "1px solid var(--line)",
        background: "var(--bg-base)",
        "flex-shrink": 0,
      }}
    >
      <div
        style={{
          display: "flex",
          gap: "5px",
          "align-items": "center",
          "min-width": 0,
          overflow: "hidden",
        }}
      >
        <span
          style={{
            "font-size": "11px",
            color: "var(--fg-tertiary)",
            "text-transform": "uppercase",
            "letter-spacing": "0.06em",
            "margin-right": "2px",
          }}
        >
          tags
        </span>
        <For each={availableTags()}>
          {(t) => (
            <Chip
              active={toolbarState.activeTags.includes(t.name)}
              onClick={() => toggleTag(t.name)}
            >
              {t.name}
            </Chip>
          )}
        </For>
        <Show when={availableTags().length === 0}>
          <span style={{ "font-size": "11.5px", color: "var(--fg-quaternary)" }}>
            none declared
          </span>
        </Show>
      </div>

      <div style={{ flex: 1 }} />

      <Show when={failed() > 0 && !props.busy}>
        <Button
          kind="ghost"
          size="sm"
          onClick={props.onToggleFocus}
          title="Show only failed steps"
          icon={
            <svg width="13" height="13" viewBox="0 0 14 14" fill="none">
              <path
                d="M2.5 3 H11.5 L8.4 6.8 V10.7 L5.6 12 V6.8 z"
                stroke="currentColor"
                stroke-width="1.2"
                stroke-linejoin="round"
              />
            </svg>
          }
        >
          {props.focusFailures ? "Showing failures" : `Show only ${failed()} failures`}
        </Button>
      </Show>

      <Show
        when={props.busy}
        fallback={
          <>
            <Button
              kind="ghost"
              size="md"
              title="Re-run last failures only"
              disabled={failed() === 0}
              onClick={props.onRunFailures}
              icon={
                <svg width="12" height="12" viewBox="0 0 14 14" fill="none">
                  <path
                    d="M11.5 6.5 a4.5 4.5 0 1 1 -1.3 -3.2 M12 1.8 V4.2 H9.6"
                    stroke="currentColor"
                    stroke-width="1.3"
                    fill="none"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                </svg>
              }
            >
              Re-run failures
            </Button>
            <Button
              kind="primary"
              size="md"
              onClick={props.onRunAll}
              kbd="⌘R"
              icon={
                <svg width="11" height="11" viewBox="0 0 12 12">
                  <path d="M3.5 2.5 L9.5 6 L3.5 9.5 z" fill="currentColor" />
                </svg>
              }
            >
              Run all
            </Button>
          </>
        }
      >
        <Button
          kind="danger"
          size="md"
          onClick={props.onCancel}
          kbd="⌘."
          icon={
            <svg width="11" height="11" viewBox="0 0 12 12">
              <rect x="3" y="3" width="6" height="6" fill="currentColor" rx="0.8" />
            </svg>
          }
        >
          Cancel
        </Button>
      </Show>
    </div>
  );
}
