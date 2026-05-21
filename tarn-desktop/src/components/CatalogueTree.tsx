import { createSignal, For, type JSX, Show } from "solid-js";
import type { ListedFile, ListedStep, ListedTest } from "../ipc/types";
import { projectState } from "../stores/project";
import { runState, stepKey, type StepStatus } from "../stores/run";
import { toolbarState } from "../stores/toolbar";
import StatusDot, { StatusTag } from "./atoms/StatusDot";
import { Chevron, Duration, MethodTag } from "./atoms/misc";
import { parseStepName } from "../utils/stepName";
import { relativePath } from "../utils/relativePath";
import {
  fileStatus,
  filePassedCount,
  testPassedCount,
  testStatus,
} from "../utils/status";

export type StepSelection = {
  key: string;
  file: string;
  test: string | null;
  stepIndex: number;
  label: string;
};

type Props = {
  selectedKey: string | null;
  focusFailures: boolean;
  busy: boolean;
  onSelect: (sel: StepSelection) => void;
  onRunFile: (file: string) => void;
  onRunTest: (file: string, test: string) => void;
  onRunStep: (file: string, test: string | null, stepIndex: number) => void;
};

export default function CatalogueTree(props: Props) {
  const filtered = () => {
    const tags = toolbarState.activeTags;
    if (tags.length === 0) return projectState.files;
    return projectState.files.filter((f) =>
      tags.every((t) => (f.tags ?? []).includes(t)),
    );
  };

  const totalSteps = () =>
    projectState.files.reduce(
      (acc, f) =>
        acc +
        f.steps.length +
        f.tests.reduce((tt, t) => tt + t.steps.length, 0),
      0,
    );

  return (
    <div
      class="scroll"
      style={{
        flex: 1,
        padding: "6px 0 24px",
        "min-height": 0,
        overflow: "auto",
      }}
    >
      <div
        style={{
          padding: "8px 14px 6px",
          display: "flex",
          "align-items": "center",
          "justify-content": "space-between",
        }}
      >
        <span
          style={{
            "font-size": "11px",
            color: "var(--fg-tertiary)",
            "text-transform": "uppercase",
            "letter-spacing": "0.06em",
            "font-weight": 600,
          }}
        >
          Catalogue
        </span>
        <span
          style={{
            "font-size": "11px",
            color: "var(--fg-tertiary)",
            "font-variant-numeric": "tabular-nums",
          }}
        >
          {projectState.files.length} files · {totalSteps()} steps
        </span>
      </div>

      <Show
        when={filtered().length > 0}
        fallback={
          <div
            style={{
              padding: "12px 14px",
              color: "var(--fg-tertiary)",
              "font-size": "12.5px",
            }}
          >
            <Show
              when={projectState.loading}
              fallback={
                <Show
                  when={projectState.files.length === 0}
                  fallback="No files match the active tag filter."
                >
                  No .tarn.yaml files found.
                </Show>
              }
            >
              Discovering…
            </Show>
          </div>
        }
      >
        <For each={filtered()}>
          {(file) => (
            <FileNode
              file={file}
              focusFailures={props.focusFailures}
              selectedKey={props.selectedKey}
              busy={props.busy}
              onSelect={props.onSelect}
              onRunFile={props.onRunFile}
              onRunTest={props.onRunTest}
              onRunStep={props.onRunStep}
            />
          )}
        </For>
      </Show>
    </div>
  );
}

function FileNode(props: {
  file: ListedFile;
  focusFailures: boolean;
  selectedKey: string | null;
  busy: boolean;
  onSelect: (sel: StepSelection) => void;
  onRunFile: (file: string) => void;
  onRunTest: (file: string, test: string) => void;
  onRunStep: (file: string, test: string | null, stepIndex: number) => void;
}) {
  const [open, setOpen] = createSignal(true);
  const status = () => fileStatus(props.file);
  const counts = () => filePassedCount(props.file);
  const hint = () => relativePath(projectState.path, props.file.file);

  return (
    <div>
      <TreeRow
        indent={0}
        statusDot={status()}
        bold
        chevron={open()}
        onToggleChevron={() => setOpen(!open())}
        label={props.file.name}
        hint={hint()}
        right={
          <span
            style={{
              "font-size": "10.5px",
              color: "var(--fg-tertiary)",
              "font-family": "var(--font-mono)",
              "font-variant-numeric": "tabular-nums",
            }}
          >
            {counts().passed}/{counts().total}
          </span>
        }
        runDisabled={props.busy}
        onRun={() => props.onRunFile(props.file.file)}
        runTitle={`Run ${props.file.name}`}
      />

      <Show when={open()}>
        <For each={props.file.tests}>
          {(test) => (
            <TestNode
              file={props.file.file}
              test={test}
              focusFailures={props.focusFailures}
              selectedKey={props.selectedKey}
              busy={props.busy}
              onSelect={props.onSelect}
              onRunTest={props.onRunTest}
              onRunStep={props.onRunStep}
            />
          )}
        </For>
        <For each={props.file.steps}>
          {(step, idx) => (
            <StepRow
              file={props.file.file}
              test={null}
              step={step}
              index={idx()}
              focusFailures={props.focusFailures}
              selectedKey={props.selectedKey}
              busy={props.busy}
              onSelect={props.onSelect}
              onRunStep={props.onRunStep}
            />
          )}
        </For>
      </Show>
    </div>
  );
}

function TestNode(props: {
  file: string;
  test: ListedTest;
  focusFailures: boolean;
  selectedKey: string | null;
  busy: boolean;
  onSelect: (sel: StepSelection) => void;
  onRunTest: (file: string, test: string) => void;
  onRunStep: (file: string, test: string | null, stepIndex: number) => void;
}) {
  const [open, setOpen] = createSignal(true);
  const status = () => testStatus(props.file, props.test);
  const counts = () => testPassedCount(props.file, props.test);

  return (
    <Show
      when={
        !props.focusFailures || status() === "failed" || status() === "running"
      }
    >
      <TreeRow
        indent={1}
        statusDot={status()}
        chevron={open()}
        onToggleChevron={() => setOpen(!open())}
        label={props.test.name}
        right={
          <span
            style={{
              "font-size": "10.5px",
              color: "var(--fg-tertiary)",
              "font-family": "var(--font-mono)",
              "font-variant-numeric": "tabular-nums",
            }}
          >
            {counts().passed}/{counts().total}
          </span>
        }
        runDisabled={props.busy}
        onRun={() => props.onRunTest(props.file, props.test.name)}
        runTitle={`Run ${props.test.name}`}
      />
      <Show when={open()}>
        <For each={props.test.steps}>
          {(step, idx) => (
            <StepRow
              file={props.file}
              test={props.test.name}
              step={step}
              index={idx()}
              focusFailures={props.focusFailures}
              selectedKey={props.selectedKey}
              busy={props.busy}
              onSelect={props.onSelect}
              onRunStep={props.onRunStep}
            />
          )}
        </For>
      </Show>
    </Show>
  );
}

function StepRow(props: {
  file: string;
  test: string | null;
  step: ListedStep;
  index: number;
  focusFailures: boolean;
  selectedKey: string | null;
  busy: boolean;
  onSelect: (sel: StepSelection) => void;
  onRunStep: (file: string, test: string | null, stepIndex: number) => void;
}) {
  const key = () => stepKey(props.file, props.test, props.index);
  const state = () => runState.steps[key()];
  const status = (): StepStatus => (state()?.status ?? "pending") as StepStatus;
  const isSelected = () => props.selectedKey === key();
  const parsed = () => parseStepName(props.step.name);

  return (
    <Show
      when={
        !props.focusFailures ||
        status() === "failed" ||
        status() === "running"
      }
    >
      <TreeRow
        indent={2}
        statusDot={status()}
        selected={isSelected()}
        onClick={() =>
          props.onSelect({
            key: key(),
            file: props.file,
            test: props.test,
            stepIndex: props.index,
            label: props.step.name,
          })
        }
        labelEl={
          <span
            style={{
              display: "inline-flex",
              "align-items": "center",
              gap: "8px",
              "min-width": 0,
              flex: 1,
            }}
          >
            <Show
              when={parsed().method}
              fallback={
                <span
                  style={{
                    "font-family": "var(--font-mono)",
                    "font-size": "12px",
                    color:
                      status() === "pending" || status() === "skipped"
                        ? "var(--fg-tertiary)"
                        : "var(--fg-primary)",
                    overflow: "hidden",
                    "text-overflow": "ellipsis",
                    "white-space": "nowrap",
                  }}
                >
                  {parsed().path}
                </span>
              }
            >
              <MethodTag
                method={parsed().method!}
                dim={status() === "pending" || status() === "skipped"}
              />
              <span
                style={{
                  "font-family": "var(--font-mono)",
                  "font-size": "12px",
                  color:
                    status() === "pending" || status() === "skipped"
                      ? "var(--fg-tertiary)"
                      : "var(--fg-primary)",
                  overflow: "hidden",
                  "text-overflow": "ellipsis",
                  "white-space": "nowrap",
                }}
              >
                {parsed().path}
              </span>
            </Show>
          </span>
        }
        right={
          status() === "running" ? (
            <span
              style={{
                "font-size": "10.5px",
                color: "var(--running)",
                "font-family": "var(--font-mono)",
                "letter-spacing": "0.04em",
              }}
            >
              RUNNING
            </span>
          ) : state()?.durationMs != null ? (
            <Duration ms={state()!.durationMs} />
          ) : status() === "skipped" ? (
            <StatusTag status="skipped" />
          ) : null
        }
        runDisabled={props.busy}
        onRun={() =>
          props.onRunStep(props.file, props.test, props.index)
        }
        runTitle={`Run ${props.step.name}`}
      />
    </Show>
  );
}

function TreeRow(props: {
  indent?: number;
  statusDot: StepStatus;
  chevron?: boolean;
  onToggleChevron?: () => void;
  label?: string;
  labelEl?: JSX.Element;
  hint?: string;
  right?: JSX.Element;
  bold?: boolean;
  selected?: boolean;
  onClick?: () => void;
  onRun?: () => void;
  runDisabled?: boolean;
  runTitle?: string;
}) {
  const [hover, setHover] = createSignal(false);
  const padLeft = () => 8 + (props.indent ?? 0) * 14;

  function onRunClick(e: MouseEvent) {
    e.stopPropagation();
    if (props.onRun && !props.runDisabled) props.onRun();
  }

  return (
    <div
      onClick={props.onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      style={{
        display: "flex",
        "align-items": "center",
        gap: "8px",
        padding: `4px 10px 4px ${padLeft()}px`,
        "min-height": "26px",
        background: props.selected
          ? "var(--bg-selected)"
          : hover()
            ? "var(--bg-elevated)"
            : "transparent",
        position: "relative",
        cursor: props.onClick ? "pointer" : "default",
        "border-left": props.selected
          ? "2px solid var(--accent)"
          : "2px solid transparent",
      }}
    >
      <span
        onClick={(e) => {
          if (props.onToggleChevron) {
            e.stopPropagation();
            props.onToggleChevron();
          }
        }}
        style={{
          width: "12px",
          display: "inline-flex",
          "justify-content": "center",
          cursor: props.onToggleChevron ? "pointer" : "default",
          "flex-shrink": 0,
        }}
      >
        <Show when={props.chevron !== undefined}>
          <Chevron open={props.chevron} />
        </Show>
      </span>
      <StatusDot status={props.statusDot} size={9} />
      <Show
        when={props.labelEl}
        fallback={
          <span
            style={{
              display: "inline-flex",
              "align-items": "center",
              gap: "8px",
              "min-width": 0,
              flex: 1,
            }}
          >
            <span
              style={{
                "font-size": "13px",
                "font-weight": props.bold ? 600 : 500,
                color: "var(--fg-primary)",
                overflow: "hidden",
                "text-overflow": "ellipsis",
                "white-space": "nowrap",
              }}
            >
              {props.label}
            </span>
            <Show when={props.hint}>
              <span
                style={{
                  "font-size": "11.5px",
                  color: "var(--fg-tertiary)",
                  "font-family": "var(--font-mono)",
                }}
              >
                {props.hint}
              </span>
            </Show>
          </span>
        }
      >
        {props.labelEl}
      </Show>
      <Show when={hover() && props.onRun}>
        <button
          type="button"
          title={props.runTitle ?? "Run"}
          disabled={props.runDisabled}
          onClick={onRunClick}
          aria-label={props.runTitle ?? "Run"}
          style={{
            width: "20px",
            height: "20px",
            "border-radius": "4px",
            border: "1px solid transparent",
            background: "transparent",
            color: props.runDisabled
              ? "var(--fg-quaternary)"
              : "var(--fg-tertiary)",
            display: "inline-flex",
            "align-items": "center",
            "justify-content": "center",
            cursor: props.runDisabled ? "not-allowed" : "pointer",
            padding: 0,
            "flex-shrink": 0,
            transition: "background 120ms, color 120ms",
          }}
          onMouseEnter={(e) => {
            if (props.runDisabled) return;
            e.currentTarget.style.background = "var(--bg-surface)";
            e.currentTarget.style.color = "var(--accent)";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = "transparent";
            e.currentTarget.style.color = props.runDisabled
              ? "var(--fg-quaternary)"
              : "var(--fg-tertiary)";
          }}
        >
          <svg width="9" height="9" viewBox="0 0 12 12">
            <path d="M3.5 2.5 L9.5 6 L3.5 9.5 z" fill="currentColor" />
          </svg>
        </button>
      </Show>
      {props.right}
    </div>
  );
}
