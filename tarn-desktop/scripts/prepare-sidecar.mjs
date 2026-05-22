#!/usr/bin/env node
/**
 * prepare-sidecar.mjs
 *
 * Places a `tarn` binary at `src-tauri/binaries/tarn-<target>(.exe)`
 * so Tauri's `bundle.externalBin` can pick it up at compile time.
 *
 * Special case: `--target=universal-apple-darwin` is not a real
 * rustc target. We build both `aarch64-apple-darwin` and
 * `x86_64-apple-darwin`, then `lipo`-merge them into a single fat
 * binary at `src-tauri/binaries/tarn-universal-apple-darwin`.
 *
 * Lookup order for a single-arch target (override with --target=<triple>):
 *   1. `$TARN_BIN` (explicit override)
 *   2. `<workspace>/target/<target>/release/tarn`   (cross-built)
 *   3. `<workspace>/target/<target>/debug/tarn`
 *   4. `<workspace>/target/release/tarn`            (default profile)
 *   5. `<workspace>/target/debug/tarn`
 *   6. `cargo build --release -p tarn --target <target>` (last resort)
 *
 * The CLI workspace is assumed to be the parent of `tarn-desktop/`.
 * Used by `pnpm tauri:dev` / `pnpm tauri:build` and by CI.
 */

import { execSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const studioRoot = resolve(here, "..");
const workspaceRoot = resolve(studioRoot, "..");
const binariesDir = join(studioRoot, "src-tauri", "binaries");

function hostTriple() {
  const out = execSync("rustc -vV", { encoding: "utf-8" });
  const m = out.match(/^host: (.+)$/m);
  if (!m) throw new Error("could not parse rustc -vV output for host target");
  return m[1].trim();
}

function ensureSingleArch(target) {
  const ext = target.includes("windows") ? ".exe" : "";
  const dest = join(binariesDir, `tarn-${target}${ext}`);
  if (existsSync(dest)) {
    console.log(`[prepare-sidecar] already present: ${dest}`);
    return dest;
  }

  const candidates = [
    process.env.TARN_BIN,
    join(workspaceRoot, "target", target, "release", `tarn${ext}`),
    join(workspaceRoot, "target", target, "debug", `tarn${ext}`),
    join(workspaceRoot, "target", "release", `tarn${ext}`),
    join(workspaceRoot, "target", "debug", `tarn${ext}`),
  ].filter(Boolean);

  let source = candidates.find((p) => existsSync(p));

  if (!source) {
    console.log(`[prepare-sidecar] no existing tarn binary; building for ${target}…`);
    execSync(`cargo build --release -p tarn --target ${target}`, {
      cwd: workspaceRoot,
      stdio: "inherit",
    });
    source = join(workspaceRoot, "target", target, "release", `tarn${ext}`);
  }

  copyFileSync(source, dest);
  console.log(`[prepare-sidecar] copied ${source} → ${dest}`);
  return dest;
}

function prepareUniversalAppleDarwin() {
  const dest = join(binariesDir, "tarn-universal-apple-darwin");
  if (existsSync(dest)) {
    console.log(`[prepare-sidecar] already present: ${dest}`);
    return;
  }

  if (process.platform !== "darwin") {
    throw new Error(
      "universal-apple-darwin can only be produced on macOS (lipo is Darwin-only)",
    );
  }

  const arm = ensureSingleArch("aarch64-apple-darwin");
  const x64 = ensureSingleArch("x86_64-apple-darwin");

  console.log(`[prepare-sidecar] lipo ${arm} + ${x64} → ${dest}`);
  execSync(`lipo -create ${JSON.stringify(arm)} ${JSON.stringify(x64)} -output ${JSON.stringify(dest)}`, {
    stdio: "inherit",
  });
  execSync(`file ${JSON.stringify(dest)}`, { stdio: "inherit" });
}

const targetArg = process.argv.find((a) => a.startsWith("--target="));
const target = targetArg ? targetArg.slice("--target=".length) : hostTriple();
mkdirSync(binariesDir, { recursive: true });

if (target === "universal-apple-darwin") {
  prepareUniversalAppleDarwin();
} else {
  ensureSingleArch(target);
}
