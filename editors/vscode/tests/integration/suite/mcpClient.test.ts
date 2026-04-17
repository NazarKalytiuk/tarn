import * as assert from "assert";
import * as cp from "child_process";
import * as fs from "fs";
import * as path from "path";

/**
 * Integration test for the NAZ-279 MCP backend. Exercises the real
 * `target/debug/tarn-mcp` binary end-to-end: spawns the child process,
 * performs the MCP `initialize` handshake, issues a `tools/call` to
 * `tarn_list`, and asserts the wrapped JSON reply round-trips.
 *
 * The test is **opt-in** — gated behind the `TARN_MCP_E2E=1` environment
 * variable — because the MCP binary is a development-only artifact and
 * CI images that haven't built it yet should skip without failing. When
 * the env flag is off we print a single `[mcp-test] skipped` line and
 * move on.
 *
 * This test intentionally talks to the MCP server directly (spawn +
 * stdio JSON-RPC), not via `TarnMcpClient`, to give us a ground-truth
 * contract probe. If `tarn-mcp`'s wire format drifts, this test fails
 * fast with a message the engineer can act on — regardless of whether
 * the TypeScript client has also drifted.
 */

function locateMcpBinary(): string | undefined {
  const candidate = path.resolve(
    __dirname,
    "../../../../../../target/debug/tarn-mcp",
  );
  if (!fs.existsSync(candidate)) {
    return undefined;
  }
  return candidate;
}

interface JsonRpcResponse {
  jsonrpc: string;
  id: number | string | null;
  result?: unknown;
  error?: { code: number; message: string };
}

interface McpContentEnvelope {
  content?: Array<{ type: string; text?: string }>;
  isError?: boolean;
}

function parseTextContent<T>(envelope: unknown): T {
  const e = envelope as McpContentEnvelope | undefined;
  const text = e?.content?.find((c) => c.type === "text")?.text;
  assert.ok(text, "MCP result missing text content block");
  return JSON.parse(text!) as T;
}

describe("MCP backend E2E (NAZ-279)", () => {
  const shouldRun = process.env.TARN_MCP_E2E === "1";

  it("handshakes with tarn-mcp and round-trips tarn_list", async function () {
    this.timeout(30000);
    if (!shouldRun) {
      console.log("[mcp-test] skipped: TARN_MCP_E2E != 1");
      this.skip();
      return;
    }
    const bin = locateMcpBinary();
    if (!bin) {
      console.log("[mcp-test] skipped: target/debug/tarn-mcp missing");
      this.skip();
      return;
    }

    const fixtureWorkspace = path.resolve(
      __dirname,
      "../../fixtures/workspace",
    );
    const child = cp.spawn(bin, [], {
      cwd: fixtureWorkspace,
      stdio: ["pipe", "pipe", "pipe"],
    });

    const lines: string[] = [];
    let buffer = "";
    child.stdout.on("data", (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      let idx: number;
      while ((idx = buffer.indexOf("\n")) >= 0) {
        lines.push(buffer.slice(0, idx).trim());
        buffer = buffer.slice(idx + 1);
      }
    });

    const waitFor = async (id: number, timeoutMs = 5000): Promise<JsonRpcResponse> => {
      const started = Date.now();
      while (Date.now() - started < timeoutMs) {
        const match = lines.find((line) => {
          try {
            const parsed = JSON.parse(line) as JsonRpcResponse;
            return parsed.id === id;
          } catch {
            return false;
          }
        });
        if (match) return JSON.parse(match) as JsonRpcResponse;
        await new Promise((r) => setTimeout(r, 20));
      }
      throw new Error(`timed out waiting for response id=${id}`);
    };

    const send = (payload: unknown): void => {
      child.stdin.write(JSON.stringify(payload) + "\n");
    };

    try {
      // Step 1: initialize
      send({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: "2024-11-05",
          capabilities: {},
          clientInfo: { name: "mcp-integration-test", version: "0" },
        },
      });
      const initResp = await waitFor(1);
      assert.ok(initResp.result, "initialize returned no result");
      assert.strictEqual(initResp.error, undefined);

      // Step 2: send the MCP notifications/initialized handshake.
      send({ jsonrpc: "2.0", method: "notifications/initialized" });

      // Step 3: tools/call for tarn_list against a fixture file.
      const fixtureFile = path.resolve(
        fixtureWorkspace,
        "tests/dry.tarn.yaml",
      );
      assert.ok(fs.existsSync(fixtureFile), `fixture ${fixtureFile} missing`);
      send({
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: {
          name: "tarn_list",
          arguments: { cwd: fixtureWorkspace, path: fixtureFile },
        },
      });
      const listResp = await waitFor(2);
      assert.strictEqual(listResp.error, undefined);
      const result = parseTextContent<{ files: Array<{ file: string }> }>(
        listResp.result,
      );
      assert.ok(Array.isArray(result.files));
      assert.ok(result.files.length >= 1, "expected at least one file entry");
      assert.ok(
        result.files.some((f) => f.file.endsWith("dry.tarn.yaml")),
        `expected dry.tarn.yaml in result, got ${JSON.stringify(result.files)}`,
      );
    } finally {
      try {
        child.stdin.end();
      } catch {
        /* ignore */
      }
      child.kill();
    }
  });
});
