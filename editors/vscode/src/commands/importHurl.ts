import * as path from "path";
import * as vscode from "vscode";
import type { TarnBackend } from "../backend/TarnBackend";
import { getOutputChannel } from "../outputChannel";

export interface ImportHurlDeps {
  backend: TarnBackend;
}

export function registerImportHurlCommand(
  deps: ImportHurlDeps,
): vscode.Disposable {
  return vscode.commands.registerCommand("tarn.importHurl", async () => {
    await runImportHurlWizard(deps);
  });
}

async function runImportHurlWizard(deps: ImportHurlDeps): Promise<void> {
  const source = await pickHurlSource();
  if (!source) return;

  const defaultDest = vscode.Uri.file(defaultHurlDestination(source.fsPath));
  const dest = await vscode.window.showSaveDialog({
    defaultUri: defaultDest,
    filters: { Tarn: ["tarn.yaml", "tarn.yml", "yaml", "yml"] },
    saveLabel: "Import Hurl File",
    title: "Choose destination for imported Tarn file",
  });
  if (!dest) return;

  const cwd = resolveCwd(source);
  const result = await runImportHurl(deps.backend, source.fsPath, dest.fsPath, cwd);
  if (!result.success) {
    vscode.window.showErrorMessage(
      `Tarn: import-hurl failed${
        result.exitCode !== null ? ` (exit ${result.exitCode})` : ""
      }. Check the Tarn output channel for details.`,
    );
    return;
  }

  try {
    const doc = await vscode.workspace.openTextDocument(dest);
    await vscode.window.showTextDocument(doc, { preview: false });
  } catch (err) {
    vscode.window.showErrorMessage(
      `Tarn: imported file but could not open it: ${String(err)}`,
    );
    return;
  }

  const action = await vscode.window.showInformationMessage(
    `Tarn: imported ${path.basename(dest.fsPath)}`,
    "Run",
    "Validate",
  );
  if (action === "Run") {
    await vscode.commands.executeCommand("tarn.runFile");
  } else if (action === "Validate") {
    await vscode.commands.executeCommand("tarn.validateFile");
  }
}

async function pickHurlSource(): Promise<vscode.Uri | undefined> {
  const uris = await vscode.window.showOpenDialog({
    canSelectFiles: true,
    canSelectFolders: false,
    canSelectMany: false,
    openLabel: "Import Hurl File",
    title: "Select a .hurl file to import",
    filters: { Hurl: ["hurl"] },
  });
  return uris?.[0];
}

function resolveCwd(source: vscode.Uri): string {
  const folder = vscode.workspace.getWorkspaceFolder(source);
  return folder?.uri.fsPath ?? path.dirname(source.fsPath);
}

/**
 * Drive the backend's `importHurl` with the supplied paths, log
 * outcomes, and return a success flag. Split out from the wizard so
 * the integration test can exercise the spawn-and-open path without
 * driving the VS Code dialogs.
 */
export async function runImportHurl(
  backend: TarnBackend,
  source: string,
  dest: string,
  cwd: string,
): Promise<{ success: boolean; exitCode: number | null; stderr: string }> {
  const out = getOutputChannel();
  const cts = new vscode.CancellationTokenSource();
  out.appendLine(`[tarn] import-hurl ${source} -> ${dest}`);
  const result = await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: `Tarn: importing ${path.basename(source)}…`,
      cancellable: true,
    },
    async (_progress, token) => {
      token.onCancellationRequested(() => cts.cancel());
      return backend.importHurl(source, dest, cwd, cts.token);
    },
  );
  cts.dispose();
  if (result.exitCode !== 0) {
    if (result.stderr) out.appendLine(result.stderr.trimEnd());
    if (result.stdout) out.appendLine(result.stdout.trimEnd());
    out.show(true);
    return { success: false, exitCode: result.exitCode, stderr: result.stderr };
  }
  if (result.stdout.trim().length > 0) {
    out.appendLine(result.stdout.trimEnd());
  }
  return { success: true, exitCode: result.exitCode, stderr: result.stderr };
}

/**
 * Compute the default destination path for an imported Hurl file.
 * Strips the `.hurl` suffix (preserving any prior segments) and
 * appends `.tarn.yaml` as a sibling. Used by the save dialog and
 * also exported for unit tests.
 */
export function defaultHurlDestination(sourcePath: string): string {
  const dir = path.dirname(sourcePath);
  const base = path.basename(sourcePath);
  const stem = base.toLowerCase().endsWith(".hurl")
    ? base.slice(0, -".hurl".length)
    : base;
  return path.join(dir, `${stem}.tarn.yaml`);
}
