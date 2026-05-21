import { createMemo, createSignal, For, Show } from "solid-js";
import type { ReportStep } from "../ipc/types";
import { runState, type StepState, type StepStatus } from "../stores/run";
import StatusDot, { StatusTag } from "./atoms/StatusDot";
import Button from "./atoms/Button";
import CodeBlock, { tokenizeJSON } from "./atoms/CodeBlock";
import { Chevron, Duration, methodFgBright, methodTint } from "./atoms/misc";
import { findReportStep } from "../utils/report";
import { parseStepName } from "../utils/stepName";
import { relativePath } from "../utils/relativePath";
import { projectState } from "../stores/project";

type Props = {
  selectedKey: string | null;
  selectedFile: string | null;
  selectedTest: string | null;
  selectedStepIndex: number | null;
  selectedLabel: string | null;
  onReRun: () => void;
};

export default function StepDetail(props: Props) {
  const live = createMemo(() =>
    props.selectedKey ? runState.steps[props.selectedKey] : undefined,
  );
  const report = createMemo<ReportStep | null>(() => {
    if (props.selectedFile == null || props.selectedStepIndex == null) return null;
    return findReportStep(
      runState.report,
      props.selectedFile,
      props.selectedTest,
      props.selectedStepIndex,
    );
  });
  const parsed = createMemo(() => parseStepName(props.selectedLabel ?? ""));
  const status = (): StepStatus => (live()?.status ?? "pending") as StepStatus;

  return (
    <Show
      when={props.selectedKey}
      fallback={
        <div
          style={{
            flex: 1,
            display: "flex",
            "align-items": "center",
            "justify-content": "center",
            color: "var(--fg-tertiary)",
            "font-size": "13px",
          }}
        >
          Select a step on the left
        </div>
      }
    >
      <div
        class="scroll"
        style={{ flex: 1, "min-height": 0, overflow: "auto" }}
      >
        <StepDetailHeader
          method={parsed().method}
          path={parsed().path}
          status={status()}
          durationMs={live()?.durationMs}
          location={report()?.location ?? null}
          fileFallback={props.selectedFile}
          onReRun={props.onReRun}
        />
        <Show
          when={status() === "failed"}
          fallback={
            <PassedSurface report={report()} live={live()} />
          }
        >
          <FailedSurface report={report()} live={live()} method={parsed().method} />
        </Show>
      </div>
    </Show>
  );
}

function StepDetailHeader(props: {
  method: string | null;
  path: string;
  status: StepStatus;
  durationMs?: number;
  location: { file: string; line: number; column: number } | null;
  fileFallback: string | null;
  onReRun: () => void;
}) {
  return (
    <div
      style={{
        padding: "18px 22px 14px",
        "border-bottom": "1px solid var(--line)",
      }}
    >
      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "10px",
          "margin-bottom": "8px",
        }}
      >
        <StatusDot status={props.status} size={11} />
        <StatusTag status={props.status} />
        <Show when={props.durationMs != null}>
          <Duration ms={props.durationMs} />
        </Show>
        <span style={{ color: "var(--fg-quaternary)" }}>·</span>
        <span
          style={{
            "font-family": "var(--font-mono)",
            "font-size": "11.5px",
            color: "var(--fg-tertiary)",
          }}
        >
          {relativePath(
            projectState.path,
            props.location?.file ?? props.fileFallback ?? "",
          )}
          <Show when={props.location?.line}>
            <span style={{ color: "var(--fg-quaternary)" }}>:</span>
            {props.location?.line}
          </Show>
        </span>
        <div style={{ flex: 1 }} />
        <Button
          kind="ghost"
          size="sm"
          title="Open YAML at this line"
          icon={
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
              <path
                d="M2.5 2.5 H6 M2.5 5 H9.5 M2.5 7.5 H7 M2.5 10 H8"
                stroke="currentColor"
                stroke-width="1.2"
                stroke-linecap="round"
              />
            </svg>
          }
        >
          Open YAML
        </Button>
        <Button
          kind="ghost"
          size="sm"
          title="Re-run this step"
          kbd="⌘⇧R"
          onClick={props.onReRun}
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
          Re-run
        </Button>
      </div>

      <div style={{ display: "flex", "align-items": "baseline", gap: "12px" }}>
        <Show when={props.method}>
          <span
            style={{
              "font-family": "var(--font-mono)",
              "font-size": "12.5px",
              "font-weight": 600,
              padding: "2px 7px",
              "border-radius": "4px",
              background: methodTint(props.method!),
              color: methodFgBright(props.method!),
            }}
          >
            {props.method}
          </span>
        </Show>
        <span
          style={{
            "font-family": "var(--font-mono)",
            "font-size": "17px",
            "font-weight": 500,
            color: "var(--fg-primary)",
            "letter-spacing": "-0.01em",
          }}
        >
          {props.path}
        </span>
      </div>
    </div>
  );
}

function FailedSurface(props: {
  report: ReportStep | null;
  live: StepState | undefined;
  method: string | null;
}) {
  const liveFailure = () => props.live?.assertionFailures[0];
  const reportFailure = () => props.report?.assertions?.failures?.[0];
  const failure = () => reportFailure() ?? liveFailure();

  const otherAssertions = () =>
    (props.report?.assertions?.details ?? []).filter((d) => d.passed);

  const assertionsTotal = () =>
    props.report?.assertions?.total ?? (1 + otherAssertions().length);

  return (
    <>
      <div style={{ padding: "18px 22px 8px" }}>
        <div
          style={{
            border: "1px solid var(--failed-dim)",
            background:
              "linear-gradient(180deg, var(--failed-dim) 0%, transparent 80%)",
            "border-radius": "var(--r-lg)",
            padding: "14px 16px 16px",
            position: "relative",
          }}
        >
          <div
            style={{
              display: "flex",
              "align-items": "center",
              gap: "8px",
              "margin-bottom": "10px",
            }}
          >
            <span
              style={{
                "font-size": "10.5px",
                "font-weight": 600,
                "letter-spacing": "0.06em",
                "text-transform": "uppercase",
                color: "var(--failed)",
              }}
            >
              Assertion failed
            </span>
            <Show when={failure()?.assertion}>
              <span style={{ color: "var(--fg-quaternary)" }}>·</span>
              <span
                style={{
                  "font-family": "var(--font-mono)",
                  "font-size": "11.5px",
                  color: "var(--fg-secondary)",
                }}
              >
                {failure()!.assertion}
              </span>
            </Show>
            <div style={{ flex: 1 }} />
            <span style={{ "font-size": "11px", color: "var(--fg-tertiary)" }}>
              1 of {assertionsTotal()} assertions
            </span>
          </div>

          <Show when={failure()?.message}>
            <div
              style={{
                "font-size": "14px",
                color: "var(--fg-primary)",
                "margin-bottom": "14px",
                "line-height": 1.45,
              }}
            >
              {failure()!.message}
            </div>
          </Show>

          <div
            style={{
              display: "grid",
              "grid-template-columns": "1fr 1fr",
              gap: "10px",
            }}
          >
            <ExpectedActualBlock
              label="Expected"
              value={failure()?.expected ?? "—"}
              kind="good"
            />
            <ExpectedActualBlock
              label="Actual"
              value={failure()?.actual ?? "—"}
              kind="bad"
            />
          </div>
        </div>

        <Show when={otherAssertions().length > 0}>
          <div style={{ "margin-top": "14px", "padding-left": "4px" }}>
            <For each={otherAssertions()}>
              {(a) => (
                <div
                  style={{
                    display: "flex",
                    "align-items": "center",
                    gap: "10px",
                    padding: "4px 0",
                    "font-size": "12.5px",
                    color: "var(--fg-secondary)",
                  }}
                >
                  <StatusDot status="passed" size={7} />
                  <span
                    style={{
                      "font-family": "var(--font-mono)",
                      color: "var(--fg-secondary)",
                    }}
                  >
                    {a.assertion}
                  </span>
                  <Show when={a.message}>
                    <span style={{ color: "var(--fg-tertiary)" }}>{a.message}</span>
                  </Show>
                </div>
              )}
            </For>
          </div>
        </Show>
      </div>

      <Show when={props.report?.request}>
        <SectionHeader title="Request" right={<CopyCurlButton />} />
        <div style={{ padding: "0 22px 14px" }}>
          <div
            style={{
              display: "flex",
              "align-items": "center",
              gap: "8px",
              padding: "8px 12px",
              background: "var(--bg-input)",
              border: "1px solid var(--line)",
              "border-radius": "var(--r-md)",
              "margin-bottom": "10px",
            }}
          >
            <span
              style={{
                "font-family": "var(--font-mono)",
                "font-size": "11.5px",
                "font-weight": 600,
                color: methodFgBright(props.report!.request!.method ?? "GET"),
              }}
            >
              {(props.report!.request!.method ?? "GET").toUpperCase()}
            </span>
            <span
              style={{
                "font-family": "var(--font-mono)",
                "font-size": "12px",
                color: "var(--fg-primary)",
                overflow: "hidden",
                "text-overflow": "ellipsis",
                "white-space": "nowrap",
              }}
            >
              {props.report!.request!.url}
            </span>
          </div>
          <Show when={props.report!.request!.headers}>
            <HeaderTable headers={props.report!.request!.headers!} />
          </Show>
          <Show when={props.report!.request!.body != null}>
            <div style={{ "margin-top": "10px" }}>
              <CodeBlock
                title="body · application/json"
                tokens={tokenizeJSON(stringifyBody(props.report!.request!.body))}
              />
            </div>
          </Show>
        </div>
      </Show>

      <Show when={props.report?.response}>
        <SectionHeader
          title="Response"
          right={
            <span
              style={{
                display: "inline-flex",
                "align-items": "center",
                gap: "8px",
                "font-size": "11.5px",
              }}
            >
              <span
                style={{
                  "font-family": "var(--font-mono)",
                  padding: "2px 7px",
                  "border-radius": "4px",
                  background: "var(--failed-dim)",
                  color: "var(--failed)",
                  "font-weight": 600,
                }}
              >
                {props.report!.response!.status}
              </span>
              <Duration ms={props.live?.durationMs} />
            </span>
          }
        />
        <div style={{ padding: "0 22px 18px" }}>
          <Show when={props.report!.response!.headers}>
            <HeaderTable headers={props.report!.response!.headers!} />
          </Show>
          <Show when={props.report!.response!.body != null}>
            <div style={{ "margin-top": "12px" }}>
              <CodeBlock
                title="body · application/json"
                tokens={tokenizeJSON(stringifyBody(props.report!.response!.body))}
                height={220}
              />
            </div>
          </Show>
        </div>
      </Show>

      <Show
        when={
          props.report?.remediation_hints &&
          props.report.remediation_hints.length > 0
        }
      >
        <div style={{ padding: "0 22px 22px" }}>
          <div
            style={{
              padding: "10px 14px",
              "border-top": "1px dashed var(--line)",
              background: "transparent",
            }}
          >
            <div
              style={{
                "font-size": "10.5px",
                "font-weight": 600,
                "letter-spacing": "0.06em",
                "text-transform": "uppercase",
                color: "var(--fg-tertiary)",
                "margin-bottom": "6px",
              }}
            >
              Hints
            </div>
            <For each={props.report!.remediation_hints}>
              {(h) => (
                <div
                  style={{
                    "font-size": "12.5px",
                    color: "var(--fg-secondary)",
                    "line-height": 1.5,
                    padding: "2px 0",
                    display: "flex",
                    gap: "8px",
                  }}
                >
                  <span style={{ color: "var(--fg-quaternary)" }}>·</span>
                  <span>{h}</span>
                </div>
              )}
            </For>
          </div>
        </div>
      </Show>
    </>
  );
}

function PassedSurface(props: {
  report: ReportStep | null;
  live: StepState | undefined;
}) {
  const [showReq, setShowReq] = createSignal(false);
  const [showResp, setShowResp] = createSignal(false);

  const passedAssertions = () =>
    (props.report?.assertions?.details ?? []).filter((d) => d.passed);

  return (
    <>
      <div style={{ padding: "18px 22px 6px" }}>
        <div
          style={{
            border: "1px solid var(--passed-dim)",
            background:
              "linear-gradient(180deg, var(--passed-dim) 0%, transparent 80%)",
            "border-radius": "var(--r-lg)",
            padding: "14px 16px",
          }}
        >
          <div
            style={{
              display: "flex",
              "align-items": "center",
              gap: "8px",
              "margin-bottom": "8px",
            }}
          >
            <span
              style={{
                "font-size": "10.5px",
                "font-weight": 600,
                "letter-spacing": "0.06em",
                "text-transform": "uppercase",
                color: "var(--passed)",
              }}
            >
              <Show
                when={props.live?.status === "passed"}
                fallback="Step ready"
              >
                All assertions passed
              </Show>
            </span>
            <div style={{ flex: 1 }} />
            <Show when={props.report?.assertions}>
              <span style={{ "font-size": "11px", color: "var(--fg-tertiary)" }}>
                {props.report!.assertions!.passed} of {props.report!.assertions!.total}
              </span>
            </Show>
          </div>
          <div style={{ display: "flex", "flex-direction": "column", gap: "4px" }}>
            <For
              each={passedAssertions()}
              fallback={
                <div
                  style={{
                    "font-size": "12.5px",
                    color: "var(--fg-tertiary)",
                  }}
                >
                  No assertion detail recorded.
                </div>
              }
            >
              {(a) => (
                <div
                  style={{
                    display: "flex",
                    "align-items": "center",
                    gap: "10px",
                    "font-size": "12.5px",
                  }}
                >
                  <StatusDot status="passed" size={7} />
                  <span
                    style={{
                      "font-family": "var(--font-mono)",
                      color: "var(--fg-secondary)",
                    }}
                  >
                    {a.assertion}
                  </span>
                  <Show when={a.message}>
                    <span style={{ color: "var(--fg-tertiary)" }}>{a.message}</span>
                  </Show>
                </div>
              )}
            </For>
          </div>
        </div>
      </div>

      <Show when={props.report?.request}>
        <div style={{ padding: "14px 22px 0" }}>
          <CollapsibleRow
            open={showReq()}
            onToggle={() => setShowReq(!showReq())}
            title="Request"
            right={
              <span
                style={{
                  display: "inline-flex",
                  "align-items": "center",
                  gap: "6px",
                  "font-family": "var(--font-mono)",
                  "font-size": "11.5px",
                  color: "var(--fg-tertiary)",
                }}
              >
                <span style={{ color: methodFgBright(props.report!.request!.method ?? "GET") }}>
                  {(props.report!.request!.method ?? "GET").toUpperCase()}
                </span>
                {(props.report!.request!.url ?? "").replace("https://", "")}
              </span>
            }
          />
          <Show when={showReq()}>
            <div style={{ "padding-top": "8px", "padding-bottom": "12px" }}>
              <Show when={props.report!.request!.headers}>
                <HeaderTable headers={props.report!.request!.headers!} />
              </Show>
              <Show when={props.report!.request!.body != null}>
                <div style={{ "margin-top": "10px" }}>
                  <CodeBlock
                    title="body · application/json"
                    tokens={tokenizeJSON(stringifyBody(props.report!.request!.body))}
                  />
                </div>
              </Show>
            </div>
          </Show>

          <CollapsibleRow
            open={showResp()}
            onToggle={() => setShowResp(!showResp())}
            title="Response"
            right={
              <span
                style={{
                  display: "inline-flex",
                  "align-items": "center",
                  gap: "8px",
                  "font-family": "var(--font-mono)",
                  "font-size": "11.5px",
                  color: "var(--fg-tertiary)",
                }}
              >
                <Show when={props.report?.response?.status}>
                  <span style={{ color: "var(--passed)", "font-weight": 600 }}>
                    {props.report!.response!.status}
                  </span>
                </Show>
                <Duration ms={props.live?.durationMs} />
              </span>
            }
          />
          <Show when={showResp() && props.report?.response}>
            <div style={{ "padding-top": "8px", "padding-bottom": "12px" }}>
              <Show when={props.report!.response!.headers}>
                <HeaderTable headers={props.report!.response!.headers!} />
              </Show>
              <Show when={props.report!.response!.body != null}>
                <div style={{ "margin-top": "10px" }}>
                  <CodeBlock
                    title="body · application/json"
                    tokens={tokenizeJSON(stringifyBody(props.report!.response!.body))}
                    height={200}
                  />
                </div>
              </Show>
            </div>
          </Show>
        </div>
      </Show>
    </>
  );
}

function ExpectedActualBlock(props: { label: string; value: string; kind: "good" | "bad" }) {
  const color = props.kind === "good" ? "var(--passed)" : "var(--failed)";
  const tint = props.kind === "good" ? "var(--passed-dim)" : "var(--failed-dim)";
  return (
    <div
      style={{
        background: tint,
        border: `1px solid ${tint}`,
        "border-radius": "var(--r-md)",
        padding: "10px 12px",
      }}
    >
      <div
        style={{
          "font-size": "10.5px",
          "font-weight": 600,
          "letter-spacing": "0.06em",
          "text-transform": "uppercase",
          color,
          "margin-bottom": "4px",
        }}
      >
        {props.label}
      </div>
      <div
        style={{
          "font-family": "var(--font-mono)",
          "font-size": "16px",
          "font-weight": 500,
          color: "var(--fg-primary)",
        }}
      >
        {props.value}
      </div>
    </div>
  );
}

function SectionHeader(props: { title: string; right?: any }) {
  return (
    <div
      style={{
        display: "flex",
        "align-items": "center",
        "justify-content": "space-between",
        padding: "14px 22px 8px",
        "border-top": "1px solid var(--line-faint)",
      }}
    >
      <span
        style={{
          "font-size": "11px",
          "font-weight": 600,
          "letter-spacing": "0.08em",
          "text-transform": "uppercase",
          color: "var(--fg-tertiary)",
        }}
      >
        {props.title}
      </span>
      {props.right}
    </div>
  );
}

function HeaderTable(props: { headers: Record<string, string> }) {
  const rows = createMemo(() => Object.entries(props.headers));
  return (
    <div
      style={{
        background: "var(--bg-input)",
        border: "1px solid var(--line)",
        "border-radius": "var(--r-md)",
        overflow: "hidden",
      }}
    >
      <For each={rows()}>
        {([k, v], i) => (
          <div
            style={{
              display: "grid",
              "grid-template-columns": "180px 1fr",
              gap: "10px",
              padding: "6px 12px",
              "border-top": i() ? "1px solid var(--line-faint)" : "none",
              "font-family": "var(--font-mono)",
              "font-size": "12px",
            }}
          >
            <span style={{ color: "var(--fg-tertiary)" }}>{k}</span>
            <span
              style={{
                color: "var(--fg-primary)",
                overflow: "hidden",
                "text-overflow": "ellipsis",
                "white-space": "nowrap",
              }}
            >
              {v}
            </span>
          </div>
        )}
      </For>
    </div>
  );
}

function CollapsibleRow(props: {
  open: boolean;
  onToggle: () => void;
  title: string;
  right?: any;
}) {
  return (
    <div
      onClick={props.onToggle}
      style={{
        display: "flex",
        "align-items": "center",
        gap: "10px",
        padding: "8px 4px 8px 2px",
        "border-top": "1px solid var(--line-faint)",
        cursor: "pointer",
      }}
    >
      <Chevron open={props.open} />
      <span
        style={{
          "font-size": "11px",
          "font-weight": 600,
          "letter-spacing": "0.08em",
          "text-transform": "uppercase",
          color: "var(--fg-tertiary)",
        }}
      >
        {props.title}
      </span>
      <div style={{ flex: 1 }} />
      {props.right}
    </div>
  );
}

function CopyCurlButton() {
  return (
    <Button
      kind="ghost"
      size="sm"
      icon={
        <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
          <path
            d="M3 6 L5 8 L9 4"
            stroke="currentColor"
            stroke-width="1.2"
            fill="none"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      }
    >
      Copy as curl
    </Button>
  );
}

function stringifyBody(body: unknown): string {
  if (typeof body === "string") return body;
  try {
    return JSON.stringify(body, null, 2);
  } catch {
    return String(body);
  }
}
