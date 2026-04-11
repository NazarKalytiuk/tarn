import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { getOutputChannel } from "../outputChannel";

export interface ResolvedLspBinary {
  readonly path: string;
}

/**
 * Error thrown when the `tarn-lsp` binary cannot be located.
 * Symmetric with `BinaryNotFoundError` from
 * `backend/binaryResolver.ts` so call sites can react uniformly.
 */
export class LspBinaryNotFoundError extends Error {
  constructor(binaryPath: string, cause: unknown) {
    super(
      vscode.l10n.t(
        "tarn-lsp binary not found at '{0}'. Set 'tarn.lspBinaryPath' in settings or install tarn-lsp. Cause: {1}",
        binaryPath,
        String(cause),
      ),
    );
    this.name = "LspBinaryNotFoundError";
  }
}

/**
 * Pure helper: given the user's configured `tarn.lspBinaryPath`
 * setting value, return the effective command string to spawn.
 *
 * Split out from {@link resolveTarnLspBinary} so unit tests can
 * pin the "setting → command" mapping without touching the file
 * system. Mirrors the setting-first, PATH-fallback split in
 * `backend/binaryResolver.ts`, where `readConfig().binaryPath` is
 * the command for `tarn`.
 *
 * - Undefined / empty / whitespace → default to `"tarn-lsp"`
 *   (resolved via `$PATH` when spawned).
 * - Non-empty string → returned as-is for PATH lookups, or
 *   normalized via `path.resolve` for absolute paths so a
 *   mixed-separator value does not slip through.
 *
 * Note: unlike `tarn`, the LSP server does not implement
 * `--version` — it is a pure stdio protocol server. We therefore
 * cannot "probe" the binary the same way `binaryResolver.ts`
 * does; the handshake itself is the verification step, performed
 * by the language client in `client.ts`.
 */
export function resolveTarnLspCommand(configured: string | undefined): string {
  if (configured === undefined || configured.trim().length === 0) {
    return "tarn-lsp";
  }
  const trimmed = configured.trim();
  if (path.isAbsolute(trimmed)) {
    return path.resolve(trimmed);
  }
  return trimmed;
}

/**
 * Read the `tarn.lspBinaryPath` setting for a given scope. Kept
 * separate from {@link resolveTarnLspCommand} so unit tests can
 * exercise the pure mapping without a `vscode.WorkspaceConfiguration`
 * mock.
 */
export function readLspBinaryPathSetting(
  scope?: vscode.Uri,
): string | undefined {
  const cfg = vscode.workspace.getConfiguration("tarn", scope);
  // `get<string>` returns `undefined` when the key is absent and a
  // non-empty string override when the user set one. We deliberately
  // do NOT pass a default value — empty string vs undefined is a
  // signal the caller uses downstream.
  return cfg.get<string>("lspBinaryPath");
}

/**
 * Resolve the `tarn-lsp` binary path. Returns the command string
 * the language client should spawn.
 *
 * - If the resolved command is an absolute path, the file must
 *   exist on disk. Missing file → `LspBinaryNotFoundError`, with
 *   the underlying `fs` error as cause.
 * - If the resolved command is a bare name (e.g. `"tarn-lsp"`),
 *   we defer resolution to the OS `PATH` at spawn time. A missing
 *   binary surfaces later as a spawn failure which the language
 *   client wraps and reports via the output channel.
 *
 * The caller — `startTarnLspClient` in `client.ts` — decides
 * whether a missing binary is fatal (activation crash) or
 * advisory (a one-shot toast). For the experimental Phase V1
 * scaffold it is advisory: activation continues with direct
 * providers only.
 */
export async function resolveTarnLspBinary(
  scope?: vscode.Uri,
): Promise<ResolvedLspBinary> {
  const configured = readLspBinaryPathSetting(scope);
  const command = resolveTarnLspCommand(configured);

  if (path.isAbsolute(command)) {
    try {
      await fs.promises.access(command, fs.constants.X_OK);
    } catch (err) {
      throw new LspBinaryNotFoundError(command, err);
    }
  }

  // l10n-ignore: debug log for engineers, shown with [tarn-lsp] prefix.
  getOutputChannel().appendLine(`[tarn-lsp] resolved binary ${command}`);
  return { path: command };
}
