import type { ReportRequest } from "../ipc/types";

/**
 * Render a `curl` command equivalent to the recorded request. The
 * output is multi-line with `\\` continuations so it's readable in a
 * code block and pastable into a shell. Headers and body are shown
 * verbatim — redaction is the CLI's responsibility before the values
 * land in the report.
 */
export function buildCurl(req: ReportRequest | undefined | null): string {
  if (!req || !req.url) return "";
  const lines: string[] = [];
  const method = (req.method ?? "GET").toUpperCase();
  lines.push(`curl -X ${shellQuote(method)} ${shellQuote(req.url)}`);

  if (req.headers) {
    for (const [k, v] of Object.entries(req.headers)) {
      lines.push(`  -H ${shellQuote(`${k}: ${v}`)}`);
    }
  }

  if (req.body !== undefined && req.body !== null) {
    const body = typeof req.body === "string" ? req.body : JSON.stringify(req.body);
    lines.push(`  --data ${shellQuote(body)}`);
  }

  return lines.join(" \\\n");
}

function shellQuote(s: string): string {
  if (!/[\s'"\\$`!*?]/.test(s)) return s;
  return `'${s.replace(/'/g, "'\\''")}'`;
}
