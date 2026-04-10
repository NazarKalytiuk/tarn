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

    const vscodeDir = path.join(fixtureWorkspace, ".vscode");
    fs.mkdirSync(vscodeDir, { recursive: true });
    fs.writeFileSync(
      path.join(vscodeDir, "settings.json"),
      JSON.stringify({ "tarn.binaryPath": tarnDebugBinary }, null, 2),
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
