import * as path from "path";
import * as fs from "fs";
import * as cp from "child_process";
import {
  downloadAndUnzipVSCode,
  resolveCliArgsFromVSCodeExecutablePath,
  runTests,
} from "@vscode/test-electron";

async function main(): Promise<void> {
  try {
    const extensionDevelopmentPath = path.resolve(__dirname, "../../..");
    const extensionTestsPath = path.resolve(__dirname, "./suite/index");
    const fixtureWorkspace = path.resolve(__dirname, "../fixtures/workspace");

    // Use the freshly built tarn debug binary so --select and --ndjson
    // are available. Integration tests always exercise the source tree's
    // CLI, not whichever tarn happens to be on PATH.
    const tarnDebugBinary = path.resolve(
      __dirname,
      "../../../../../target/debug/tarn",
    );
    if (!fs.existsSync(tarnDebugBinary)) {
      throw new Error(
        `tarn debug binary not found at ${tarnDebugBinary}. Run 'cargo build' in the tarn crate first.`,
      );
    }

    // Phase V1 (NAZ-309): if the `tarn-lsp` debug binary is also
    // available, the integration test for the experimental LSP
    // client can point at it via a workspace setting. Missing
    // binary is not fatal — the lspClient.test.ts suite skips
    // its assertions gracefully in that case. This mirrors the
    // advisory-not-fatal behavior of the runtime resolver.
    const tarnLspDebugBinary = path.resolve(
      __dirname,
      "../../../../../target/debug/tarn-lsp",
    );
    const hasLspBinary = fs.existsSync(tarnLspDebugBinary);

    const vscodeDir = path.join(fixtureWorkspace, ".vscode");
    fs.mkdirSync(vscodeDir, { recursive: true });
    const workspaceSettings: Record<string, unknown> = {
      "tarn.binaryPath": tarnDebugBinary,
      // Default the experimental LSP client OFF for every suite
      // except `lspClient.test.ts`, which flips it on for its
      // own scope and turns it back off in `afterEach`. Keeps
      // the other 19 integration suites from spawning an LSP
      // server they do not exercise.
      "tarn.experimentalLspClient": false,
    };
    if (hasLspBinary) {
      workspaceSettings["tarn.lspBinaryPath"] = tarnLspDebugBinary;
    }
    fs.writeFileSync(
      path.join(vscodeDir, "settings.json"),
      JSON.stringify(workspaceSettings, null, 2),
    );

    const vscodeExecutablePath = await downloadAndUnzipVSCode();
    const [cliPath, ...cliArgs] = resolveCliArgsFromVSCodeExecutablePath(vscodeExecutablePath);

    cp.spawnSync(
      cliPath,
      [...cliArgs, "--install-extension", "redhat.vscode-yaml", "--force"],
      { encoding: "utf-8", stdio: "inherit" },
    );

    await runTests({
      vscodeExecutablePath,
      extensionDevelopmentPath,
      extensionTestsPath,
      launchArgs: [fixtureWorkspace],
    });
  } catch (err) {
    console.error("Failed to run integration tests:", err);
    process.exit(1);
  }
}

main();
