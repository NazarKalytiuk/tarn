import Button from "./atoms/Button";
import CodeBlock from "./atoms/CodeBlock";
import StatusDot from "./atoms/StatusDot";

export default function BinaryError(_props: { message?: string }) {
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
      <div style={{ width: "460px" }}>
        <div
          style={{
            display: "inline-flex",
            "align-items": "center",
            gap: "8px",
            padding: "4px 10px",
            "border-radius": "11px",
            background: "var(--failed-dim)",
            color: "var(--failed)",
            "font-size": "11.5px",
            "font-weight": 600,
            "letter-spacing": "0.04em",
            "text-transform": "uppercase",
            "margin-bottom": "14px",
          }}
        >
          <StatusDot status="failed" size={7} />
          Binary not found
        </div>
        <div
          style={{
            "font-size": "20px",
            "font-weight": 600,
            color: "var(--fg-primary)",
            "letter-spacing": "-0.01em",
            "margin-bottom": "6px",
          }}
        >
          Tarn Studio couldn't find the <span class="mono">tarn</span> binary.
        </div>
        <div
          style={{
            "font-size": "13.5px",
            color: "var(--fg-secondary)",
            "line-height": 1.55,
            "margin-bottom": "22px",
          }}
        >
          Studio shells out to the CLI to run tests. Install it, or point Studio at an
          existing binary in Settings.
        </div>
        <CodeBlock
          title="install"
          tokens={[
            { t: "p", v: "$ " },
            { t: "s", v: "brew install tarn-tools/tap/tarn" },
          ]}
          style={{ "margin-bottom": "12px" }}
        />
        <div style={{ display: "flex", gap: "8px", "margin-top": "16px" }}>
          <Button kind="primary" size="md">
            Locate binary…
          </Button>
          <Button kind="secondary" size="md">
            Open install docs
          </Button>
        </div>
      </div>
    </div>
  );
}
