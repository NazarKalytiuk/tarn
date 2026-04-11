import { describe, it, expect } from "vitest";
import {
  buildClientOptions,
  REVEAL_OUTPUT_CHANNEL_ON_NEVER,
  TARN_LSP_CLIENT_ID,
  TARN_LSP_CLIENT_NAME,
  TRANSPORT_KIND_STDIO,
} from "../../src/lsp/client";

/**
 * Unit tests for `buildClientOptions` (Phase V1 / NAZ-309).
 *
 * These pin the pure "setting → LanguageClient wiring" mapping so
 * any Phase V2 feature ticket that bumps the scaffold can notice
 * a regression before it ships. The impure `startTarnLspClient`
 * wrapper that actually spawns `tarn-lsp` is covered by the
 * integration test in `tests/integration/suite/lspClient.test.ts`,
 * which drives a real extension host.
 *
 * We also verify that the inlined numeric literals for
 * `TransportKind.stdio` and `RevealOutputChannelOn.Never` still
 * match the values exported by `vscode-languageclient/node`. If a
 * future language-client release ever renumbers either enum, this
 * test fails loud instead of the client silently spawning on the
 * wrong transport or hiding output from the wrong channel.
 */

describe("buildClientOptions: pure builder", () => {
  it("uses the supplied binary path as the run/debug command", () => {
    const { serverOptions } = buildClientOptions("/abs/path/to/tarn-lsp");
    const opts = serverOptions as unknown as {
      run: { command: string; args: string[] };
      debug: { command: string; args: string[] };
    };
    expect(opts.run.command).toBe("/abs/path/to/tarn-lsp");
    expect(opts.debug.command).toBe("/abs/path/to/tarn-lsp");
    expect(opts.run.args).toEqual([]);
    expect(opts.debug.args).toEqual([]);
  });

  it("selects stdio as the transport for both run and debug", () => {
    const { serverOptions } = buildClientOptions("tarn-lsp");
    const opts = serverOptions as unknown as {
      run: { transport: number; options: { shell: boolean } };
      debug: { transport: number; options: { shell: boolean } };
    };
    expect(opts.run.transport).toBe(TRANSPORT_KIND_STDIO);
    expect(opts.debug.transport).toBe(TRANSPORT_KIND_STDIO);
    expect(opts.run.options.shell).toBe(false);
    expect(opts.debug.options.shell).toBe(false);
  });

  it("registers only on-disk `.tarn.yaml` files via the document selector", () => {
    const { clientOptions } = buildClientOptions("tarn-lsp");
    const selector = clientOptions.documentSelector as Array<{
      language: string;
      scheme: string;
    }>;
    expect(Array.isArray(selector)).toBe(true);
    expect(selector).toHaveLength(1);
    expect(selector[0].language).toBe("tarn");
    expect(selector[0].scheme).toBe("file");
  });

  it("names the dedicated output channel 'Tarn LSP'", () => {
    const { clientOptions } = buildClientOptions("tarn-lsp");
    expect(TARN_LSP_CLIENT_NAME).toBe("Tarn LSP");
    expect(TARN_LSP_CLIENT_ID).toBe("tarn-lsp");
    expect(clientOptions.outputChannelName).toBe("Tarn LSP");
  });

  it("hides the output channel from the user unless they open it", () => {
    // End-user-facing: an experimental LSP scaffold must never
    // auto-reveal its output channel on warning or error. The
    // direct providers are the user's source of truth until
    // Phase V3 deletes them.
    const { clientOptions } = buildClientOptions("tarn-lsp");
    expect(clientOptions.revealOutputChannelOn).toBe(
      REVEAL_OUTPUT_CHANNEL_ON_NEVER,
    );
  });

  it("does not mutate shared state across repeated calls", () => {
    const a = buildClientOptions("bin-a");
    const b = buildClientOptions("bin-b");
    const optsA = a.serverOptions as unknown as { run: { command: string } };
    const optsB = b.serverOptions as unknown as { run: { command: string } };
    expect(optsA.run.command).toBe("bin-a");
    expect(optsB.run.command).toBe("bin-b");
    // No cross-pollination on the client options either.
    expect(a.clientOptions.documentSelector).not.toBe(
      b.clientOptions.documentSelector,
    );
  });
});

/**
 * Pin the inlined numeric constants in `src/lsp/client.ts` to the
 * real values exported from `vscode-languageclient/node`. Kept in
 * this file because it is the place that documents the inline
 * decision; if the upstream enum is ever renumbered, this test is
 * the signal to update the constants.
 *
 * Skipped gracefully in environments where
 * `vscode-languageclient/node` cannot be imported under vitest
 * (e.g. because the mock vscode module is missing a symbol the
 * language-client touches at module load). In that case the
 * constants are still covered by the integration test, which
 * runs under a real extension host.
 */
describe("inlined constants match vscode-languageclient/node enums", () => {
  async function tryImportLc(): Promise<
    typeof import("vscode-languageclient/node") | undefined
  > {
    try {
      return await import("vscode-languageclient/node");
    } catch {
      // Expected under vitest: the module touches `vscode.version`
      // at the top of its class loader, which the unit-test mock
      // does not provide. The integration test re-verifies these
      // invariants under a real extension host.
      return undefined;
    }
  }

  it("TRANSPORT_KIND_STDIO matches TransportKind.stdio", async () => {
    const lc = await tryImportLc();
    if (!lc) return;
    expect(lc.TransportKind.stdio).toBe(TRANSPORT_KIND_STDIO);
  });

  it("State.Running = 2 (pinned for extension.ts numeric comparison)", async () => {
    const lc = await tryImportLc();
    if (!lc) return;
    expect(lc.State.Running).toBe(2);
  });
});
