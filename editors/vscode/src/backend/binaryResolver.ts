import { execFile } from "child_process";
import * as fs from "fs";
import * as path from "path";
import { promisify } from "util";
import * as vscode from "vscode";
import { getOutputChannel } from "../outputChannel";
import { readConfig, readMcpPath } from "../config";

const execFileAsync = promisify(execFile);

export interface ResolvedBinary {
  path: string;
  version: string;
}

export interface ResolvedMcpBinary {
  readonly path: string;
}

export class BinaryNotFoundError extends Error {
  constructor(binaryPath: string, cause: unknown) {
    super(
      vscode.l10n.t(
        "Tarn binary not found at '{0}'. Set 'tarn.binaryPath' in settings or install tarn. Cause: {1}",
        binaryPath,
        String(cause),
      ),
    );
    this.name = "BinaryNotFoundError";
  }
}

/**
 * Error thrown when the `tarn-mcp` binary cannot be located. Symmetric
 * with {@link BinaryNotFoundError} so call sites in `extension.ts` can
 * handle both the CLI-missing and MCP-missing paths the same way.
 */
export class McpBinaryNotFoundError extends Error {
  constructor(binaryPath: string, cause: unknown) {
    super(
      vscode.l10n.t(
        "tarn-mcp binary not found at '{0}'. Set 'tarn.mcpPath' in settings or install tarn-mcp. Cause: {1}",
        binaryPath,
        String(cause),
      ),
    );
    this.name = "McpBinaryNotFoundError";
  }
}

export async function resolveBinary(scope?: vscode.Uri): Promise<ResolvedBinary> {
  const { binaryPath } = readConfig(scope);
  try {
    const { stdout } = await execFileAsync(binaryPath, ["--version"], { timeout: 5000 });
    const version = stdout.trim();
    // l10n-ignore: debug log for engineers, shown with [tarn] prefix.
    getOutputChannel().appendLine(`[tarn] resolved binary ${binaryPath} (${version})`);
    return { path: binaryPath, version };
  } catch (err) {
    throw new BinaryNotFoundError(binaryPath, err);
  }
}

export async function promptInstallIfMissing(scope?: vscode.Uri): Promise<ResolvedBinary | undefined> {
  try {
    return await resolveBinary(scope);
  } catch (err) {
    const installAction = vscode.l10n.t("Install Instructions");
    const settingsAction = vscode.l10n.t("Open Settings");
    const choice = await vscode.window.showErrorMessage(
      err instanceof Error ? err.message : String(err),
      installAction,
      settingsAction,
    );
    if (choice === installAction) {
      await vscode.env.openExternal(
        vscode.Uri.parse("https://github.com/NazarKalytiuk/tarn#install"),
      );
    } else if (choice === settingsAction) {
      await vscode.commands.executeCommand("workbench.action.openSettings", "tarn.binaryPath");
    }
    return undefined;
  }
}

/**
 * Pure helper: given the user's configured `tarn.mcpPath` setting value,
 * return the effective command string to spawn.
 *
 * Split out from {@link resolveMcpBinary} so unit tests can pin the
 * "setting → command" mapping without touching the file system. Mirrors
 * the same pattern `tarnLspResolver.ts` uses for `tarn-lsp`.
 *
 * - Undefined / empty / whitespace → default to `"tarn-mcp"` (resolved
 *   via `$PATH` when spawned).
 * - Absolute path → normalized via `path.resolve` so mixed-separator
 *   values never slip through.
 * - Bare name → returned as-is for PATH lookups at spawn time.
 */
export function resolveMcpCommand(configured: string | undefined): string {
  if (configured === undefined || configured.trim().length === 0) {
    return "tarn-mcp";
  }
  const trimmed = configured.trim();
  if (path.isAbsolute(trimmed)) {
    return path.resolve(trimmed);
  }
  return trimmed;
}

/**
 * Resolve the `tarn-mcp` binary path. Returns the command string the
 * MCP client should spawn.
 *
 * Follows the same rules as `tarn`: a configured `tarn.mcpPath` wins,
 * otherwise we return the bare `"tarn-mcp"` and let the OS `PATH`
 * resolve it at spawn time. Absolute paths are probed with
 * `fs.access(X_OK)` so a missing file surfaces as a clear
 * {@link McpBinaryNotFoundError} here rather than a confusing spawn
 * failure downstream.
 *
 * `tarn-mcp` does not implement `--version` — it is a pure JSON-RPC
 * stdio server — so we cannot probe the binary the same way
 * {@link resolveBinary} probes `tarn`. Absolute paths are verified via
 * `fs.access(X_OK)`; PATH-resolved commands are verified by the
 * `initialize` handshake the MCP client performs at startup.
 */
export async function resolveMcpBinary(scope?: vscode.Uri): Promise<ResolvedMcpBinary> {
  const configured = readMcpPath(scope);
  const command = resolveMcpCommand(configured);

  if (path.isAbsolute(command)) {
    try {
      await fs.promises.access(command, fs.constants.X_OK);
    } catch (err) {
      throw new McpBinaryNotFoundError(command, err);
    }
  }

  // l10n-ignore: debug log for engineers, shown with [tarn-mcp] prefix.
  getOutputChannel().appendLine(`[tarn-mcp] resolved binary ${command}`);
  return { path: command };
}
