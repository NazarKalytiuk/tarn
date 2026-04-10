import { describe, it, expect } from "vitest";
import { BRIDGE_MARKER, injectReportBridge } from "../../src/views/ReportWebview";

const SAMPLE_HTML = `<!DOCTYPE html>
<html><head><title>Tarn</title></head>
<body>
<script>const DATA = {"files":[]};</script>
<div id="app"></div>
</body></html>`;

describe("injectReportBridge", () => {
  it("inserts the bridge script immediately before </body>", () => {
    const out = injectReportBridge(SAMPLE_HTML);
    const bridgePos = out.indexOf(BRIDGE_MARKER);
    const bodyClose = out.indexOf("</body>");
    expect(bridgePos).toBeGreaterThan(0);
    expect(bodyClose).toBeGreaterThan(bridgePos);
  });

  it("emits a script that walks .file-card and .test-group nodes", () => {
    const out = injectReportBridge(SAMPLE_HTML);
    expect(out).toContain("acquireVsCodeApi");
    expect(out).toContain(".file-card[data-file]");
    expect(out).toContain(".test-group[data-test]");
    expect(out).toContain('"jumpTo"');
  });

  it("preserves the original DATA and app div", () => {
    const out = injectReportBridge(SAMPLE_HTML);
    expect(out).toContain('const DATA = {"files":[]};');
    expect(out).toContain('<div id="app"></div>');
  });

  it("is idempotent: running twice does not double-inject", () => {
    const once = injectReportBridge(SAMPLE_HTML);
    const twice = injectReportBridge(once);
    const firstIndex = twice.indexOf(BRIDGE_MARKER);
    const secondIndex = twice.indexOf(BRIDGE_MARKER, firstIndex + 1);
    expect(firstIndex).toBeGreaterThanOrEqual(0);
    expect(secondIndex).toBe(-1);
  });

  it("falls back to appending when </body> is missing", () => {
    const malformed = `<html><body><div>hi</div>`;
    const out = injectReportBridge(malformed);
    expect(out).toContain(BRIDGE_MARKER);
    expect(out.trim().endsWith("</script>")).toBe(true);
  });

  it("only clicks the step body, not nested expand/collapse controls", () => {
    const out = injectReportBridge(SAMPLE_HTML);
    // The bridge must bail out on clicks inside these selectors so
    // users can still expand assertion details and request/response
    // panels without accidentally jumping to source.
    expect(out).toContain(".assert-details-toggle");
    expect(out).toContain(".req-resp-toggle");
    expect(out).toContain(".assert-detail");
  });
});
