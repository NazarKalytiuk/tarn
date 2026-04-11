import * as vscode from "vscode";
import type {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";
import { getOutputChannel } from "../outputChannel";

/**
 * Stable identifier for the language client. Shown in the output
 * dropdown and used as the telemetry-safe client id inside
 * `vscode-languageclient`. Kept in one place so the integration
 * test can assert the output channel name without reimporting the
 * whole module.
 */
export const TARN_LSP_CLIENT_ID = "tarn-lsp";
export const TARN_LSP_CLIENT_NAME = "Tarn LSP";

/**
 * `TransportKind.stdio` is a numeric enum value in
 * `vscode-languageclient/node` that has been `0` since at least
 * version 7.x of the package. Inlining the literal avoids a
 * runtime `require("vscode-languageclient/node")` from
 * `buildClientOptions`, which in turn lets this file be imported
 * from vitest unit tests without dragging the full language-client
 * module chain (which touches real `vscode` APIs) into the mock
 * environment. See `tests/unit/lspClient.test.ts` for the lint
 * that pins the numeric value to the enum and fails loudly if an
 * upstream bump ever renumbers it.
 */
export const TRANSPORT_KIND_STDIO = 0;

/**
 * `RevealOutputChannelOn.Never` is likewise a numeric enum
 * constant from `vscode-languageclient/common/client`. We inline
 * it for the same reason: keep the pure builder free of runtime
 * imports from the language-client module. See the lint in
 * `tests/unit/lspClient.test.ts`.
 */
export const REVEAL_OUTPUT_CHANNEL_ON_NEVER = 4;

/**
 * Pure, unit-testable builder. Produces the `(serverOptions,
 * clientOptions)` pair the `LanguageClient` constructor needs
 * without performing any side effects — no process spawns, no
 * output-channel creation, no VS Code `registerProvider` calls,
 * and no runtime import of `vscode-languageclient/node`.
 *
 * Exposed so unit tests can pin the shape of the client wiring
 * (binary path, stdio transport, document selector, output
 * channel name) without instantiating a real `LanguageClient` —
 * which requires a live VS Code extension host, not a vitest
 * process.
 *
 * @param binaryPath Absolute path or PATH-resolvable command
 *   returned by {@link resolveTarnLspBinary}.
 */
export function buildClientOptions(binaryPath: string): {
  serverOptions: ServerOptions;
  clientOptions: LanguageClientOptions;
} {
  // Typed via a shared local so the `run` and `debug` shapes stay
  // in lockstep. Cast once at the assembly step rather than
  // sprinkling `as unknown as TransportKind.stdio` throughout.
  const serverEntry = {
    command: binaryPath,
    args: [] as string[],
    transport: TRANSPORT_KIND_STDIO,
    options: {
      // `shell: false` is the `vscode-languageclient` default for
      // stdio-based servers. We set it explicitly so a binary path
      // that happens to contain a space cannot accidentally be
      // interpreted by a shell.
      shell: false,
    },
  };

  // Same entry for both run and debug — `tarn-lsp` has no separate
  // debug build inside the extension. The `transport` field tells
  // `vscode-languageclient` to frame messages over stdio, which is
  // what `tarn-lsp`'s `main.rs` speaks.
  const serverOptions = {
    run: serverEntry,
    debug: serverEntry,
  } as unknown as ServerOptions;

  const clientOptions: LanguageClientOptions = {
    // Restrict the client to on-disk `.tarn.yaml` files so the
    // scaffold does NOT intercept in-memory untitled documents
    // (which would race the existing direct providers). This
    // matches the document selector every other Tarn provider
    // already uses.
    documentSelector: [{ language: "tarn", scheme: "file" }],
    // Dedicated output channel so an engineer eyeballing an LSP
    // regression can trace the protocol exchange without wading
    // through the main "Tarn" channel. Lazily created by
    // `vscode-languageclient` on first use; the pair
    // (outputChannelName, traceOutputChannelName) collapses into
    // a single channel when trace is off.
    outputChannelName: TARN_LSP_CLIENT_NAME,
    // Surface unsupported server features as engineer-facing log
    // lines, not end-user toasts. The end user of an experimental
    // LSP scaffold should never see a popup — that is the whole
    // point of the dual-host migration plan.
    revealOutputChannelOn: REVEAL_OUTPUT_CHANNEL_ON_NEVER,
  };

  return { serverOptions, clientOptions };
}

/**
 * Start the `tarn-lsp` language client side-by-side with the
 * in-process providers. Registered for on-disk `.tarn.yaml`
 * files only. The returned client is already `start()`ed and
 * has completed the `initialize` handshake by the time this
 * Promise resolves.
 *
 * Impure: this spawns a child process, registers VS Code
 * providers, and wires `context.subscriptions` to dispose
 * cleanly on extension deactivate.
 *
 * Returns `undefined` if `binaryPath` is empty — the caller is
 * expected to have already decided not to start the client in
 * that case, but we defend against the edge so activation
 * never panics.
 */
export async function startTarnLspClient(
  context: vscode.ExtensionContext,
  binaryPath: string,
): Promise<LanguageClient | undefined> {
  if (binaryPath.trim().length === 0) {
    // l10n-ignore: debug log for engineers.
    getOutputChannel().appendLine(
      "[tarn-lsp] startTarnLspClient skipped: empty binary path",
    );
    return undefined;
  }

  // Dynamic import so `buildClientOptions` callers (and the unit
  // tests that exercise it) never pull in the
  // `vscode-languageclient/node` runtime, which touches the real
  // `vscode` namespace at module load. Under production (the
  // extension host), this `import()` is fully synchronous after
  // esbuild has bundled it; under vitest it's simply never called.
  //
  // The `.js` subpath is required under `moduleResolution: Node16`:
  // `vscode-languageclient` ships a `node.js`/`node.d.ts` shim
  // under its package root but does not expose it via an `exports`
  // map, so the TypeScript Node16 resolver needs the explicit
  // extension to find it. Fixed in 9.x's shim layout.
  const lc = (await import(
    "vscode-languageclient/node.js"
  )) as typeof import("vscode-languageclient/node");

  const { serverOptions, clientOptions } = buildClientOptions(binaryPath);

  const client = new lc.LanguageClient(
    TARN_LSP_CLIENT_ID,
    TARN_LSP_CLIENT_NAME,
    serverOptions,
    clientOptions,
  );

  // l10n-ignore: debug log for engineers.
  getOutputChannel().appendLine(
    `[tarn-lsp] starting language client (binary=${binaryPath})`,
  );

  // `context.subscriptions` owns the client lifetime. When the
  // extension deactivates, `dispose()` is invoked via the
  // ExtensionContext contract. `client.stop()` returns a
  // Promise; we fire-and-forget here because VS Code's
  // `dispose` is declared sync. The `deactivate()` function in
  // `extension.ts` also awaits `client.stop()` explicitly so
  // the `shutdown`/`exit` handshake has time to drain.
  context.subscriptions.push({
    dispose: () => {
      if (client.state !== lc.State.Stopped) {
        void client.stop().catch((err) => {
          // l10n-ignore: debug log for engineers.
          getOutputChannel().appendLine(
            `[tarn-lsp] client.stop() failed: ${String(err)}`,
          );
        });
      }
    },
  });

  // `start()` on vscode-languageclient 9.x returns a Promise that
  // resolves when the `initialize` handshake completes. If the
  // server crashes during startup the Promise rejects; we let the
  // caller handle that so activation can downgrade to a warning
  // toast instead of a hard crash.
  await client.start();

  return client;
}
