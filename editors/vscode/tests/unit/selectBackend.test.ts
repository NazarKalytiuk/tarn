import { describe, it, expect, beforeEach } from "vitest";
import {
  selectBackend,
  showMcpFallbackNoticeOnce,
  resetMcpFallbackNoticeLatch,
} from "../../src/backend/selectBackend";
import type { TarnBackend } from "../../src/backend/TarnBackend";
import type { TarnMcpClient } from "../../src/backend/TarnMcpClient";
import * as vscodeMock from "./__mocks__/vscode";

interface MockVscode {
  __setMockConfig(entries: Record<string, unknown>): void;
  __getShownInformationMessages(): string[];
  __clearShownInformationMessages(): void;
}

const mockApi = vscodeMock as unknown as MockVscode;

/**
 * Cast helper: our tests never invoke methods on the CLI backend, so we
 * feed in a minimal object typed as {@link TarnBackend} to satisfy the
 * signature without building a full stub.
 */
function stubCliBackend(): TarnBackend {
  return {} as TarnBackend;
}

/**
 * Produce a stand-in for {@link TarnMcpClient} that the factory can
 * return without spawning a real child process. The test never invokes
 * methods on it, so casting through `unknown` is safe.
 */
function stubMcpClient(): TarnMcpClient {
  return { dispose: () => {} } as unknown as TarnMcpClient;
}

describe("selectBackend: tarn.backend selection (NAZ-279)", () => {
  beforeEach(() => {
    mockApi.__setMockConfig({});
    mockApi.__clearShownInformationMessages();
    resetMcpFallbackNoticeLatch();
  });

  it("returns the CLI backend when tarn.backend is unset (default)", async () => {
    const cli = stubCliBackend();
    const factory = async (): Promise<TarnMcpClient> => {
      throw new Error("factory must not be invoked for cli backend");
    };
    const result = await selectBackend(cli, "/workspace", factory);
    expect(result.backend).toBe(cli);
    expect(result.mcpClient).toBeUndefined();
    expect(result.fellBack).toBe(false);
  });

  it("returns the MCP client when tarn.backend is mcp and the factory succeeds", async () => {
    mockApi.__setMockConfig({ "tarn.backend": "mcp" });
    const cli = stubCliBackend();
    const mcp = stubMcpClient();
    let factoryInvocations = 0;
    const factory = async (): Promise<TarnMcpClient> => {
      factoryInvocations++;
      return mcp;
    };
    const result = await selectBackend(cli, "/workspace", factory);
    expect(factoryInvocations).toBe(1);
    expect(result.backend).toBe(mcp);
    expect(result.mcpClient).toBe(mcp);
    expect(result.fellBack).toBe(false);
  });

  it("falls back to the CLI and marks fellBack=true when the factory throws", async () => {
    mockApi.__setMockConfig({ "tarn.backend": "mcp" });
    const cli = stubCliBackend();
    const factory = async (): Promise<TarnMcpClient> => {
      throw new Error("tarn-mcp binary not found");
    };
    const result = await selectBackend(cli, "/workspace", factory);
    expect(result.backend).toBe(cli);
    expect(result.mcpClient).toBeUndefined();
    expect(result.fellBack).toBe(true);
  });
});

describe("showMcpFallbackNoticeOnce: single-shot latch", () => {
  beforeEach(() => {
    mockApi.__clearShownInformationMessages();
    resetMcpFallbackNoticeLatch();
  });

  it("fires the information toast on first call", async () => {
    showMcpFallbackNoticeOnce();
    // showInformationMessage is async but we only care that it was
    // enqueued; flush the microtask queue to let the promise settle.
    await new Promise((r) => setImmediate(r));
    const messages = mockApi.__getShownInformationMessages();
    expect(messages).toHaveLength(1);
    expect(messages[0]).toContain("falling back to the CLI");
  });

  it("does not re-fire on subsequent calls within the same session", async () => {
    showMcpFallbackNoticeOnce();
    showMcpFallbackNoticeOnce();
    showMcpFallbackNoticeOnce();
    await new Promise((r) => setImmediate(r));
    expect(mockApi.__getShownInformationMessages()).toHaveLength(1);
  });

  it("re-fires after the latch is explicitly reset (test hook)", async () => {
    showMcpFallbackNoticeOnce();
    resetMcpFallbackNoticeLatch();
    showMcpFallbackNoticeOnce();
    await new Promise((r) => setImmediate(r));
    expect(mockApi.__getShownInformationMessages()).toHaveLength(2);
  });
});
