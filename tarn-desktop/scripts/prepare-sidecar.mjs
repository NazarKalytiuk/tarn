#!/usr/bin/env node
/**
 * prepare-sidecar.mjs
 *
 * Places a `tarn` binary into `src-tauri/binaries/tarn-<target>(.exe)`
 * so Tauri's `bundle.externalBin` can pick it up at compile time.
 *
 * Lookup order, host-target by default, override with `--target=<triple>`:
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

function hostTriple() {
  const out = execSync("rustc -vV", { encoding: "utf-8" });
  const m = out.match(/^host: (.+)$/m);
  if (!m) throw new Error("could not parse rustc -vV output for host target");
  return m[1].trim();
}

const targetArg = process.argv.find((a) => a.startsWith("--target="));
const target = targetArg ? targetArg.slice("--target=".length) : hostTriple();
const ext = target.includes("windows") ? ".exe" : "";

const binariesDir = join(studioRoot, "src-tauri", "binaries");
mkdirSync(binariesDir, { recursive: true });

const dest = join(binariesDir, `tarn-${target}${ext}`);
if (existsSync(dest)) {
  console.log(`[prepare-sidecar] already present: ${dest}`);
  process.exit(0);
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
