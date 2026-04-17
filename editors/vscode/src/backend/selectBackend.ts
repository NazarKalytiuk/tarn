import * as vscode from "vscode";
import type { TarnBackend } from "./TarnBackend";
import { TarnMcpClient } from "./TarnMcpClient";
import { getOutputChannel } from "../outputChannel";
import { readBackendKind } from "../config";

/**
 * Result of {@link selectBackend}.
 *
 * The caller uses `backend` as the primary `TarnBackend`. `mcpClient`
 * is populated only when the MCP backend was successfully promoted,
 * so `extension.ts` can hold onto it across the activation lifetime
 * and dispose the underlying child process on deactivate.
 *
 * `fellBack` is true when the user asked for MCP but the extension
 * silently switched to the CLI (missing binary, handshake failure).
 * The caller is expected to call {@link showMcpFallbackNoticeOnce}
 * exactly once per session on that branch so the user sees a
 * breadcrumb explaining why.
 */
export interface SelectedBackend {
  readonly backend: TarnBackend;
  readonly mcpClient: TarnMcpClient | undefined;
  readonly fellBack: boolean;
}

/**
 * Spawn hook for {@link selectBackend}. Returns an already-constructed
 * {@link TarnMcpClient} backed by the resolved binary path, or throws
 * if the binary cannot be resolved / the handshake fails.
 *
 * Kept as a caller-supplied function so unit tests can inject a
 * scripted client without touching the real `child_process` spawn.
 */
export type McpClientFactory = (
  fallback: TarnBackend,
  workspaceRoot: string | undefined,
) => Promise<TarnMcpClient>;

/**
 * Decide which backend the extension should use for this activation.
 *
 * Reads `tarn.backend` from the active workspace configuration. When
 * the user selects `"mcp"` we attempt to construct the MCP client via
 * {@link factory}; on any error we log it, mark the fallback, and use
 * the CLI backend instead. The caller (extension.ts) is responsible
 * for showing the one-shot notification — this helper stays free of
 * side effects beyond the output channel log so unit tests can assert
 * the fallback flag directly.
 */
export async function selectBackend(
  cliBackend: TarnBackend,
  workspaceRoot: string | undefined,
  factory: McpClientFactory,
): Promise<SelectedBackend> {
  const kind = readBackendKind();
  if (kind !== "mcp") {
    return { backend: cliBackend, mcpClient: undefined, fellBack: false };
  }
  const output = getOutputChannel();
  try {
    const client = await factory(cliBackend, workspaceRoot);
    // l10n-ignore: debug log for engineers.
    output.appendLine("[tarn-mcp] backend ready");
    return { backend: client, mcpClient: client, fellBack: false };
  } catch (err) {
    // l10n-ignore: debug log for engineers.
    output.appendLine(
      `[tarn-mcp] falling back to CLI backend: ${err instanceof Error ? err.message : String(err)}`,
    );
    return { backend: cliBackend, mcpClient: undefined, fellBack: true };
  }
}

/**
 * Module-scoped latch so the MCP fallback notification fires at most
 * once per extension-host session. Cleared by
 * {@link resetMcpFallbackNoticeLatch} from unit tests only.
 */
let shownMcpFallbackNotice = false;

/**
 * Show the MCP-to-CLI fallback notification exactly once per session.
 * Exported so `extension.ts` can call it from the `selectBackend`
 * wrapper and so unit tests can assert the one-shot latch holds.
 */
export function showMcpFallbackNoticeOnce(): void {
  if (shownMcpFallbackNotice) {
    return;
  }
  shownMcpFallbackNotice = true;
  void vscode.window.showInformationMessage(
    vscode.l10n.t(
      "Tarn is falling back to the CLI backend: tarn-mcp could not be started. Install tarn-mcp or set 'tarn.mcpPath' to point at a working binary.",
    ),
  );
}

/**
 * Test-only: reset the one-shot latch so a subsequent
 * {@link showMcpFallbackNoticeOnce} call will show the toast again.
 */
export function resetMcpFallbackNoticeLatch(): void {
  shownMcpFallbackNotice = false;
}
